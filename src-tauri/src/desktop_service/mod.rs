//! Application orchestration for desktops; separate modules own protocols, routes, and secure windows.
mod commands;
mod gateway;
mod prompts;
pub(crate) use commands::*;

use crate::{
    host_service::HostService, ssh_agent_service::SshAgentService,
    transient_credential_service::TransientCredentialService, vault_service::VaultService,
};
use norishell_app_persistence::{CredentialRecordDetails, validate_desktop_profile};
use norishell_core_api::{
    DesktopAudioState, DesktopOpenRequest, DesktopProfile, DesktopPromptDecision,
    DesktopPromptKind, DesktopSessionState, DesktopSessionSummary, WireSequence,
};
use norishell_desktop_protocol::{
    AudioMuteState, AudioPlaybackState, DesktopCursor, DesktopFrame, DirtyRegion, EngineCommand,
    EngineControl, EngineError, EngineEvent, EventSink, Result,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::AppHandle;
use tokio::sync::{mpsc, watch};
use zeroize::Zeroizing;

#[derive(Clone)]
pub(crate) struct DesktopService {
    hosts: HostService,
    vault: VaultService,
    transient: TransientCredentialService,
    agent: SshAgentService,
    app: AppHandle,
    sessions: Arc<Mutex<BTreeMap<String, Arc<Session>>>>,
    focus: Arc<tokio::sync::Mutex<Focus>>,
    prompts: prompts::Prompts,
}

#[derive(Default)]
struct Focus {
    session: Option<String>,
    window_label: Option<String>,
    epoch: u64,
    sequence: u64,
}

struct Session {
    projection: Mutex<Projection>,
    commands: mpsc::Sender<EngineCommand>,
    stop: watch::Sender<bool>,
    focus_epoch: watch::Sender<u64>,
    audio_muted: watch::Sender<AudioMuteState>,
    done: watch::Sender<bool>,
    /// Bumped whenever frame or cursor state changes and when the session stops, so frame long-polls wake
    /// without lost notifications (a receiver subscribed before checking observes every later bump).
    changes: watch::Sender<u64>,
    transports: Mutex<Vec<norishell_ssh_transport::TransportCloseHandle>>,
}
struct Projection {
    summary: DesktopSessionSummary,
    /// Core-owned copy of the remote screen. Engines publish full `Frame` replacements and `FramePatch`
    /// deltas; `None` before the first frame and after the session stops (the buffer is released then).
    frame: Option<DesktopFrame>,
    /// Frame sequence last delivered to the WebView; a patch is only valid relative to it.
    frame_base_sequence: u64,
    /// Rectangles changed since `frame_base_sequence` was delivered.
    frame_dirty: DirtyRegion,
    /// Set by every full replacement; the next delivery must then carry the whole frame.
    frame_requires_full: bool,
    /// 0 means the cursor was never set (the view keeps its default arrow).
    cursor_sequence: u64,
    cursor: DesktopCursor,
    clipboard: Option<String>,
    cleanup_failed: bool,
}

impl Projection {
    /// Drops display memory; a stopped session never renders again, so its last frame must not linger
    /// while the Tab still holds the closed session record.
    fn release_display(&mut self) {
        self.frame = None;
        self.frame_dirty = DirtyRegion::default();
        self.frame_requires_full = true;
        self.cursor = DesktopCursor::Default;
    }
}

impl Session {
    fn summary(&self) -> DesktopSessionSummary {
        self.projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .summary
            .clone()
    }
    fn phase(&self, phase: &str) {
        let mut projection = self
            .projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(
            projection.summary.state,
            DesktopSessionState::Connecting | DesktopSessionState::NeedsInteraction
        ) {
            projection.summary.state = DesktopSessionState::Connecting;
            projection.summary.phase = phase.to_owned();
            projection.summary.revision = WireSequence::new(projection.summary.revision.get() + 1);
        }
    }
    fn fail_and_stop(&self, failure: &str) {
        self.projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .summary
            .failure = Some(failure.to_owned());
        self.request_stop();
    }
    fn request_stop(&self) {
        self.focus_epoch
            .send_modify(|epoch| *epoch = epoch.saturating_add(1));
        self.stop.send_replace(true);
        // `event` ignores updates once `stop` is set, so the released buffer is not re-created.
        self.projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .release_display();
        self.notify_change();
    }
    fn notify_change(&self) {
        self.changes
            .send_modify(|value| *value = value.wrapping_add(1));
    }
    fn event(&self, event: EngineEvent) {
        if *self.stop.borrow() {
            return;
        }
        let mut projection = self
            .projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match event {
            EngineEvent::RdpTransport(actual) => {
                projection.summary.rdp_transport_actual = Some(match actual {
                    norishell_desktop_protocol::RdpTransportActual::Tcp => {
                        norishell_core_api::RdpTransportActual::Tcp
                    }
                    norishell_desktop_protocol::RdpTransportActual::Udp => {
                        norishell_core_api::RdpTransportActual::Udp
                    }
                });
                projection.summary.revision =
                    WireSequence::new(projection.summary.revision.get() + 1);
            }
            EngineEvent::RdpGraphics(actual) => {
                let mapped = match actual {
                    norishell_desktop_protocol::RdpGraphicsActual::Bitmap => {
                        norishell_core_api::RdpGraphicsActual::Bitmap
                    }
                    norishell_desktop_protocol::RdpGraphicsActual::RemoteFx => {
                        norishell_core_api::RdpGraphicsActual::RemoteFx
                    }
                    norishell_desktop_protocol::RdpGraphicsActual::RemoteFxProgressive => {
                        norishell_core_api::RdpGraphicsActual::RemoteFxProgressive
                    }
                    norishell_desktop_protocol::RdpGraphicsActual::Avc420 => {
                        norishell_core_api::RdpGraphicsActual::Avc420
                    }
                };
                if projection.summary.rdp_graphics_actual != Some(mapped) {
                    projection.summary.rdp_graphics_actual = Some(mapped);
                    projection.summary.revision =
                        WireSequence::new(projection.summary.revision.get() + 1);
                }
            }
            EngineEvent::AudioState(state) => {
                projection.summary.audio_state = match state {
                    AudioPlaybackState::Waiting => DesktopAudioState::Waiting,
                    AudioPlaybackState::Ready => DesktopAudioState::Ready,
                    AudioPlaybackState::Unavailable => DesktopAudioState::Unavailable,
                    AudioPlaybackState::Unsupported => DesktopAudioState::Unsupported,
                };
                projection.summary.revision =
                    WireSequence::new(projection.summary.revision.get() + 1);
            }
            EngineEvent::Ready => {
                projection.summary.state = DesktopSessionState::Running;
                projection.summary.phase = "running".to_owned();
                projection.summary.revision =
                    WireSequence::new(projection.summary.revision.get() + 1);
            }
            EngineEvent::Frame(frame) => {
                if norishell_desktop_protocol::frame_len(frame.width, frame.height).ok()
                    != Some(frame.rgba.len())
                {
                    drop(projection);
                    self.fail_and_stop("resourceLimit");
                    return;
                }
                // Engines normally hand over their only reference, so this moves rather than copies.
                let frame = Arc::unwrap_or_clone(frame);
                // A full replacement may differ anywhere (first frame, size change, reactivation), so the
                // next delivery is always a full frame.
                projection.frame_dirty.clear();
                projection.frame_requires_full = true;
                projection.summary.width = frame.width;
                projection.summary.height = frame.height;
                projection.summary.frame_sequence =
                    WireSequence::new(projection.summary.frame_sequence.get() + 1);
                projection.frame = Some(frame);
                drop(projection);
                self.notify_change();
            }
            EngineEvent::FramePatch(patch) => {
                if patch.rects.is_empty() {
                    return;
                }
                let applied = projection
                    .frame
                    .as_mut()
                    .ok_or(EngineError::Protocol)
                    .and_then(|frame| frame.apply_patch(&patch));
                if applied.is_err() {
                    drop(projection);
                    self.fail_and_stop("protocolError");
                    return;
                }
                if !projection.frame_requires_full {
                    for (rect, _) in &patch.rects {
                        projection.frame_dirty.add(*rect);
                    }
                }
                projection.summary.frame_sequence =
                    WireSequence::new(projection.summary.frame_sequence.get() + 1);
                drop(projection);
                self.notify_change();
            }
            EngineEvent::Cursor(cursor) => {
                projection.cursor = cursor;
                projection.cursor_sequence = projection.cursor_sequence.wrapping_add(1).max(1);
                drop(projection);
                self.notify_change();
            }
            EngineEvent::Clipboard(text) => {
                if projection.summary.profile.clipboard_enabled
                    && text.len() <= norishell_desktop_protocol::MAX_TEXT_BYTES
                {
                    projection.clipboard = Some(text);
                }
            }
        }
    }
}

impl DesktopService {
    pub(crate) fn new(
        app: AppHandle,
        hosts: HostService,
        vault: VaultService,
        transient: TransientCredentialService,
        agent: SshAgentService,
    ) -> Self {
        Self {
            app,
            hosts,
            vault,
            transient,
            agent,
            sessions: Arc::default(),
            focus: Arc::default(),
            prompts: prompts::Prompts::default(),
        }
    }

    fn session(&self, id: &str, generation: u64) -> Result<Arc<Session>> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| EngineError::Protocol)?
            .get(id)
            .cloned()
            .ok_or(EngineError::InvalidConfiguration)?;
        if session.summary().generation.get() != generation {
            return Err(EngineError::StaleInput);
        }
        Ok(session)
    }

    pub(crate) fn snapshot(&self) -> Vec<DesktopSessionSummary> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(|session| session.summary())
            .collect()
    }

    fn open(&self, request: DesktopOpenRequest) -> Result<DesktopSessionSummary> {
        validate_desktop_profile(&request.profile)
            .map_err(|_| EngineError::InvalidConfiguration)?;
        uuid::Uuid::parse_str(&request.operation_id)
            .map_err(|_| EngineError::InvalidConfiguration)?;
        if request.profile.revision.get() > 0 {
            let profiles = self
                .hosts
                .with_desktop_repository(|repo| repo.list_desktop_profiles())
                .map_err(|_| EngineError::InvalidConfiguration)?;
            if !profiles.iter().any(|profile| profile == &request.profile) {
                return Err(EngineError::InvalidConfiguration);
            }
        }
        let mut sessions = self.sessions.lock().map_err(|_| EngineError::Protocol)?;
        // The owned open publishes this id to its Tab before creation; a replay
        // must never attach a second Tab to an existing session.
        if sessions.contains_key(&request.operation_id) {
            return Err(EngineError::StaleInput);
        }
        if sessions.len() >= 16 {
            return Err(EngineError::ResourceLimit);
        }
        let (commands, receiver) = mpsc::channel(64);
        let (stop, stop_receiver) = watch::channel(false);
        let (focus_epoch, focus_receiver) = watch::channel(0);
        let (done, _) = watch::channel(false);
        let (audio_muted, _) = watch::channel(AudioMuteState::default());
        let (changes, _) = watch::channel(0);
        let summary = DesktopSessionSummary {
            id: request.operation_id.clone(),
            width: request.profile.width,
            height: request.profile.height,
            audio_state: if request.profile.audio_playback_enabled {
                DesktopAudioState::Waiting
            } else {
                DesktopAudioState::Disabled
            },
            audio_muted: false,
            rdp_transport_actual: None,
            rdp_graphics_actual: None,
            profile: request.profile,
            generation: WireSequence::new(1),
            revision: WireSequence::new(1),
            state: DesktopSessionState::Connecting,
            phase: "preparing".to_owned(),
            failure: None,
            frame_sequence: WireSequence::new(0),
        };
        let session = Arc::new(Session {
            projection: Mutex::new(Projection {
                summary: summary.clone(),
                frame: None,
                frame_base_sequence: 0,
                frame_dirty: DirtyRegion::default(),
                frame_requires_full: true,
                cursor_sequence: 0,
                cursor: DesktopCursor::Default,
                clipboard: None,
                cleanup_failed: false,
            }),
            commands,
            stop,
            focus_epoch,
            audio_muted,
            done,
            changes,
            transports: Mutex::new(Vec::new()),
        });
        sessions.insert(request.operation_id, session.clone());
        let service = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut result = service
                .run(
                    session.clone(),
                    receiver,
                    EngineControl {
                        stop: stop_receiver,
                        focus_epoch: focus_receiver,
                    },
                )
                .await;
            let observed = session
                .transports
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            for mut transport in observed {
                if tokio::time::timeout(Duration::from_secs(5), transport.wait_closed())
                    .await
                    .is_err()
                {
                    session
                        .projection
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .cleanup_failed = true;
                    result = Err(EngineError::Timeout);
                }
            }
            service.prompts.cancel_session(&session.summary().id);
            let mut projection = session
                .projection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            projection.summary.state = if projection.summary.failure.is_some()
                || result
                    .as_ref()
                    .is_err_and(|error| *error != EngineError::Cancelled)
            {
                DesktopSessionState::Failed
            } else {
                DesktopSessionState::Closed
            };
            if projection.summary.failure.is_none() {
                projection.summary.failure = result
                    .err()
                    .filter(|error| *error != EngineError::Cancelled)
                    .map(|error| error.to_string());
            }
            projection.summary.revision = WireSequence::new(projection.summary.revision.get() + 1);
            projection.clipboard = None;
            projection.release_display();
            if projection.summary.profile.audio_playback_enabled {
                projection.summary.audio_state = DesktopAudioState::Closed;
            }
            drop(projection);
            session.done.send_replace(true);
            session.notify_change();
        });
        Ok(summary)
    }

    async fn prompt(
        &self,
        session: &Arc<Session>,
        prompt: DesktopPromptKind,
    ) -> Result<DesktopPromptDecision> {
        let _interaction = self.prompts.block_input();
        self.invalidate_input();
        let summary = session.summary();
        session
            .projection
            .lock()
            .map_err(|_| EngineError::Protocol)?
            .summary
            .state = DesktopSessionState::NeedsInteraction;
        let mut stop = session.stop.subscribe();
        if *stop.borrow() {
            return Err(EngineError::Cancelled);
        }
        let result = tokio::select! { biased;
            _ = stop.changed() => Err(EngineError::Cancelled),
            result = self.prompts.request(&self.app, &summary.id, &summary.profile.label, prompt) => result,
        };
        session.phase(&summary.phase);
        result
    }

    async fn credentials(
        &self,
        session: &Arc<Session>,
        profile: &DesktopProfile,
    ) -> Result<(String, Option<String>, Zeroizing<String>)> {
        if let Some(id) = &profile.credential_ref_id {
            self.ensure_vault_unlocked(session).await?;
            let credential = self
                .hosts
                .get_ready_credential(id)
                .map_err(|_| EngineError::AuthenticationRejected)?;
            let CredentialRecordDetails::Password { secret_ref_id } = credential.details else {
                return Err(EngineError::UnsupportedAuthentication);
            };
            let secret = self
                .vault
                .read_secret(&secret_ref_id, norishell_secret_vault::SecretKind::Password)
                .map_err(|_| EngineError::AuthenticationRejected)?;
            let password = std::str::from_utf8(secret.expose())
                .map_err(|_| EngineError::AuthenticationRejected)?;
            return Ok((
                profile.username.clone(),
                (!profile.domain.is_empty()).then(|| profile.domain.clone()),
                Zeroizing::new(password.to_owned()),
            ));
        }
        let mut decision = self
            .prompt(
                session,
                DesktopPromptKind::Credentials {
                    username: profile.username.clone(),
                    domain: profile.domain.clone(),
                    password_only: profile.protocol == norishell_core_api::DesktopProtocol::Vnc,
                },
            )
            .await?;
        let password = Zeroizing::new(
            decision
                .password
                .take()
                .ok_or(EngineError::AuthenticationRejected)?,
        );
        let username = decision
            .username
            .take()
            .unwrap_or_else(|| profile.username.clone());
        let domain = decision.domain.take().filter(|value| !value.is_empty());
        prompts::clear_decision(&mut decision);
        Ok((username, domain, password))
    }

    async fn ensure_vault_unlocked(&self, session: &Arc<Session>) -> Result<()> {
        if self.vault.is_unlocked() {
            return Ok(());
        }
        let create = self.vault.status().state == norishell_core_api::VaultState::Missing;
        let kind = if create {
            DesktopPromptKind::VaultCreate
        } else {
            DesktopPromptKind::VaultUnlock
        };
        let mut decision = self.prompt(session, kind).await?;
        let password = decision.password.take().map(Zeroizing::new);
        let confirmation = decision.password_confirmation.take().map(Zeroizing::new);
        prompts::clear_decision(&mut decision);
        let password = password.ok_or(EngineError::AuthenticationRejected)?;
        let vault = self.vault.clone();
        let pending_session = session.clone();
        tokio::task::spawn_blocking(move || {
            if *pending_session.stop.borrow() {
                return Err(EngineError::Cancelled);
            }
            if create {
                let confirmation = confirmation.ok_or(EngineError::AuthenticationRejected)?;
                vault.create_for_protected_operation(password.as_bytes(), confirmation.as_bytes())
            } else {
                vault.unlock_for_protected_operation(password.as_bytes())
            }
            .map_err(|_| EngineError::AuthenticationRejected)
        })
        .await
        .map_err(|_| EngineError::Protocol)??;
        Ok(())
    }

    async fn run(
        &self,
        session: Arc<Session>,
        receiver: mpsc::Receiver<EngineCommand>,
        mut control: EngineControl,
    ) -> Result<()> {
        let profile = session.summary().profile;
        let prepared = async {
            let credentials = self.credentials(&session, &profile).await?;
            if profile.protocol == norishell_core_api::DesktopProtocol::Vnc {
                self.prompt(
                    &session,
                    DesktopPromptKind::UnencryptedVnc {
                        address: profile.address.clone(),
                        gateway: profile.gateway_host_id.is_some(),
                    },
                )
                .await?;
            }
            let connection = gateway::connect(self, &session).await?;
            Ok::<_, EngineError>((credentials, connection))
        };
        let ((username, domain, password), (stream, udp_peer, cleanup)) = tokio::select! { biased;
            _ = control.stop.changed() => return Err(EngineError::Cancelled),
            result = prepared => result?,
        };
        let weak = Arc::downgrade(&session);
        let events: EventSink = Arc::new(move |event| {
            if let Some(session) = weak.upgrade() {
                session.event(event);
            }
        });
        let result = match profile.protocol {
            norishell_core_api::DesktopProtocol::Rdp => {
                let service = self.clone();
                let approval_session = session.clone();
                let approval: norishell_rdp_client::CertificateApproval =
                    Arc::new(move |challenge| {
                        let service = service.clone();
                        let session = approval_session.clone();
                        Box::pin(async move {
                            service
                                .prompt(
                                    &session,
                                    DesktopPromptKind::Certificate {
                                        address: challenge.server_name,
                                        fingerprint: challenge.sha256_fingerprint,
                                    },
                                )
                                .await
                                .is_ok()
                        })
                    });
                norishell_rdp_client::run(
                    stream,
                    norishell_rdp_client::RdpOptions {
                        server_name: profile.address,
                        username,
                        domain,
                        password,
                        width: profile.width,
                        height: profile.height,
                        clipboard_enabled: profile.clipboard_enabled,
                        audio_playback_enabled: profile.audio_playback_enabled,
                        audio_muted: session.audio_muted.subscribe(),
                        certificate_approval: Some(approval),
                        udp_peer,
                        transport_mode: match profile.rdp_transport_mode {
                            norishell_core_api::RdpTransportMode::Auto => {
                                norishell_rdp_client::TransportMode::Auto
                            }
                            norishell_core_api::RdpTransportMode::TcpOnly => {
                                norishell_rdp_client::TransportMode::TcpOnly
                            }
                            norishell_core_api::RdpTransportMode::UdpRequired => {
                                norishell_rdp_client::TransportMode::UdpRequired
                            }
                        },
                        graphics_mode: match profile.rdp_graphics_mode {
                            norishell_core_api::RdpGraphicsMode::Auto => {
                                norishell_rdp_client::GraphicsMode::Auto
                            }
                            norishell_core_api::RdpGraphicsMode::RemoteFx => {
                                norishell_rdp_client::GraphicsMode::RemoteFx
                            }
                            norishell_core_api::RdpGraphicsMode::Avc420 => {
                                norishell_rdp_client::GraphicsMode::Avc420
                            }
                            norishell_core_api::RdpGraphicsMode::Bitmap => {
                                norishell_rdp_client::GraphicsMode::Bitmap
                            }
                        },
                    },
                    receiver,
                    control,
                    events,
                )
                .await
            }
            norishell_core_api::DesktopProtocol::Vnc => {
                norishell_vnc_client::run(
                    stream,
                    norishell_vnc_client::VncOptions {
                        password,
                        allow_unauthenticated: false,
                        clipboard_enabled: profile.clipboard_enabled,
                        initial_resize: (profile.vnc_resolution_mode
                            == norishell_core_api::VncResolutionMode::Fixed)
                            .then_some((profile.width, profile.height)),
                        version: match profile.vnc_protocol_version {
                            norishell_core_api::VncProtocolVersion::Auto => None,
                            norishell_core_api::VncProtocolVersion::Rfb33 => {
                                Some(norishell_vnc_client::VncVersion::RFB33)
                            }
                            norishell_core_api::VncProtocolVersion::Rfb37 => {
                                Some(norishell_vnc_client::VncVersion::RFB37)
                            }
                            norishell_core_api::VncProtocolVersion::Rfb38 => {
                                Some(norishell_vnc_client::VncVersion::RFB38)
                            }
                        },
                    },
                    receiver,
                    control,
                    events,
                )
                .await
            }
        };
        // Close the owned gateway and heartbeat only after the protocol future finishes and releases its stream.
        if let Err(error) = cleanup.close().await {
            session
                .projection
                .lock()
                .map_err(|_| EngineError::Protocol)?
                .cleanup_failed = true;
            return Err(error);
        }
        result
    }

    async fn disconnect(&self, id: &str, generation: u64) -> Result<()> {
        let session = self.session(id, generation)?;
        session.request_stop();
        self.prompts.cancel_session(id);
        let mut done = session.done.subscribe();
        if !*done.borrow() {
            session
                .projection
                .lock()
                .map_err(|_| EngineError::Protocol)?
                .summary
                .state = DesktopSessionState::Disconnecting;
            tokio::time::timeout(Duration::from_secs(15), async {
                while !*done.borrow() {
                    done.changed()
                        .await
                        .map_err(|_| EngineError::ConnectionLost)?;
                }
                Ok::<_, EngineError>(())
            })
            .await
            .map_err(|_| EngineError::Timeout)??;
        }
        if session
            .projection
            .lock()
            .map_err(|_| EngineError::Protocol)?
            .cleanup_failed
        {
            let observers = session
                .transports
                .lock()
                .map_err(|_| EngineError::Protocol)?;
            if observers.is_empty() || observers.iter().any(|observer| !observer.is_closed()) {
                return Err(EngineError::ConnectionLost);
            }
            session
                .projection
                .lock()
                .map_err(|_| EngineError::Protocol)?
                .cleanup_failed = false;
        }
        Ok(())
    }

    pub(crate) async fn shutdown_all(&self) -> Result<()> {
        let snapshot = self.snapshot();
        // Stop all owners first; one slow cleanup must not leave later sessions running throughout the exit wait.
        for summary in &snapshot {
            if let Ok(session) = self.session(&summary.id, summary.generation.get()) {
                session.request_stop();
                self.prompts.cancel_session(&summary.id);
            }
        }
        let mut cleanup = tokio::task::JoinSet::new();
        for summary in snapshot {
            let service = self.clone();
            cleanup.spawn(async move {
                service
                    .disconnect(&summary.id, summary.generation.get())
                    .await
            });
        }
        let mut result = Ok(());
        while let Some(completed) = cleanup.join_next().await {
            match completed {
                Ok(Ok(())) => {}
                Ok(Err(error)) => result = Err(error),
                Err(_) => result = Err(EngineError::Protocol),
            }
        }
        result
    }

    pub(crate) fn exit_blockers(&self) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, session)| {
                !*session.done.borrow()
                    || session
                        .projection
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .cleanup_failed
            })
            .map(|(id, _)| id.clone())
            .collect()
    }
}

#[cfg(test)]
mod projection_tests {
    use super::*;
    use norishell_desktop_protocol::{DesktopRect, FramePatch};

    fn session() -> Session {
        let profile: DesktopProfile = serde_json::from_value(serde_json::json!({
            "id": "p", "label": "p", "protocol": "rdp", "address": "a", "port": 3389,
            "username": "", "domain": "", "hostId": null, "gatewayHostId": null,
            "credentialRefId": null, "width": 8, "height": 8, "clipboardEnabled": false,
            "revision": "0",
        }))
        .unwrap();
        let summary = DesktopSessionSummary {
            id: "s".into(),
            width: 8,
            height: 8,
            audio_state: DesktopAudioState::Disabled,
            audio_muted: false,
            rdp_transport_actual: None,
            rdp_graphics_actual: None,
            profile,
            generation: WireSequence::new(1),
            revision: WireSequence::new(1),
            state: DesktopSessionState::Running,
            phase: "running".into(),
            failure: None,
            frame_sequence: WireSequence::new(0),
        };
        Session {
            projection: Mutex::new(Projection {
                summary,
                frame: None,
                frame_base_sequence: 0,
                frame_dirty: DirtyRegion::default(),
                frame_requires_full: true,
                cursor_sequence: 0,
                cursor: DesktopCursor::Default,
                clipboard: None,
                cleanup_failed: false,
            }),
            commands: mpsc::channel(1).0,
            stop: watch::channel(false).0,
            focus_epoch: watch::channel(0).0,
            audio_muted: watch::channel(AudioMuteState::default()).0,
            done: watch::channel(false).0,
            changes: watch::channel(0).0,
            transports: Mutex::new(Vec::new()),
        }
    }

    fn patch(width: u16, height: u16, rect: DesktopRect) -> FramePatch {
        FramePatch {
            width,
            height,
            rects: vec![(rect, vec![7; rect.area() as usize * 4])],
        }
    }

    const RECT: DesktopRect = DesktopRect {
        x: 1,
        y: 1,
        width: 2,
        height: 2,
    };

    #[test]
    fn frames_patches_and_cursor_update_projection_and_wake_waiters() {
        let session = session();
        let changes = session.changes.subscribe();
        session.event(EngineEvent::Frame(Arc::new(
            DesktopFrame::new(8, 8).unwrap(),
        )));
        {
            let mut projection = session.projection.lock().unwrap();
            assert_eq!(projection.summary.frame_sequence.get(), 1);
            assert!(projection.frame_requires_full);
            // Simulate a delivery, after which patches accumulate.
            projection.frame_base_sequence = 1;
            projection.frame_requires_full = false;
        }
        session.event(EngineEvent::FramePatch(patch(8, 8, RECT)));
        {
            let projection = session.projection.lock().unwrap();
            assert_eq!(projection.summary.frame_sequence.get(), 2);
            assert_eq!(projection.frame_dirty.rects(), &[RECT]);
            assert_eq!(
                projection.frame.as_ref().unwrap().read_rect(RECT),
                vec![7; 16]
            );
        }
        session.event(EngineEvent::Cursor(DesktopCursor::Hidden));
        {
            let projection = session.projection.lock().unwrap();
            assert_eq!(projection.cursor_sequence, 1);
            assert_eq!(projection.cursor, DesktopCursor::Hidden);
        }
        assert!(changes.has_changed().unwrap());
        assert_eq!(*session.changes.borrow(), 3);
    }

    #[test]
    fn patch_without_matching_base_fails_and_releases_the_frame() {
        let session = session();
        session.event(EngineEvent::FramePatch(patch(8, 8, RECT)));
        assert_eq!(session.summary().failure.as_deref(), Some("protocolError"));
        assert!(*session.stop.borrow());

        let session = super::projection_tests::session();
        session.event(EngineEvent::Frame(Arc::new(
            DesktopFrame::new(8, 8).unwrap(),
        )));
        session.event(EngineEvent::FramePatch(patch(4, 4, RECT)));
        let projection = session.projection.lock().unwrap();
        assert_eq!(projection.summary.failure.as_deref(), Some("protocolError"));
        assert!(projection.frame.is_none());
        assert!(*session.stop.borrow());
    }
}
