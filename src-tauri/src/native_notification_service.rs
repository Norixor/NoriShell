//! Background orchestration for long-running task notifications; reads completion metadata, never commands or terminal output.
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use norishell_core_api::{
    DesktopPreferences, DesktopSessionState, ForwardSessionState,
    NATIVE_TERMINAL_EVENT_SCHEMA_VERSION, NativeTerminalCommandCompletion,
    NativeTerminalSessionScope, NativeTerminalSettings, NativeTerminalSnapshotRequest, RequestId,
    RequestMeta, SshSessionCloseReason, SshSessionFailureCode, SshSessionFailureStage,
    SshSessionState, WireSequence,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    desktop_preferences::DesktopPreferencesService,
    native_notifications::{
        DispatchResult, NativeNotifications, NotificationError, NotificationPermission,
    },
    native_terminal::NativeTerminalService,
    sftp_session_service::{
        NotificationTransferKind, NotificationTransferState, NotificationTransferSummary,
    },
};

const CLICK_EVENT: &str = "native-terminal-notification-click";
const RESOURCE_CLICK_EVENT: &str = "native-resource-notification-click";
const CLICK_LIMIT: usize = 128;
const SEEN_LIMIT: usize = 256;
const RATE_WINDOW: Duration = Duration::from_secs(60);
const RATE_MIN_INTERVAL: Duration = Duration::from_secs(3);
const RATE_MAX: usize = 6;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
pub enum NotificationLocale {
    #[default]
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en")]
    En,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationDeliveryState {
    #[default]
    Idle,
    Accepted,
    PermissionDenied,
    Unavailable,
    Failed,
    Unknown,
    RateLimited,
    Suppressed,
}

/// Core-owned pause deadline used by the native tray. The deadline is
/// monotonic and is checked immediately before every platform dispatch, so it
/// remains effective while every WebView is hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotificationPauseStatus {
    Active,
    Paused { remaining: Duration },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationFailureCode {
    WindowNotAllowed,
    ServiceStopped,
    Busy,
    PermissionDenied,
    Unavailable,
    DispatchFailed,
    Timeout,
    RateLimited,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationCommandError {
    pub code: NotificationFailureCode,
    pub request_id: RequestId,
}

type CommandResult<T> = Result<T, NotificationCommandError>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationRequest {
    pub meta: RequestMeta,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationTestRequest {
    pub meta: RequestMeta,
    pub locale: NotificationLocale,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationContextRequest {
    pub meta: RequestMeta,
    pub locale: NotificationLocale,
    pub visible_scope: Option<NativeTerminalSessionScope>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPermissionSnapshot {
    pub permission: NotificationPermission,
    pub last_delivery: NotificationDeliveryState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalNotificationClick {
    pub event_id: String,
    pub scope: NativeTerminalSessionScope,
}

/// An opaque resource fence consumed by the desktop renderer after a native
/// notification click. The renderer may focus an existing resource only after
/// revalidating this exact generation/revision; it must never connect or
/// reconnect from a notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NativeResourceNotificationClick {
    SftpTransfer {
        event_id: String,
        transfer_id: String,
        state_revision: WireSequence,
    },
    Ssh {
        event_id: String,
        session_id: String,
        generation: WireSequence,
        state_revision: WireSequence,
    },
    Telnet {
        event_id: String,
        session_id: String,
        generation: WireSequence,
        state_revision: WireSequence,
    },
    Desktop {
        event_id: String,
        session_id: String,
        generation: WireSequence,
        revision: WireSequence,
    },
    Forward {
        event_id: String,
        session_id: String,
        generation: WireSequence,
        state_revision: WireSequence,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NotificationClick {
    Terminal(NativeTerminalNotificationClick),
    Resource(NativeResourceNotificationClick),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NotificationKind {
    CommandFinished,
    CommandFailed,
    TransferCompleted,
    TransferFailed,
    SshDisconnected,
    TelnetDisconnected,
    DesktopDisconnected,
    ForwardFailed,
}

impl NotificationKind {
    fn key(self) -> &'static str {
        match self {
            Self::CommandFinished => "commandFinished",
            Self::CommandFailed => "commandFailed",
            Self::TransferCompleted => "transferCompleted",
            Self::TransferFailed => "transferFailed",
            Self::SshDisconnected => "sshDisconnected",
            Self::TelnetDisconnected => "telnetDisconnected",
            Self::DesktopDisconnected => "desktopDisconnected",
            Self::ForwardFailed => "forwardFailed",
        }
    }

    fn preference_enabled(self, preferences: &DesktopPreferences) -> bool {
        match self {
            Self::CommandFinished | Self::CommandFailed => true,
            Self::TransferCompleted => preferences.notify_transfer_completed,
            Self::TransferFailed => preferences.notify_transfer_failed,
            Self::SshDisconnected
            | Self::TelnetDisconnected
            | Self::DesktopDisconnected
            | Self::ForwardFailed => preferences.notify_disconnected,
        }
    }
}

struct ClickRecord {
    id: String,
    // None denotes a test notification and never participates in resource navigation.
    target: Option<NotificationClick>,
    dispatched: bool,
    early_click: bool,
}

#[derive(Default)]
struct NotificationState {
    locale: NotificationLocale,
    stopped: bool,
    pause_until: Option<Instant>,
    seen: VecDeque<String>,
    clicks: VecDeque<ClickRecord>,
    attempts: VecDeque<Instant>,
    last_delivery: NotificationDeliveryState,
}

impl NotificationState {
    fn pause_status(&mut self, now: Instant) -> NotificationPauseStatus {
        let Some(deadline) = self.pause_until else {
            return NotificationPauseStatus::Active;
        };
        match deadline.checked_duration_since(now) {
            Some(remaining) if !remaining.is_zero() => {
                NotificationPauseStatus::Paused { remaining }
            }
            _ => {
                self.pause_until = None;
                NotificationPauseStatus::Active
            }
        }
    }

    fn is_paused(&mut self, now: Instant) -> bool {
        matches!(
            self.pause_status(now),
            NotificationPauseStatus::Paused { .. }
        )
    }

    fn pause_for(
        &mut self,
        duration: Duration,
        now: Instant,
    ) -> Result<NotificationPauseStatus, NotificationFailureCode> {
        if self.stopped {
            return Err(NotificationFailureCode::ServiceStopped);
        }
        self.pause_until = if duration.is_zero() {
            None
        } else {
            Some(
                now.checked_add(duration)
                    .ok_or(NotificationFailureCode::Unavailable)?,
            )
        };
        Ok(self.pause_status(now))
    }

    fn resume(&mut self) -> Result<NotificationPauseStatus, NotificationFailureCode> {
        if self.stopped {
            return Err(NotificationFailureCode::ServiceStopped);
        }
        self.pause_until = None;
        Ok(NotificationPauseStatus::Active)
    }

    fn observe(&mut self, id: &str) -> bool {
        if self.seen.iter().any(|seen| seen == id) {
            return false;
        }
        if self.seen.len() == SEEN_LIMIT {
            self.seen.pop_front();
        }
        self.seen.push_back(id.to_owned());
        true
    }

    fn reserve(
        &mut self,
        id: String,
        target: Option<NotificationClick>,
        now: Instant,
    ) -> Result<(), NotificationFailureCode> {
        if self.stopped {
            return Err(NotificationFailureCode::ServiceStopped);
        }
        while self
            .attempts
            .front()
            .is_some_and(|time| now.duration_since(*time) >= RATE_WINDOW)
        {
            self.attempts.pop_front();
        }
        if self.attempts.len() >= RATE_MAX
            || self
                .attempts
                .back()
                .is_some_and(|time| now.duration_since(*time) < RATE_MIN_INTERVAL)
        {
            self.last_delivery = NotificationDeliveryState::RateLimited;
            return Err(NotificationFailureCode::RateLimited);
        }
        self.attempts.push_back(now);
        // Keep only the latest 128 navigation mappings; ignore older clicks without blocking later notifications.
        if self.clicks.len() == CLICK_LIMIT {
            self.clicks.pop_front();
        }
        self.clicks.push_back(ClickRecord {
            id,
            target,
            dispatched: false,
            early_click: false,
        });
        Ok(())
    }

    fn dispatched(&mut self, id: &str) -> Option<NotificationClick> {
        if self.stopped {
            return None;
        }
        let index = self.clicks.iter().position(|record| record.id == id)?;
        self.last_delivery = NotificationDeliveryState::Accepted;
        if self.clicks[index].target.is_none() {
            self.clicks.remove(index);
            return None;
        }
        let early_click = {
            let record = &mut self.clicks[index];
            record.dispatched = true;
            record.early_click
        };
        if early_click { self.clicked(id) } else { None }
    }

    fn failed(&mut self, id: &str, error: NotificationError) {
        self.clicks.retain(|record| record.id != id);
        self.last_delivery = delivery_error(error);
    }

    fn suppressed(&mut self, id: &str) {
        self.clicks.retain(|record| record.id != id);
        self.last_delivery = NotificationDeliveryState::Suppressed;
    }

    fn clicked(&mut self, id: &str) -> Option<NotificationClick> {
        if self.stopped {
            return None;
        }
        let index = self.clicks.iter().position(|record| record.id == id)?;
        if !self.clicks[index].dispatched {
            self.clicks[index].early_click = true;
            return None;
        }
        let record = self.clicks.remove(index)?;
        record.target
    }
}

#[derive(Clone)]
struct ResourceCandidate {
    event_key: String,
    kind: NotificationKind,
    target: NativeResourceNotificationClick,
}

struct LifecycleInput {
    observation_key: String,
    candidate: ResourceCandidate,
    running: bool,
    terminal: bool,
    unexpected: bool,
    requires_running: bool,
}

#[derive(Clone, Copy, Default)]
struct LifecycleObservation {
    ever_running: bool,
    terminal_seen: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TransferObservation {
    state: NotificationTransferState,
}

#[derive(Default)]
struct ResourceBaselines {
    ssh_initialized: bool,
    ssh: BTreeMap<String, LifecycleObservation>,
    telnet_initialized: bool,
    telnet: BTreeMap<String, LifecycleObservation>,
    desktop_initialized: bool,
    desktop: BTreeMap<String, LifecycleObservation>,
    forward_initialized: bool,
    forward: BTreeMap<String, LifecycleObservation>,
    transfers_initialized: bool,
    transfers: BTreeMap<String, TransferObservation>,
}

impl ResourceBaselines {
    fn observe_ssh(
        &mut self,
        sessions: Vec<norishell_core_api::SshSessionSummary>,
    ) -> Vec<ResourceCandidate> {
        let inputs = sessions.into_iter().map(|session| {
            let session_id = session.session_id.to_string();
            let generation = session.generation;
            let revision = session.state_revision;
            let unexpected = ssh_unexpected_disconnect(&session);
            LifecycleInput {
                observation_key: format!("{session_id}:{}", generation.get()),
                candidate: ResourceCandidate {
                    event_key: resource_event_key(
                        NotificationKind::SshDisconnected,
                        &session_id,
                        Some(generation),
                        revision,
                    ),
                    kind: NotificationKind::SshDisconnected,
                    target: NativeResourceNotificationClick::Ssh {
                        event_id: notification_event_id(),
                        session_id,
                        generation,
                        state_revision: revision,
                    },
                },
                running: session.state == SshSessionState::Running,
                terminal: matches!(
                    session.state,
                    SshSessionState::Closed | SshSessionState::Failed
                ),
                unexpected,
                requires_running: true,
            }
        });
        observe_lifecycle(&mut self.ssh_initialized, &mut self.ssh, inputs)
    }

    fn observe_telnet(
        &mut self,
        sessions: Vec<crate::telnet_session_service::TelnetSessionSummary>,
    ) -> Vec<ResourceCandidate> {
        let inputs = sessions.into_iter().map(|session| {
            let session_id = session.session_id.to_string();
            let generation = WireSequence::new(session.generation);
            let revision = WireSequence::new(session.state_revision);
            let unexpected = telnet_unexpected_disconnect(&session);
            LifecycleInput {
                observation_key: format!("{session_id}:{}", generation.get()),
                candidate: ResourceCandidate {
                    event_key: resource_event_key(
                        NotificationKind::TelnetDisconnected,
                        &session_id,
                        Some(generation),
                        revision,
                    ),
                    kind: NotificationKind::TelnetDisconnected,
                    target: NativeResourceNotificationClick::Telnet {
                        event_id: notification_event_id(),
                        session_id,
                        generation,
                        state_revision: revision,
                    },
                },
                running: session.state
                    == crate::telnet_session_service::TelnetSessionState::Running,
                terminal: matches!(
                    session.state,
                    crate::telnet_session_service::TelnetSessionState::Closed
                        | crate::telnet_session_service::TelnetSessionState::Failed
                ),
                unexpected,
                requires_running: true,
            }
        });
        observe_lifecycle(&mut self.telnet_initialized, &mut self.telnet, inputs)
    }

    fn observe_desktop(
        &mut self,
        sessions: Vec<norishell_core_api::DesktopSessionSummary>,
    ) -> Vec<ResourceCandidate> {
        let inputs = sessions.into_iter().map(|session| {
            let session_id = session.id.clone();
            let generation = session.generation;
            let revision = session.revision;
            let unexpected = session.state == DesktopSessionState::Failed;
            LifecycleInput {
                observation_key: format!("{session_id}:{}", generation.get()),
                candidate: ResourceCandidate {
                    event_key: resource_event_key(
                        NotificationKind::DesktopDisconnected,
                        &session_id,
                        Some(generation),
                        revision,
                    ),
                    kind: NotificationKind::DesktopDisconnected,
                    target: NativeResourceNotificationClick::Desktop {
                        event_id: notification_event_id(),
                        session_id,
                        generation,
                        revision,
                    },
                },
                running: session.state == DesktopSessionState::Running,
                terminal: matches!(
                    session.state,
                    DesktopSessionState::Closed | DesktopSessionState::Failed
                ),
                unexpected,
                requires_running: true,
            }
        });
        observe_lifecycle(&mut self.desktop_initialized, &mut self.desktop, inputs)
    }

    fn observe_forward(
        &mut self,
        sessions: Vec<norishell_core_api::ForwardSessionSummary>,
    ) -> Vec<ResourceCandidate> {
        let inputs = sessions.into_iter().map(|session| {
            let session_id = session.session_id.to_string();
            let generation = session.generation;
            let revision = session.state_revision;
            let unexpected =
                session.state == ForwardSessionState::Failed && session.failure.is_some();
            LifecycleInput {
                observation_key: format!("{session_id}:{}", generation.get()),
                candidate: ResourceCandidate {
                    event_key: resource_event_key(
                        NotificationKind::ForwardFailed,
                        &session_id,
                        Some(generation),
                        revision,
                    ),
                    kind: NotificationKind::ForwardFailed,
                    target: NativeResourceNotificationClick::Forward {
                        event_id: notification_event_id(),
                        session_id,
                        generation,
                        state_revision: revision,
                    },
                },
                running: session.state == ForwardSessionState::Running,
                terminal: matches!(
                    session.state,
                    ForwardSessionState::Failed | ForwardSessionState::Stopped
                ),
                unexpected,
                // Forward setup can fail before it reaches Running. Its
                // explicit Core failure is still a useful disconnected fact.
                requires_running: false,
            }
        });
        observe_lifecycle(&mut self.forward_initialized, &mut self.forward, inputs)
    }

    fn observe_transfers(
        &mut self,
        summaries: Vec<NotificationTransferSummary>,
    ) -> Vec<ResourceCandidate> {
        let inputs = summaries.into_iter().map(|summary| {
            let source_kind = match summary.kind {
                NotificationTransferKind::Direct => "transfer",
                NotificationTransferKind::Intent => "transferIntent",
            };
            let transfer_id = summary.transfer_id.to_string();
            let generation = summary.generation;
            let revision = summary.state_revision;
            let kind = match summary.state {
                NotificationTransferState::Active => None,
                NotificationTransferState::Completed => Some(NotificationKind::TransferCompleted),
                NotificationTransferState::Failed => Some(NotificationKind::TransferFailed),
            };
            let candidate = kind.map(|kind| ResourceCandidate {
                event_key: format!(
                    "{source_kind}:{}:{transfer_id}:{}:{}",
                    kind.key(),
                    generation.map_or_else(|| "none".to_owned(), |value| value.get().to_string()),
                    revision.get(),
                ),
                kind,
                target: NativeResourceNotificationClick::SftpTransfer {
                    event_id: notification_event_id(),
                    transfer_id: transfer_id.clone(),
                    state_revision: revision,
                },
            });
            (
                format!(
                    "{source_kind}:{transfer_id}:{}",
                    generation.map_or_else(|| "none".to_owned(), |value| value.get().to_string())
                ),
                TransferObservation {
                    state: summary.state,
                },
                candidate,
            )
        });
        observe_transfers(&mut self.transfers_initialized, &mut self.transfers, inputs)
    }
}

fn observe_lifecycle(
    initialized: &mut bool,
    observations: &mut BTreeMap<String, LifecycleObservation>,
    inputs: impl IntoIterator<Item = LifecycleInput>,
) -> Vec<ResourceCandidate> {
    let inputs = inputs.into_iter().collect::<Vec<_>>();
    if !*initialized {
        for input in inputs {
            observations.insert(
                input.observation_key,
                LifecycleObservation {
                    ever_running: input.running,
                    terminal_seen: input.terminal,
                },
            );
        }
        *initialized = true;
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for input in inputs {
        let observation = observations.entry(input.observation_key).or_default();
        observation.ever_running |= input.running;
        if input.terminal {
            let eligible = (!input.requires_running || observation.ever_running)
                && !observation.terminal_seen
                && input.unexpected;
            observation.terminal_seen = true;
            if eligible {
                candidates.push(input.candidate);
            }
        }
    }
    candidates
}

fn observe_transfers(
    initialized: &mut bool,
    observations: &mut BTreeMap<String, TransferObservation>,
    inputs: impl IntoIterator<Item = (String, TransferObservation, Option<ResourceCandidate>)>,
) -> Vec<ResourceCandidate> {
    let inputs = inputs.into_iter().collect::<Vec<_>>();
    if !*initialized {
        for (key, observation, _) in inputs {
            observations.insert(key, observation);
        }
        *initialized = true;
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for (key, observation, candidate) in inputs {
        let changed = observations.get(&key) != Some(&observation);
        observations.insert(key, observation);
        if changed && let Some(candidate) = candidate {
            candidates.push(candidate);
        }
    }
    candidates
}

fn ssh_unexpected_disconnect(session: &norishell_core_api::SshSessionSummary) -> bool {
    match session.state {
        SshSessionState::Closed => matches!(
            session.close_reason.as_ref(),
            Some(SshSessionCloseReason::RemoteEof)
        ),
        SshSessionState::Failed => session.failure_reason.as_ref().is_some_and(|failure| {
            failure.stage == SshSessionFailureStage::Running
                && failure.code == SshSessionFailureCode::ConnectionLost
        }),
        _ => false,
    }
}

fn telnet_unexpected_disconnect(
    session: &crate::telnet_session_service::TelnetSessionSummary,
) -> bool {
    match session.state {
        crate::telnet_session_service::TelnetSessionState::Closed => matches!(
            session.close_reason,
            Some(crate::telnet_session_service::TelnetCloseReason::RemoteClosed)
        ),
        crate::telnet_session_service::TelnetSessionState::Failed => true,
        _ => false,
    }
}

fn notification_event_id() -> String {
    format!("notification-{}", Uuid::new_v4())
}

fn resource_event_key(
    kind: NotificationKind,
    id: &str,
    generation: Option<WireSequence>,
    revision: WireSequence,
) -> String {
    format!(
        "{}:{id}:{}:{}",
        kind.key(),
        generation.map_or_else(|| "none".to_owned(), |value| value.get().to_string()),
        revision.get(),
    )
}

struct Inner {
    app: AppHandle,
    terminal: NativeTerminalService,
    adapter: NativeNotifications,
    state: Arc<Mutex<NotificationState>>,
    explicit_gate: tokio::sync::Mutex<()>,
    stop_tx: watch::Sender<bool>,
    worker: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct NativeNotificationService(Arc<Inner>);

impl NativeNotificationService {
    /// Call only on the setup main thread; starting the service neither requests permission nor sends a test notification.
    pub fn start(app: AppHandle, terminal: NativeTerminalService) -> Self {
        let state = Arc::new(Mutex::new(NotificationState::default()));
        let click_state = Arc::downgrade(&state);
        let click_app = app.clone();
        let adapter = NativeNotifications::new(app.clone(), move |id| {
            let click = click_state
                .upgrade()
                .and_then(|state| state.lock().ok().and_then(|mut state| state.clicked(&id)));
            if let Some(click) = click
                && let Some(state) = click_state.upgrade()
            {
                emit_click(&click_app, &state, click);
            }
        });
        let (stop_tx, mut stop_rx) = watch::channel(false);
        // Start an independent cursor at the current facts; service startup or reconstruction must not resend old events.
        let mut cursor = completion_snapshot(&terminal, None).completion_cursor;
        let inner = Arc::new(Inner {
            app,
            terminal,
            adapter,
            state,
            explicit_gate: tokio::sync::Mutex::new(()),
            stop_tx,
            worker: Mutex::new(None),
        });
        let weak = Arc::downgrade(&inner);
        let worker = tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut resources = ResourceBaselines::default();
            loop {
                tokio::select! {
                    biased;
                    _ = stop_rx.changed() => break,
                    _ = interval.tick() => {}
                }
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                tokio::select! {
                    biased;
                    _ = stop_rx.changed() => break,
                    _ = inner.poll(&mut cursor, &mut resources) => {}
                }
            }
        });
        *inner
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(worker);
        Self(inner)
    }

    /// Call on actual exit. Stop new dispatches and revoke click permissions; notifications already handed to the OS cannot be recalled.
    pub fn stop(&self) {
        {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.stopped = true;
            state.clicks.clear();
        }
        let _ = self.0.stop_tx.send(true);
        let worker = self
            .0
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(worker) = worker {
            // The Exit callback must not wait for window queries or notification callbacks that may be queued on the main thread.
            worker.abort();
        }
    }

    /// Snapshot the Core-owned pause deadline for native menu rendering.
    pub(crate) fn pause_status(&self) -> Result<NotificationPauseStatus, NotificationFailureCode> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| NotificationFailureCode::Unavailable)?;
        if state.stopped {
            return Err(NotificationFailureCode::ServiceStopped);
        }
        Ok(state.pause_status(Instant::now()))
    }

    /// Pausing never queues pending events: resource and completion cursors
    /// continue to advance, and the final platform-dispatch guard checks this
    /// deadline again immediately before delivery.
    pub(crate) fn pause_for(
        &self,
        duration: Duration,
    ) -> Result<NotificationPauseStatus, NotificationFailureCode> {
        self.0
            .state
            .lock()
            .map_err(|_| NotificationFailureCode::Unavailable)?
            .pause_for(duration, Instant::now())
    }

    pub(crate) fn resume(&self) -> Result<NotificationPauseStatus, NotificationFailureCode> {
        self.0
            .state
            .lock()
            .map_err(|_| NotificationFailureCode::Unavailable)?
            .resume()
    }

    fn ensure_running(&self) -> Result<(), NotificationFailureCode> {
        if self
            .0
            .state
            .lock()
            .map_err(|_| NotificationFailureCode::Unavailable)?
            .stopped
        {
            Err(NotificationFailureCode::ServiceStopped)
        } else {
            Ok(())
        }
    }

    async fn permission(&self) -> Result<NotificationPermissionSnapshot, NotificationFailureCode> {
        self.ensure_running()?;
        let permission = self.0.adapter.permission().await.map_err(failure_code)?;
        self.ensure_running()?;
        let last_delivery = self
            .0
            .state
            .lock()
            .map_err(|_| NotificationFailureCode::Unavailable)?
            .last_delivery;
        Ok(NotificationPermissionSnapshot {
            permission,
            last_delivery,
        })
    }

    async fn request_permission(
        &self,
    ) -> Result<NotificationPermissionSnapshot, NotificationFailureCode> {
        self.ensure_running()?;
        let _guard = self
            .0
            .explicit_gate
            .try_lock()
            .map_err(|_| NotificationFailureCode::Busy)?;
        let permission = self
            .0
            .adapter
            .request_permission()
            .await
            .map_err(failure_code)?;
        self.ensure_running()?;
        let last_delivery = self
            .0
            .state
            .lock()
            .map_err(|_| NotificationFailureCode::Unavailable)?
            .last_delivery;
        Ok(NotificationPermissionSnapshot {
            permission,
            last_delivery,
        })
    }

    async fn test(
        &self,
        locale: NotificationLocale,
    ) -> Result<NotificationPermissionSnapshot, NotificationFailureCode> {
        self.ensure_running()?;
        let _guard = self
            .0
            .explicit_gate
            .try_lock()
            .map_err(|_| NotificationFailureCode::Busy)?;
        let permission = self.0.adapter.permission().await.map_err(failure_code)?;
        match permission {
            NotificationPermission::Granted => {}
            NotificationPermission::Unavailable => {
                return Err(NotificationFailureCode::Unavailable);
            }
            NotificationPermission::NotDetermined | NotificationPermission::Denied => {
                return Err(NotificationFailureCode::PermissionDenied);
            }
        }
        {
            let mut state = self
                .0
                .state
                .lock()
                .map_err(|_| NotificationFailureCode::Unavailable)?;
            if state.is_paused(Instant::now()) {
                state.last_delivery = NotificationDeliveryState::Suppressed;
                return Ok(NotificationPermissionSnapshot {
                    permission,
                    last_delivery: NotificationDeliveryState::Suppressed,
                });
            }
        }
        let id = notification_event_id();
        self.0
            .state
            .lock()
            .map_err(|_| NotificationFailureCode::Unavailable)?
            .reserve(id.clone(), None, Instant::now())?;
        let (title, body) = notification_test_text(locale);
        let state = self.0.state.clone();
        match self
            .0
            .adapter
            .send_if(&id, title, body, move || {
                state
                    .lock()
                    .is_ok_and(|mut state| !state.stopped && !state.is_paused(Instant::now()))
            })
            .await
        {
            Ok(DispatchResult::Accepted) => {
                self.0
                    .state
                    .lock()
                    .map_err(|_| NotificationFailureCode::Unavailable)?
                    .dispatched(&id);
            }
            Ok(DispatchResult::Suppressed) => {
                self.0
                    .state
                    .lock()
                    .map_err(|_| NotificationFailureCode::Unavailable)?
                    .suppressed(&id);
            }
            Err(error) => {
                self.0
                    .state
                    .lock()
                    .map_err(|_| NotificationFailureCode::Unavailable)?
                    .failed(&id, error);
                return Err(failure_code(error));
            }
        }
        self.ensure_running()?;
        let last_delivery = self
            .0
            .state
            .lock()
            .map_err(|_| NotificationFailureCode::Unavailable)?
            .last_delivery;
        Ok(NotificationPermissionSnapshot {
            permission,
            last_delivery,
        })
    }
}

impl Inner {
    async fn poll(&self, cursor: &mut WireSequence, resources: &mut ResourceBaselines) {
        self.poll_terminal(cursor).await;
        self.poll_resources(resources).await;
    }

    async fn poll_terminal(&self, cursor: &mut WireSequence) {
        let snapshot = completion_snapshot(&self.terminal, Some(*cursor));
        *cursor = snapshot.completion_cursor;
        if snapshot.schema_version != NATIVE_TERMINAL_EVENT_SCHEMA_VERSION {
            return;
        }
        for completion in snapshot.completions {
            let kind = if completion.exit_code.is_some_and(|code| code != 0) {
                NotificationKind::CommandFailed
            } else {
                NotificationKind::CommandFinished
            };
            let event_key = resource_event_key(
                kind,
                completion.event_id.as_str(),
                Some(scope_generation(&completion.session)),
                completion.cursor,
            );
            {
                let Ok(mut state) = self.state.lock() else {
                    return;
                };
                if state.stopped {
                    return;
                }
                // Completion facts advance regardless of the current settings,
                // rate limit, or pause. Re-enabling later never backfills them.
                if !state.observe(&event_key) {
                    continue;
                }
            }
            let Some(preferences) = desktop_preferences(&self.app) else {
                continue;
            };
            let settings = self.terminal.settings_snapshot().settings;
            let focused = read_application_focus(&self.app).await;
            let (id, locale) = {
                let Ok(mut state) = self.state.lock() else {
                    return;
                };
                if state.stopped {
                    return;
                }
                if state.is_paused(Instant::now()) {
                    state.last_delivery = NotificationDeliveryState::Suppressed;
                    continue;
                }
                if !command_eligible(&completion, &settings, &preferences, focused) {
                    continue;
                }
                let id = notification_event_id();
                let target = NotificationClick::Terminal(NativeTerminalNotificationClick {
                    event_id: completion.event_id.to_string(),
                    scope: completion.session.clone(),
                });
                if state
                    .reserve(id.clone(), Some(target), Instant::now())
                    .is_err()
                {
                    continue;
                }
                (id, state.locale)
            };
            let (title, body) = notification_text(locale, kind);
            let terminal = self.terminal.clone();
            let app = self.app.clone();
            let state = self.state.clone();
            let guarded_completion = completion.clone();
            let result =
                self.adapter
                    .send_if(&id, title, body, move || {
                        if !state.lock().is_ok_and(|mut state| {
                            !state.stopped && !state.is_paused(Instant::now())
                        }) {
                            return false;
                        }
                        let settings = terminal.settings_snapshot().settings;
                        desktop_preferences(&app).is_some_and(|preferences| {
                            command_eligible(
                                &guarded_completion,
                                &settings,
                                &preferences,
                                application_is_focused(&app),
                            )
                        })
                    })
                    .await;
            if let Some(click) = self.finish_dispatch(&id, result) {
                emit_click(&self.app, &self.state, click);
            }
        }
    }

    async fn poll_resources(&self, baselines: &mut ResourceBaselines) {
        let mut candidates = Vec::new();
        if let Some(service) = self
            .app
            .try_state::<crate::ssh_session_service::SshSessionService>()
            .map(|service| service.inner().clone())
            && let Ok(snapshot) = service.snapshot(RequestId::new()).await
        {
            candidates.extend(baselines.observe_ssh(snapshot.sessions));
        }
        if let Some(service) = self
            .app
            .try_state::<crate::telnet_session_service::TelnetSessionService>()
            .map(|service| service.inner().clone())
            && let Ok(sessions) = service.snapshot().await
        {
            candidates.extend(baselines.observe_telnet(sessions));
        }
        if let Some(service) = self
            .app
            .try_state::<crate::desktop_service::DesktopService>()
        {
            candidates.extend(baselines.observe_desktop(service.snapshot()));
        }
        if let Some(service) = self
            .app
            .try_state::<crate::forward_session_service::ForwardSessionService>()
            && let Ok(sessions) = service.wire_summaries()
        {
            candidates.extend(baselines.observe_forward(sessions));
        }
        if let Some(service) = self
            .app
            .try_state::<crate::sftp_session_service::SftpSessionService>()
            .map(|service| service.inner().clone())
            && let Ok(summaries) = service.notification_transfer_snapshot().await
        {
            candidates.extend(baselines.observe_transfers(summaries));
        }
        for candidate in candidates {
            self.dispatch_resource(candidate).await;
        }
    }

    async fn dispatch_resource(&self, candidate: ResourceCandidate) {
        {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if state.stopped || !state.observe(&candidate.event_key) {
                return;
            }
            if state.is_paused(Instant::now()) {
                state.last_delivery = NotificationDeliveryState::Suppressed;
                return;
            }
        }
        let Some(preferences) = desktop_preferences(&self.app) else {
            return;
        };
        let focused = read_application_focus(&self.app).await;
        if !resource_eligible(candidate.kind, &preferences, focused) {
            return;
        }
        let (id, locale) = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if state.stopped {
                return;
            }
            if state.is_paused(Instant::now()) {
                state.last_delivery = NotificationDeliveryState::Suppressed;
                return;
            }
            let id = notification_event_id();
            if state
                .reserve(
                    id.clone(),
                    Some(NotificationClick::Resource(candidate.target.clone())),
                    Instant::now(),
                )
                .is_err()
            {
                return;
            }
            (id, state.locale)
        };
        let (title, body) = notification_text(locale, candidate.kind);
        let app = self.app.clone();
        let state = self.state.clone();
        let kind = candidate.kind;
        let result = self
            .adapter
            .send_if(&id, title, body, move || {
                if !state
                    .lock()
                    .is_ok_and(|mut state| !state.stopped && !state.is_paused(Instant::now()))
                {
                    return false;
                }
                desktop_preferences(&app).is_some_and(|preferences| {
                    resource_eligible(kind, &preferences, application_is_focused(&app))
                })
            })
            .await;
        if let Some(click) = self.finish_dispatch(&id, result) {
            emit_click(&self.app, &self.state, click);
        }
    }

    fn finish_dispatch(
        &self,
        id: &str,
        result: Result<DispatchResult, NotificationError>,
    ) -> Option<NotificationClick> {
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        match result {
            Ok(DispatchResult::Accepted) => state.dispatched(id),
            Ok(DispatchResult::Suppressed) => {
                state.suppressed(id);
                None
            }
            Err(error) => {
                state.failed(id, error);
                None
            }
        }
    }
}

fn completion_snapshot(
    terminal: &NativeTerminalService,
    cursor: Option<WireSequence>,
) -> norishell_core_api::NativeTerminalSnapshot {
    terminal.snapshot(NativeTerminalSnapshotRequest {
        meta: RequestMeta {
            request_id: RequestId::new(),
        },
        after_completion_cursor: cursor,
    })
}

fn command_eligible(
    completion: &NativeTerminalCommandCompletion,
    settings: &NativeTerminalSettings,
    preferences: &DesktopPreferences,
    focused: bool,
) -> bool {
    settings.notifications_enabled
        && completion.elapsed_millis
            >= u64::from(settings.notification_threshold_seconds).saturating_mul(1_000)
        && (!preferences.notification_failure_only
            || completion.exit_code.is_some_and(|code| code != 0))
        && background_eligible(preferences, focused)
}

fn resource_eligible(
    kind: NotificationKind,
    preferences: &DesktopPreferences,
    focused: bool,
) -> bool {
    kind.preference_enabled(preferences) && background_eligible(preferences, focused)
}

fn background_eligible(preferences: &DesktopPreferences, application_focused: bool) -> bool {
    !preferences.notification_background_only || !application_focused
}

fn application_is_focused(app: &AppHandle) -> bool {
    // `main` is not the only focusable WebView. Focus in a protected window also means the user is
    // using the application; a hidden main window alone must not trigger a notification.
    app.webview_windows()
        .values()
        .any(|window| window.is_focused().unwrap_or(false))
}

async fn read_application_focus(app: &AppHandle) -> bool {
    // Never synchronously wait for the UI thread from a runtime worker; exit can cancel the query without waiting on the main thread.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let app_for_callback = app.clone();
    if app
        .run_on_main_thread(move || {
            let _ = tx.send(application_is_focused(&app_for_callback));
        })
        .is_err()
    {
        return false;
    }
    tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .is_ok_and(|result| result.unwrap_or(false))
}

fn emit_click(app: &AppHandle, state: &Arc<Mutex<NotificationState>>, click: NotificationClick) {
    let app_for_callback = app.clone();
    let state = Arc::downgrade(state);
    let _ = app.run_on_main_thread(move || {
        if !state
            .upgrade()
            .is_some_and(|state| state.lock().is_ok_and(|state| !state.stopped))
        {
            return;
        }
        if let Some(window) = app_for_callback.get_webview_window("main") {
            let _ = crate::window_first_show::show_if_revealed(&window);
            match click {
                NotificationClick::Terminal(click) => {
                    let _ = window.emit(CLICK_EVENT, click);
                }
                NotificationClick::Resource(click) => {
                    let _ = window.emit(RESOURCE_CLICK_EVENT, click);
                }
            }
        }
    });
}

fn notification_text(
    locale: NotificationLocale,
    kind: NotificationKind,
) -> (&'static str, &'static str) {
    match (locale, kind) {
        (NotificationLocale::ZhCn, NotificationKind::CommandFinished) => (
            "NoriShell · 任务已结束",
            "一个长时间运行的终端任务已结束。点击返回对应终端查看结果。",
        ),
        (NotificationLocale::En, NotificationKind::CommandFinished) => (
            "NoriShell · Task finished",
            "A long-running terminal task has finished. Click to return to its terminal and inspect the result.",
        ),
        (NotificationLocale::ZhCn, NotificationKind::CommandFailed) => (
            "NoriShell · 任务失败",
            "一个长时间运行的终端任务失败。点击返回对应终端查看结果。",
        ),
        (NotificationLocale::En, NotificationKind::CommandFailed) => (
            "NoriShell · Task failed",
            "A long-running terminal task failed. Click to return to its terminal and inspect the result.",
        ),
        (NotificationLocale::ZhCn, NotificationKind::TransferCompleted) => (
            "NoriShell · 文件传输已完成",
            "一项文件传输已完成。点击返回对应资源查看状态。",
        ),
        (NotificationLocale::En, NotificationKind::TransferCompleted) => (
            "NoriShell · File transfer completed",
            "A file transfer completed. Click to return to the related resource.",
        ),
        (NotificationLocale::ZhCn, NotificationKind::TransferFailed) => (
            "NoriShell · 文件传输失败",
            "一项文件传输失败。点击返回对应资源查看状态。",
        ),
        (NotificationLocale::En, NotificationKind::TransferFailed) => (
            "NoriShell · File transfer failed",
            "A file transfer failed. Click to return to the related resource.",
        ),
        (NotificationLocale::ZhCn, NotificationKind::SshDisconnected) => (
            "NoriShell · SSH 连接已断开",
            "一个远程连接意外断开。点击返回对应资源查看状态。",
        ),
        (NotificationLocale::En, NotificationKind::SshDisconnected) => (
            "NoriShell · SSH connection lost",
            "A remote connection ended unexpectedly. Click to return to the related resource.",
        ),
        (NotificationLocale::ZhCn, NotificationKind::TelnetDisconnected) => (
            "NoriShell · Telnet 连接已断开",
            "一个远程连接意外断开。点击返回对应资源查看状态。",
        ),
        (NotificationLocale::En, NotificationKind::TelnetDisconnected) => (
            "NoriShell · Telnet connection lost",
            "A remote connection ended unexpectedly. Click to return to the related resource.",
        ),
        (NotificationLocale::ZhCn, NotificationKind::DesktopDisconnected) => (
            "NoriShell · 远程桌面已断开",
            "一个远程桌面连接意外断开。点击返回对应资源查看状态。",
        ),
        (NotificationLocale::En, NotificationKind::DesktopDisconnected) => (
            "NoriShell · Remote desktop connection lost",
            "A remote desktop connection ended unexpectedly. Click to return to the related resource.",
        ),
        (NotificationLocale::ZhCn, NotificationKind::ForwardFailed) => (
            "NoriShell · 端口转发失败",
            "一项端口转发失败。点击返回对应资源查看状态。",
        ),
        (NotificationLocale::En, NotificationKind::ForwardFailed) => (
            "NoriShell · Port forwarding failed",
            "A port-forwarding session failed. Click to return to the related resource.",
        ),
    }
}

fn notification_test_text(locale: NotificationLocale) -> (&'static str, &'static str) {
    match locale {
        NotificationLocale::ZhCn => (
            "NoriShell · 测试通知",
            "这是一条测试通知，不对应任何终端任务。",
        ),
        NotificationLocale::En => (
            "NoriShell · Test notification",
            "This is a test notification. It is not associated with a terminal task.",
        ),
    }
}

fn desktop_preferences(app: &AppHandle) -> Option<DesktopPreferences> {
    app.try_state::<DesktopPreferencesService>()
        .and_then(|service| service.preferences().ok())
}

fn scope_generation(scope: &NativeTerminalSessionScope) -> WireSequence {
    match scope {
        NativeTerminalSessionScope::Ssh { generation, .. }
        | NativeTerminalSessionScope::Local { generation, .. } => *generation,
    }
}

fn failure_code(error: NotificationError) -> NotificationFailureCode {
    match error {
        NotificationError::PermissionDenied => NotificationFailureCode::PermissionDenied,
        NotificationError::Unavailable => NotificationFailureCode::Unavailable,
        NotificationError::Timeout => NotificationFailureCode::Timeout,
        NotificationError::DispatchFailed | NotificationError::InvalidContent => {
            NotificationFailureCode::DispatchFailed
        }
    }
}

fn delivery_error(error: NotificationError) -> NotificationDeliveryState {
    match error {
        NotificationError::PermissionDenied => NotificationDeliveryState::PermissionDenied,
        NotificationError::Unavailable => NotificationDeliveryState::Unavailable,
        NotificationError::Timeout => NotificationDeliveryState::Unknown,
        NotificationError::DispatchFailed | NotificationError::InvalidContent => {
            NotificationDeliveryState::Failed
        }
    }
}

fn require_main<R: tauri::Runtime>(
    window: &WebviewWindow<R>,
    request_id: &RequestId,
) -> CommandResult<()> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err(command_error(
            request_id,
            NotificationFailureCode::WindowNotAllowed,
        ))
    }
}

fn command_error(
    request_id: &RequestId,
    code: NotificationFailureCode,
) -> NotificationCommandError {
    NotificationCommandError {
        code,
        request_id: request_id.clone(),
    }
}

#[tauri::command]
pub async fn native_notification_permission_get<R: tauri::Runtime>(
    request: NotificationRequest,
    window: WebviewWindow<R>,
    service: State<'_, NativeNotificationService>,
) -> CommandResult<NotificationPermissionSnapshot> {
    require_main(&window, &request.meta.request_id)?;
    service
        .permission()
        .await
        .map_err(|code| command_error(&request.meta.request_id, code))
}

#[tauri::command]
pub async fn native_notification_permission_request<R: tauri::Runtime>(
    request: NotificationRequest,
    window: WebviewWindow<R>,
    service: State<'_, NativeNotificationService>,
) -> CommandResult<NotificationPermissionSnapshot> {
    require_main(&window, &request.meta.request_id)?;
    service
        .request_permission()
        .await
        .map_err(|code| command_error(&request.meta.request_id, code))
}

#[tauri::command]
pub async fn native_notification_test<R: tauri::Runtime>(
    request: NotificationTestRequest,
    window: WebviewWindow<R>,
    service: State<'_, NativeNotificationService>,
) -> CommandResult<NotificationPermissionSnapshot> {
    require_main(&window, &request.meta.request_id)?;
    service
        .test(request.locale)
        .await
        .map_err(|code| command_error(&request.meta.request_id, code))
}

#[tauri::command]
pub fn native_notification_context_set<R: tauri::Runtime>(
    request: NotificationContextRequest,
    window: WebviewWindow<R>,
    service: State<'_, NativeNotificationService>,
) -> CommandResult<()> {
    require_main(&window, &request.meta.request_id)?;
    service
        .ensure_running()
        .map_err(|code| command_error(&request.meta.request_id, code))?;
    let mut state = service.0.state.lock().map_err(|_| {
        command_error(
            &request.meta.request_id,
            NotificationFailureCode::Unavailable,
        )
    })?;
    state.locale = request.locale;
    // The renderer still sends its visible scope for protocol compatibility,
    // but foreground suppression is intentionally based on every application
    // window, including protected windows, rather than on one terminal pane.
    let _ = request.visible_scope;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_core_api::{
        LocalPtyId, LocalSessionId, LocalViewId, NativeTerminalEventId, TransferId,
    };

    fn scope() -> NativeTerminalSessionScope {
        NativeTerminalSessionScope::Local {
            session_id: LocalSessionId::new(),
            generation: WireSequence::new(1),
            pty_id: LocalPtyId::new(),
            pane_id: LocalViewId::new(),
        }
    }

    fn completion(
        scope: NativeTerminalSessionScope,
        elapsed_millis: u64,
    ) -> NativeTerminalCommandCompletion {
        NativeTerminalCommandCompletion {
            cursor: WireSequence::new(1),
            event_id: NativeTerminalEventId::new(),
            session: scope,
            elapsed_millis,
            exit_code: Some(0),
            completed_at_unix_ms: 1,
        }
    }

    #[test]
    fn command_eligibility_uses_whole_application_focus_and_known_failures_only() {
        let mut settings = NativeTerminalSettings {
            notifications_enabled: true,
            ..Default::default()
        };
        let scope = scope();
        let elapsed = u64::from(settings.notification_threshold_seconds) * 1_000;
        let mut event = completion(scope.clone(), elapsed);
        let mut preferences = DesktopPreferences::default();
        assert!(command_eligible(&event, &settings, &preferences, false));
        assert!(!command_eligible(&event, &settings, &preferences, true));
        preferences.notification_background_only = false;
        assert!(command_eligible(&event, &settings, &preferences, true));
        preferences.notification_failure_only = true;
        event.exit_code = None;
        assert!(!command_eligible(&event, &settings, &preferences, false));
        event.exit_code = Some(0);
        assert!(!command_eligible(&event, &settings, &preferences, false));
        event.exit_code = Some(-1);
        assert!(command_eligible(&event, &settings, &preferences, false));
        event.elapsed_millis = elapsed.saturating_sub(1);
        assert!(!command_eligible(&event, &settings, &preferences, false));
        settings.notifications_enabled = false;
        assert!(!command_eligible(&event, &settings, &preferences, false));
    }

    #[test]
    fn clicks_require_successful_dispatch_and_are_consumed_once() {
        let mut state = NotificationState::default();
        let target = scope();
        let target = NativeTerminalNotificationClick {
            event_id: "completion".to_owned(),
            scope: target,
        };
        state
            .reserve(
                "event".into(),
                Some(NotificationClick::Terminal(target.clone())),
                Instant::now(),
            )
            .unwrap();
        assert!(state.clicked("foreign").is_none());
        assert!(state.clicked("event").is_none());
        let click = state.dispatched("event").unwrap();
        assert_eq!(click, NotificationClick::Terminal(target));
        assert!(state.clicked("event").is_none());
    }

    #[test]
    fn pause_uses_a_monotonic_deadline_and_suppression_is_not_service_stopped() {
        let mut state = NotificationState::default();
        let now = Instant::now();
        assert_eq!(
            state.pause_for(Duration::from_secs(30 * 60), now),
            Ok(NotificationPauseStatus::Paused {
                remaining: Duration::from_secs(30 * 60)
            })
        );
        assert!(state.is_paused(now + Duration::from_secs(29 * 60)));
        state.pause_until = Some(now - Duration::from_secs(1));
        assert_eq!(state.pause_status(now), NotificationPauseStatus::Active);
        state
            .reserve("test".into(), None, now + Duration::from_secs(8))
            .unwrap();
        state.suppressed("test");
        assert_eq!(state.last_delivery, NotificationDeliveryState::Suppressed);
        assert!(!state.stopped);
        assert!(state.clicked("test").is_none());
        assert!(state.clicks.is_empty());
    }

    #[test]
    fn initial_resource_snapshot_and_disappearance_do_not_backfill_or_infer_disconnects() {
        let target = NativeResourceNotificationClick::Ssh {
            event_id: "event".to_owned(),
            session_id: "session".to_owned(),
            generation: WireSequence::new(1),
            state_revision: WireSequence::new(2),
        };
        let input = || LifecycleInput {
            observation_key: "session:1".to_owned(),
            candidate: ResourceCandidate {
                event_key: "sshDisconnected:session:1:2".to_owned(),
                kind: NotificationKind::SshDisconnected,
                target: target.clone(),
            },
            running: true,
            terminal: false,
            unexpected: false,
            requires_running: true,
        };
        let mut initialized = false;
        let mut observations = BTreeMap::new();
        assert!(observe_lifecycle(&mut initialized, &mut observations, [input()]).is_empty());
        assert!(observe_lifecycle(&mut initialized, &mut observations, []).is_empty());
        let mut terminal = input();
        terminal.running = false;
        terminal.terminal = true;
        terminal.unexpected = true;
        assert_eq!(
            observe_lifecycle(&mut initialized, &mut observations, [terminal]).len(),
            1
        );
    }

    #[test]
    fn fast_terminal_transfers_are_observed_once_after_the_initial_baseline() {
        let transfer_id = TransferId::new();
        let mut baselines = ResourceBaselines::default();
        let summary = |state, revision| NotificationTransferSummary {
            kind: NotificationTransferKind::Direct,
            transfer_id: transfer_id.clone(),
            generation: Some(WireSequence::new(3)),
            state_revision: WireSequence::new(revision),
            state,
        };
        assert!(
            baselines
                .observe_transfers(vec![summary(NotificationTransferState::Active, 1)])
                .is_empty()
        );
        let completed =
            baselines.observe_transfers(vec![summary(NotificationTransferState::Completed, 2)]);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].kind, NotificationKind::TransferCompleted);
        assert!(
            baselines
                .observe_transfers(vec![summary(NotificationTransferState::Completed, 3)])
                .is_empty()
        );
        assert!(
            baselines
                .observe_transfers(vec![summary(NotificationTransferState::Active, 4)])
                .is_empty()
        );
        assert_eq!(
            baselines
                .observe_transfers(vec![summary(NotificationTransferState::Completed, 5)])
                .len(),
            1
        );
    }

    #[test]
    fn stopped_service_rejects_new_notifications_and_late_clicks() {
        let mut state = NotificationState::default();
        let now = Instant::now();
        state.reserve("event".into(), None, now).unwrap();
        state.stopped = true;
        assert_eq!(
            state.reserve("new".into(), None, now),
            Err(NotificationFailureCode::ServiceStopped)
        );
        assert!(state.dispatched("event").is_none());
        assert!(state.clicked("event").is_none());
    }

    #[test]
    fn duplicates_rate_and_click_maps_remain_bounded() {
        let mut state = NotificationState::default();
        assert!(state.observe("same"));
        assert!(!state.observe("same"));
        for index in 0..1_000 {
            state.observe(&format!("event-{index}"));
        }
        assert_eq!(state.seen.len(), SEEN_LIMIT);
        let now = Instant::now();
        state.reserve("first".into(), None, now).unwrap();
        assert_eq!(
            state.reserve("burst".into(), None, now),
            Err(NotificationFailureCode::RateLimited)
        );
        for index in 1..CLICK_LIMIT {
            state
                .reserve(
                    format!("event-{index}"),
                    None,
                    now + Duration::from_secs(index as u64 * 61),
                )
                .unwrap();
        }
        assert_eq!(state.clicks.len(), CLICK_LIMIT);
        assert!(
            state
                .reserve("overflow".into(), None, now + Duration::from_secs(10_000))
                .is_ok()
        );
        assert_eq!(state.clicks.len(), CLICK_LIMIT);
        assert!(state.clicked("first").is_none());
    }

    #[test]
    fn minute_rate_limit_expires_without_replaying_dropped_events() {
        let mut state = NotificationState::default();
        let now = Instant::now();
        for index in 0..RATE_MAX {
            state
                .reserve(
                    format!("event-{index}"),
                    None,
                    now + Duration::from_secs(index as u64 * 4),
                )
                .unwrap();
        }
        assert_eq!(
            state.reserve("overflow".into(), None, now + Duration::from_secs(30)),
            Err(NotificationFailureCode::RateLimited)
        );
        assert!(
            state
                .reserve("fresh".into(), None, now + Duration::from_secs(60))
                .is_ok()
        );
        assert!(state.clicks.iter().all(|entry| entry.id != "overflow"));
    }

    #[test]
    fn ipc_rejects_caller_notification_text_and_renderer_focus_claims() {
        let meta = serde_json::json!({ "requestId": RequestId::new() });
        assert!(
            serde_json::from_value::<NotificationTestRequest>(
                serde_json::json!({ "meta": meta, "locale": "en", "body": "secret" })
            )
            .is_err()
        );
        assert!(serde_json::from_value::<NotificationContextRequest>(serde_json::json!({ "meta": meta, "locale": "en", "visibleScope": null, "focused": true })).is_err());
        for locale in [NotificationLocale::En, NotificationLocale::ZhCn] {
            assert_ne!(
                notification_text(locale, NotificationKind::CommandFinished),
                notification_test_text(locale)
            );
        }
    }

    #[test]
    fn resource_click_payload_contains_only_opaque_resource_fences() {
        let click = NativeResourceNotificationClick::SftpTransfer {
            event_id: "event".to_owned(),
            transfer_id: "transfer".to_owned(),
            state_revision: WireSequence::new(7),
        };
        let value = serde_json::to_value(click).unwrap();
        assert_eq!(value["kind"], "sftpTransfer");
        assert!(value.get("path").is_none());
        assert!(value.get("endpoint").is_none());
        assert!(value.get("displayName").is_none());
    }
}
