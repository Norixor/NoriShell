//! Core-owned RDP engine. Consumes Core's TCP transport and may open reliable UDP only to that transport's actual peer.
//! The engine does no DNS or KDC lookup and never bypasses an SSH gateway.
//! TLS signature verification and one-shot approval of unknown certificates precede transmission of credentials.
mod audio;
mod clipboard;
mod limits;
mod tls;
mod udp;
mod wire;
use ironrdp::{
    connector::{self, ClientConnector, ClientConnectorState, Credentials},
    core::WriteBuf,
    graphics::image_processing::PixelFormat,
    graphics::pointer::DecodedPointer,
    input::{Database, MouseButton, MousePosition, Operation, Scancode, WheelRotations},
    session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput, image::DecodedImage},
};
use norishell_desktop_protocol::{
    AudioMuteState, BoxedDesktopIo, CursorBitmap, DesktopCursor, DesktopFrame, DesktopInput,
    DesktopRect, DirtyRegion, EngineCommand, EngineControl, EngineError, EngineEvent, EventSink,
    FramePatch, PUBLISH_INTERVAL, RdpGraphicsActual, RdpTransportActual, Result, frame_len,
    read_rect,
};
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::Duration,
};
pub use tls::{CertificateApproval, CertificateChallenge};
use tokio::sync::mpsc;
use wire::{Wire, connector_error};
use zeroize::Zeroizing;

#[cfg(debug_assertions)]
fn record_protocol_failure(stage: &'static str, detail: impl std::fmt::Display) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/private/tmp/norishell-rdp-stage.log")
    {
        let _ = writeln!(file, "{} {stage} {detail}", std::process::id());
    }
}

#[cfg(not(debug_assertions))]
fn record_protocol_failure(_: &'static str, _: impl std::fmt::Display) {}

/// The WebView draws the remote pointer locally (CSS cursor), so IronRDP must emit pointer bitmaps
/// instead of compositing them into decoded frame pixels.
const POINTER_SOFTWARE_RENDERING: bool = false;

/// Runs CPU-heavy protocol decoding without stalling other tasks on the same async worker.
/// `block_in_place` is only valid on the multi-threaded runtime; tests use `current_thread`.
fn decode_in_place<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(work)
        }
        _ => work(),
    }
}

pub struct RdpOptions {
    pub server_name: String,
    /// The peer of the already-established direct TCP transport; `None` for SSH gateways.
    pub udp_peer: Option<SocketAddr>,
    pub transport_mode: TransportMode,
    pub graphics_mode: GraphicsMode,
    pub username: String,
    pub domain: Option<String>,
    pub password: Zeroizing<String>,
    pub width: u16,
    pub height: u16,
    pub clipboard_enabled: bool,
    pub audio_playback_enabled: bool,
    pub audio_muted: tokio::sync::watch::Receiver<AudioMuteState>,
    pub certificate_approval: Option<CertificateApproval>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    Auto,
    TcpOnly,
    UdpRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsMode {
    Auto,
    RemoteFx,
    Avc420,
    Bitmap,
}

/// Cancellation outside `run` can drop its inner future at any `.await`. This guard retains the audio
/// owner so cancellation cannot bypass cleanup of the CPAL thread.
struct AudioGuard(Option<Arc<audio::AudioPlayback>>);

impl Drop for AudioGuard {
    fn drop(&mut self) {
        if let Some(audio) = &self.0 {
            audio.close();
        }
    }
}

/// Stop signaling is independent of input backpressure; cancellation drops the engine's exclusively owned transport.
pub async fn run(
    stream: BoxedDesktopIo,
    options: RdpOptions,
    commands: mpsc::Receiver<EngineCommand>,
    mut control: EngineControl,
    events: EventSink,
) -> Result<()> {
    if *control.stop.borrow() {
        return Err(EngineError::Cancelled);
    }
    let audio = options
        .audio_playback_enabled
        .then(|| audio::AudioPlayback::new(events.clone(), options.audio_muted.clone()));
    let _audio_guard = AudioGuard(audio.clone());
    let mut stop = control.stop.clone();
    tokio::select! { biased;
        _ = stop.changed() => Err(EngineError::Cancelled),
        result = inner(stream,options,commands,&mut control,events,audio) => result,
    }
}

async fn inner(
    stream: BoxedDesktopIo,
    options: RdpOptions,
    mut commands: mpsc::Receiver<EngineCommand>,
    control: &mut EngineControl,
    events: EventSink,
    audio: Option<Arc<audio::AudioPlayback>>,
) -> Result<()> {
    frame_len(options.width, options.height)?;
    if options.transport_mode == TransportMode::UdpRequired && options.udp_peer.is_none() {
        return Err(EngineError::RdpUdpUnavailable);
    }
    if options.server_name.is_empty()
        || options.username.is_empty()
        || options.username.len() > 1024
        || options.domain.as_ref().is_some_and(|s| s.len() > 1024)
        || options.password.len() > 65536
    {
        return Err(EngineError::InvalidConfiguration);
    }
    let clip = Arc::new(Mutex::new(clipboard::State::default()));
    let (
        mut wire,
        result,
        mut udp,
        soft_sync,
        graphics_unavailable,
        graphics_seen,
        graphics_actual,
    ) = tokio::time::timeout(
        Duration::from_secs(180),
        connect(
            stream,
            &options,
            clip.clone(),
            events.clone(),
            audio.clone(),
        ),
    )
    .await
    .map_err(|_| EngineError::Timeout)??;
    if graphics_unavailable.load(Ordering::Relaxed) {
        return Err(EngineError::RdpGraphicsUnavailable);
    }
    let required_avc = options.graphics_mode == GraphicsMode::Avc420;
    let clipboard_enabled = options.clipboard_enabled;
    drop(options);
    frame_len(result.desktop_size.width, result.desktop_size.height)?;
    let mut image = DecodedImage::new(
        PixelFormat::RgbA32,
        result.desktop_size.width,
        result.desktop_size.height,
    );
    let mut limits = limits::Limits::new(
        result.static_channels.channel_ids(),
        result
            .static_channels
            .get_channel_id_by_type::<ironrdp::dvc::DrdynvcClient>(),
    );
    let activation = result.activation_factory;
    let mut active = ActiveStageBuilder {
        static_channels: result.static_channels,
        user_channel_id: result.user_channel_id,
        io_channel_id: result.io_channel_id,
        message_channel_id: result.message_channel_id,
        share_id: result.share_id,
        compression_type: result.compression_type,
        enable_server_pointer: result.enable_server_pointer,
        pointer_software_rendering: POINTER_SOFTWARE_RENDERING,
    }
    .build();
    if udp.established() {
        active
            .enable_reliable_udp_dvc_tunnel()
            .map_err(|_| EngineError::Protocol)?;
    }
    let mut input = Database::new();
    let mut pending_udp_payload: Option<Vec<u8>> = None;
    let mut pending_resize: Option<(EngineCommand, tokio::time::Instant)> = None;
    let mut pending_apply: Option<(EngineCommand, u16, u16, tokio::time::Instant)> = None;
    let mut last_transport = None;
    let mut presentation = Presentation::default();
    (events)(EngineEvent::Ready);
    loop {
        if presentation
            .due()
            .is_some_and(|due| due <= tokio::time::Instant::now())
        {
            presentation.flush(&image, &events)?;
        }
        let transport = graphics_transport(&active, &udp);
        if last_transport != Some(transport) {
            (events)(EngineEvent::RdpTransport(transport));
            last_transport = Some(transport);
        }
        if let Some((command, width, height, deadline)) = pending_apply.take() {
            if image.width() == width && image.height() == height {
                let _ = command.completion.send(Ok(()));
            } else if tokio::time::Instant::now() >= deadline {
                let _ = command
                    .completion
                    .send(Err(EngineError::RdpResolutionNotApplied));
            } else {
                pending_apply = Some((command, width, height, deadline));
            }
        }
        if udp.established() && !udp.available() {
            if active.reliable_udp_dvc_tunnel_in_use() {
                return Err(EngineError::ConnectionLost);
            }
            udp.discard();
            active
                .disable_reliable_udp_dvc_tunnel()
                .map_err(|_| EngineError::Protocol)?;
        }
        if active.reliable_udp_dvc_tunnel_in_use() && !udp.available() {
            return Err(EngineError::ConnectionLost);
        }
        if active.reliable_udp_dvc_tunnel_in_use()
            && let Some(payload) = pending_udp_payload.take()
        {
            process_udp_payload(
                &payload,
                &mut udp,
                &mut wire,
                &mut image,
                &mut active,
                &activation,
                &events,
                &mut presentation,
                soft_sync,
                &graphics_unavailable,
                &graphics_seen,
                &graphics_actual,
                required_avc,
            )
            .await?;
            continue;
        }
        if let Some((command, deadline)) = pending_resize.take() {
            let result = send_input(
                &command,
                control,
                &mut input,
                &mut active,
                &mut image,
                &mut wire,
                &udp,
                &clip,
                &events,
                &mut presentation,
                clipboard_enabled,
            )
            .await;
            if matches!(result, Err(EngineError::RdpResolutionUnavailable))
                && tokio::time::Instant::now() < deadline
            {
                pending_resize = Some((command, deadline));
            } else if result.is_ok()
                && let Some((width, height)) = resize_target(&command.input)
            {
                pending_apply = Some((
                    command,
                    width,
                    height,
                    tokio::time::Instant::now() + Duration::from_secs(8),
                ));
            } else {
                let fatal = matches!(
                    result,
                    Err(EngineError::ConnectionLost | EngineError::Timeout | EngineError::Protocol)
                );
                let _ = command.completion.send(result);
                if fatal {
                    return result;
                }
            }
        }
        let resize_deadline = pending_resize
            .as_ref()
            .map(|(_, deadline)| *deadline)
            .into_iter()
            .chain(pending_apply.as_ref().map(|(_, _, _, deadline)| *deadline))
            .min();
        let publish_due = presentation.due();
        tokio::select! { biased;
            _=async move {
                if let Some(deadline) = resize_deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            }=> {
                let now = tokio::time::Instant::now();
                if pending_resize.as_ref().is_some_and(|(_, deadline)| *deadline <= now)
                    && let Some((command, _)) = pending_resize.take() {
                    let _ = command.completion.send(Err(EngineError::RdpResolutionUnavailable));
                }
                if pending_apply.as_ref().is_some_and(|(_, _, _, deadline)| *deadline <= now)
                    && let Some((command, _, _, _)) = pending_apply.take() {
                    let _ = command.completion.send(Err(EngineError::RdpResolutionNotApplied));
                }
            }
            // The loop head flushes pending pixels once their coalescing interval has elapsed.
            _=async move {
                match publish_due {
                    Some(due) => tokio::time::sleep_until(due).await,
                    None => std::future::pending::<()>().await,
                }
            }=>{}
            changed=control.focus_epoch.changed()=> {
                if changed.is_err() { return Err(EngineError::Cancelled); }
                let released=input.release_all();
                for chunk in released.chunks(15) {
                    let outputs=active.process_fastpath_input(&mut image,chunk).map_err(|_|EngineError::Protocol)?;
                    process_outputs(outputs,&mut wire,&mut image,&mut active,&activation,&events,&mut presentation,&mut udp,soft_sync).await?;
                }
            }
            command=commands.recv()=> {
                let Some(command)=command else { return Ok(()); };
                if !control.accepts(&command) { let _=command.completion.send(Err(EngineError::StaleInput)); continue; }
                if let Err(error)=command.input.validate() { let _=command.completion.send(Err(error)); continue; }
                if matches!(&command.input, DesktopInput::Resize { .. })
                    && let Some((previous, _)) = pending_resize.take() {
                    let _ = previous.completion.send(Err(EngineError::StaleInput));
                }
                if matches!(&command.input, DesktopInput::Resize { .. })
                    && let Some((previous, _, _, _)) = pending_apply.take() {
                    let _ = previous.completion.send(Err(EngineError::StaleInput));
                }
                let result=send_input(&command,control,&mut input,&mut active,&mut image,&mut wire,&udp,&clip,&events,&mut presentation,clipboard_enabled).await;
                if command.focus_epoch.is_none() && matches!(result, Err(EngineError::RdpResolutionUnavailable)) {
                    pending_resize = Some((command, tokio::time::Instant::now() + Duration::from_secs(4)));
                    continue;
                }
                if result.is_ok() && let Some((width, height)) = resize_target(&command.input) {
                    pending_apply = Some((command, width, height, tokio::time::Instant::now() + Duration::from_secs(8)));
                    continue;
                }
                let fatal=matches!(result,Err(EngineError::ConnectionLost|EngineError::Timeout|EngineError::Protocol));
                if let Err(error) = &result { record_protocol_failure("send_input", error); }
                let _=command.completion.send(result);
                if fatal { return result; }
            }
            packet=wire.pdu()=> {
                let (action,bytes)=packet.inspect_err(|error| { record_protocol_failure("wire.pdu", error); })?;
                limits.check(action,&bytes).inspect_err(|error| { record_protocol_failure("limits.check", error); })?;
                graphics_seen.store(false, Ordering::Relaxed);
                let outputs=decode_in_place(|| active.process(&mut image,action,&bytes)).map_err(|error| { record_protocol_failure("active.process", error.report()); EngineError::Protocol })?;
                check_graphics_mode(&outputs, &graphics_unavailable, &graphics_seen, &graphics_actual, required_avc, &events)?;
                if process_outputs(outputs,&mut wire,&mut image,&mut active,&activation,&events,&mut presentation,&mut udp,soft_sync).await.inspect_err(|error| { record_protocol_failure("process_outputs", error); })? { return Ok(()); }
                flush_clipboard(&mut active,&mut wire,&clip).await.inspect_err(|error| { record_protocol_failure("flush_clipboard", error); })?;
            }
            payload=udp.recv(), if udp.available() && pending_udp_payload.is_none()=> {
                match payload {
                    Some(payload) if payload.is_empty() => {},
                    Some(payload) if payload.len() > u16::MAX as usize => return Err(EngineError::ResourceLimit),
                    Some(payload) if active.reliable_udp_dvc_tunnel_in_use() => {
                        process_udp_payload(&payload,&mut udp,&mut wire,&mut image,&mut active,&activation,&events,&mut presentation,soft_sync,&graphics_unavailable,&graphics_seen,&graphics_actual,required_avc).await.inspect_err(|error| { record_protocol_failure("udp_payload", error); })?;
                    }
                    Some(payload) => pending_udp_payload=Some(payload),
                    None if active.reliable_udp_dvc_tunnel_in_use() => return Err(EngineError::ConnectionLost),
                    None => { udp.discard(); active.disable_reliable_udp_dvc_tunnel().map_err(|_|EngineError::Protocol)?; }
                }
            }
        }
    }
}

fn resize_target(input: &DesktopInput) -> Option<(u16, u16)> {
    match input {
        DesktopInput::Resize { width, height, .. } => Some((*width, *height)),
        _ => None,
    }
}

fn graphics_transport(active: &ActiveStage, udp: &udp::UdpSession) -> RdpTransportActual {
    use ironrdp::dvc::pdu::SoftSyncTunnelType;
    let graphics_on_udp = udp.available()
        && active
            .get_dvc::<ironrdp_egfx::client::GraphicsPipelineClient>()
            .is_some_and(|graphics| {
                active.dvc_tunnel_for_channel(graphics.channel_id())
                    == Some(SoftSyncTunnelType::RELIABLE_UDP)
            });
    if graphics_on_udp {
        RdpTransportActual::Udp
    } else {
        RdpTransportActual::Tcp
    }
}

fn check_graphics_mode(
    outputs: &[ActiveStageOutput],
    unavailable: &AtomicBool,
    seen: &AtomicBool,
    actual: &AtomicU8,
    required_avc: bool,
    events: &EventSink,
) -> Result<()> {
    if unavailable.load(Ordering::Relaxed) {
        return Err(EngineError::RdpGraphicsUnavailable);
    }
    if outputs
        .iter()
        .any(|output| matches!(output, ActiveStageOutput::GraphicsUpdate(_)))
        && !seen.load(Ordering::Relaxed)
    {
        if required_avc {
            return Err(EngineError::RdpGraphicsUnavailable);
        }
        report_graphics_actual(RdpGraphicsActual::Bitmap, actual, events);
    }
    Ok(())
}

fn report_graphics_actual(actual: RdpGraphicsActual, reported: &AtomicU8, events: &EventSink) {
    let code = match actual {
        RdpGraphicsActual::Bitmap => 1,
        RdpGraphicsActual::RemoteFx => 2,
        RdpGraphicsActual::RemoteFxProgressive => 3,
        RdpGraphicsActual::Avc420 => 4,
    };
    if reported.swap(code, Ordering::Relaxed) != code {
        (events)(EngineEvent::RdpGraphics(actual));
    }
}

async fn connect(
    stream: BoxedDesktopIo,
    options: &RdpOptions,
    clip: Arc<Mutex<clipboard::State>>,
    events: EventSink,
    audio: Option<Arc<audio::AudioPlayback>>,
) -> Result<(
    Wire,
    connector::ConnectionResult,
    udp::UdpSession,
    bool,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
    Arc<AtomicU8>,
)> {
    let mut connector = ClientConnector::new(
        config(options),
        "0.0.0.0:0"
            .parse()
            .map_err(|_| EngineError::InvalidConfiguration)?,
    );
    struct EgfxHandler {
        mode: GraphicsMode,
        events: EventSink,
        unavailable: Arc<AtomicBool>,
        seen: Arc<AtomicBool>,
        actual: Arc<AtomicU8>,
    }
    impl EgfxHandler {
        fn report(&mut self, actual: RdpGraphicsActual) {
            self.seen.store(true, Ordering::Relaxed);
            report_graphics_actual(actual, &self.actual, &self.events);
        }
    }
    impl ironrdp_egfx::client::GraphicsPipelineHandler for EgfxHandler {
        fn capabilities(&self) -> Vec<ironrdp_egfx::pdu::CapabilitySet> {
            use ironrdp_egfx::pdu::{CapabilitiesV8Flags, CapabilitiesV81Flags, CapabilitySet};
            let v8 = CapabilitySet::V8 {
                flags: CapabilitiesV8Flags::SMALL_CACHE,
            };
            let v81 = CapabilitySet::V8_1 {
                flags: CapabilitiesV81Flags::AVC420_ENABLED | CapabilitiesV81Flags::SMALL_CACHE,
            };
            match self.mode {
                GraphicsMode::Auto => vec![v81, v8],
                GraphicsMode::RemoteFx | GraphicsMode::Bitmap => vec![v8],
                GraphicsMode::Avc420 => vec![v81],
            }
        }
        fn on_capabilities_confirmed(&mut self, caps: &ironrdp_egfx::pdu::CapabilitySet) {
            record_protocol_failure("egfx.confirmed", format_args!("{caps:?}"));
            if self.mode == GraphicsMode::Avc420
                && !matches!(caps, ironrdp_egfx::pdu::CapabilitySet::V8_1 { flags } if flags.contains(ironrdp_egfx::pdu::CapabilitiesV81Flags::AVC420_ENABLED))
            {
                self.unavailable.store(true, Ordering::Relaxed);
            }
        }
        fn on_bitmap_updated(&mut self, update: &ironrdp_egfx::client::BitmapUpdate) {
            use ironrdp_egfx::pdu::Codec1Type;
            let actual = match update.codec_id {
                Codec1Type::Avc420 => RdpGraphicsActual::Avc420,
                Codec1Type::RemoteFx => RdpGraphicsActual::RemoteFx,
                // Progressive tiles are delivered as uncompressed RGBA after decoding;
                // its completion callback reports their actual wire codec.
                Codec1Type::Uncompressed => return,
                _ => RdpGraphicsActual::Bitmap,
            };
            if self.mode == GraphicsMode::Avc420 && actual != RdpGraphicsActual::Avc420 {
                self.unavailable.store(true, Ordering::Relaxed);
            }
            self.report(actual);
        }
        fn on_progressive_decoded(&mut self) {
            if self.mode == GraphicsMode::Avc420 {
                self.unavailable.store(true, Ordering::Relaxed);
            }
            self.report(RdpGraphicsActual::RemoteFxProgressive);
        }
    }
    let graphics_unavailable = Arc::new(AtomicBool::new(false));
    let graphics_seen = Arc::new(AtomicBool::new(false));
    let graphics_actual = Arc::new(AtomicU8::new(0));
    let decoder = match ironrdp_egfx::decode::OpenH264Decoder::new() {
        Ok(decoder) => {
            record_protocol_failure("h264.decoder", "ready");
            Some(Box::new(decoder) as Box<dyn ironrdp_egfx::decode::H264Decoder>)
        }
        Err(error) => {
            record_protocol_failure("h264.decoder", error);
            None
        }
    };
    if options.graphics_mode == GraphicsMode::Avc420 && decoder.is_none() {
        return Err(EngineError::RdpGraphicsUnavailable);
    }
    connector.attach_static_channel(
        ironrdp::dvc::DrdynvcClient::new()
            .with_dynamic_channel(ironrdp::displaycontrol::client::DisplayControlClient::new(
                |_| Ok(Vec::new()),
            ))
            .with_dynamic_channel(ironrdp_egfx::client::GraphicsPipelineClient::new(
                Box::new(EgfxHandler {
                    mode: options.graphics_mode,
                    events: events.clone(),
                    unavailable: graphics_unavailable.clone(),
                    seen: graphics_seen.clone(),
                    actual: graphics_actual.clone(),
                }),
                decoder,
            )),
    );
    if options.clipboard_enabled {
        connector.attach_static_channel(ironrdp::cliprdr::CliprdrClient::new(Box::new(
            clipboard::Backend {
                state: clip,
                events,
            },
        )));
    }
    if let Some(audio) = audio {
        connector.attach_static_channel(ironrdp::rdpsnd::client::Rdpsnd::new(Box::new(
            audio::Backend::new(audio),
        )));
    }
    let mut wire = Wire::new(stream);
    while !connector.should_perform_security_upgrade() {
        wire.step(&mut connector).await?;
    }
    if !wire.buffer.is_empty() {
        return Err(EngineError::Protocol);
    }
    let (stream, key, approved_leaf) = tls::upgrade(
        wire.stream,
        &options.server_name,
        options.certificate_approval.as_ref(),
    )
    .await?;
    let mut wire = Wire::new(stream);
    let udp_peer = (options.transport_mode != TransportMode::TcpOnly)
        .then_some(options.udp_peer)
        .flatten();
    let mut udp = udp::UdpSession::new(udp_peer, &options.server_name, approved_leaf);
    connector.mark_security_upgrade_as_done();
    if connector.should_perform_credssp() {
        let protocol = match connector.state {
            ClientConnectorState::Credssp {
                selected_protocol, ..
            } => selected_protocol,
            _ => return Err(EngineError::Protocol),
        };
        let (mut sequence, mut request) = connector::credssp::CredsspSequence::init(
            connector.config.credentials.clone(),
            connector.config.domain.as_deref(),
            protocol,
            options.server_name.clone().into(),
            key,
            None,
        )
        .map_err(connector_error)?;
        loop {
            // A missing KerberosConfig forces NTLM; reject all unexpected external network requests.
            let state = {
                use connector::sspi::generator::GeneratorState;
                let mut generator = sequence.process_ts_request(request);
                match generator.start() {
                    GeneratorState::Completed(value) => {
                        value.map_err(|_| EngineError::AuthenticationRejected)?
                    }
                    GeneratorState::Suspended(_) => {
                        return Err(EngineError::UnsupportedAuthentication);
                    }
                }
            };
            let mut output = WriteBuf::new();
            let written = sequence
                .handle_process_result(state, &mut output)
                .map_err(connector_error)?;
            if written.size().is_some() {
                wire.write(output.filled()).await?;
            }
            let Some(hint) = sequence.next_pdu_hint() else {
                break;
            };
            let bytes = wire.hint(hint).await?;
            match sequence
                .decode_server_message(&bytes)
                .map_err(connector_error)?
            {
                Some(next) => request = next,
                None => break,
            }
        }
        connector.mark_credssp_as_done();
    }
    loop {
        if connector.should_perform_multitransport() {
            let request = connector
                .multitransport_request()
                .cloned()
                .ok_or(EngineError::Protocol)?;
            let soft_sync = connector
                .multitransport_soft_sync_negotiated()
                .ok_or(EngineError::Protocol)?;
            let success = udp.bootstrap(request, soft_sync).await;
            let mut output = WriteBuf::new();
            let written = connector
                .complete_multitransport(
                    if success {
                        connector::MultitransportResult::Success
                    } else {
                        connector::MultitransportResult::Failure(
                            ironrdp::pdu::rdp::multitransport::MultitransportResponsePdu::E_ABORT,
                        )
                    },
                    &mut output,
                )
                .map_err(connector_error)?;
            if written.size().is_some() {
                wire.write(output.filled()).await?;
            }
            continue;
        }
        wire.step(&mut connector).await?;
        if let ClientConnectorState::Connected { mut result } = connector.state {
            let soft_sync = result.multitransport_soft_sync();
            record_protocol_failure(
                "transport.connected",
                format_args!(
                    "direct_udp_peer={} soft_sync={soft_sync} udp_established={}",
                    options.udp_peer.is_some(),
                    udp.established()
                ),
            );
            // Reactivation needs no credentials; avoid retaining the connector password for the entire desktop session.
            let mut clean = config(options);
            if let Credentials::UsernamePassword { password, .. } = &mut clean.credentials {
                zeroize::Zeroize::zeroize(password);
            }
            clean.credentials = Credentials::UsernamePassword {
                username: String::new(),
                password: String::new(),
            };
            result.activation_factory =
                connector::connection_activation::ConnectionActivationFactory::new(
                    clean,
                    result.io_channel_id,
                    result.user_channel_id,
                );
            if let Credentials::UsernamePassword { password, .. } =
                &mut connector.config.credentials
            {
                zeroize::Zeroize::zeroize(password);
            }
            if options.transport_mode == TransportMode::UdpRequired && !udp.established() {
                return Err(EngineError::RdpUdpUnavailable);
            }
            return Ok((
                wire,
                result,
                udp,
                soft_sync,
                graphics_unavailable,
                graphics_seen,
                graphics_actual,
            ));
        }
    }
}

fn config(options: &RdpOptions) -> connector::Config {
    use ironrdp::pdu::{
        gcc::{ConnectionType, KeyboardType},
        rdp::{
            capability_sets::{CodecProperty, MajorPlatformType, client_codecs_capabilities},
            client_info::{PerformanceFlags, TimezoneInfo},
        },
    };
    let mut codecs = client_codecs_capabilities(&[]).expect("default bitmap codecs");
    // The legacy renderer implements RemoteFX, but not every codec the connector may offer.
    codecs
        .0
        .retain(|codec| matches!(&codec.property, CodecProperty::RemoteFx(_)));
    if options.graphics_mode == GraphicsMode::Bitmap {
        codecs.0.clear();
    }
    connector::Config {
        credentials: Credentials::UsernamePassword {
            username: options.username.clone(),
            password: options.password.to_string(),
        },
        domain: options.domain.clone(),
        enable_tls: true,
        enable_credssp: true,
        enable_standard_rdp_security: false,
        keyboard_type: KeyboardType::IBM_ENHANCED,
        keyboard_subtype: 0,
        keyboard_layout: 0,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: connector::DesktopSize {
            width: options.width,
            height: options.height,
        },
        monitor_layout: None,
        bitmap: Some(connector::BitmapConfig {
            lossy_compression: true,
            color_depth: 32,
            codecs,
        }),
        connection_type: ConnectionType::Wan,
        client_build: 0,
        client_name: "NoriShell".into(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".into(),
        platform: MajorPlatformType::UNSPECIFIED,
        enable_server_pointer: true,
        request_data: None,
        autologon: true,
        enable_audio_playback: options.audio_playback_enabled,
        enable_audio_capture: false,
        compression_type: None,
        pointer_software_rendering: POINTER_SOFTWARE_RENDERING,
        multitransport_flags: options
            .udp_peer
            .filter(|_| options.transport_mode != TransportMode::TcpOnly)
            .map(|_| {
                ironrdp::pdu::gcc::MultiTransportFlags::TRANSPORT_TYPE_UDP_FECR
                    | ironrdp::pdu::gcc::MultiTransportFlags::SOFT_SYNC_TCP_TO_UDP
            }),
        support_dyn_vc_gfx_protocol: options.graphics_mode != GraphicsMode::Bitmap,
        remote_application_mode: false,
        rail_support_level: ironrdp::pdu::rdp::capability_sets::RailSupportLevel::empty(),
        performance_flags: PerformanceFlags::default(),
        desktop_scale_factor: 0,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        alternate_shell: String::new(),
        work_dir: String::new(),
    }
}

fn dirty_rect(
    left: u16,
    top: u16,
    right: u16,
    bottom: u16,
    width: u16,
    height: u16,
) -> Option<DesktopRect> {
    if left > right || top > bottom || right >= width || bottom >= height {
        return None;
    }
    let rect = DesktopRect {
        x: left,
        y: top,
        width: right - left + 1,
        height: bottom - top + 1,
    };
    rect.fits(width, height).then_some(rect)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishKind {
    Nothing,
    Full,
    Patch,
}

/// Chooses between a full frame and a patch. A full frame is required after activation, a size change, an
/// out-of-bounds region, or when the dirty area reaches 60% of the frame (a patch would save little).
fn publish_kind(force_full: bool, region: &DirtyRegion, width: u16, height: u16) -> PublishKind {
    if force_full || !region.fits(width, height) {
        return PublishKind::Full;
    }
    if region.is_empty() {
        return PublishKind::Nothing;
    }
    let frame_area = u64::from(width) * u64::from(height);
    if region.area() * 10 >= frame_area * 6 {
        PublishKind::Full
    } else {
        PublishKind::Patch
    }
}

/// Converts an IronRDP accelerated-target pointer (straight RGBA; inverted pixels already expanded to a
/// checker pattern) into a WebView cursor. A zero-sized pointer means hidden; an invalid one falls back to
/// the default arrow instead of failing the session.
fn cursor_from_pointer(pointer: &DecodedPointer) -> DesktopCursor {
    if pointer.width == 0 || pointer.height == 0 {
        return DesktopCursor::Hidden;
    }
    match CursorBitmap::new(
        pointer.width,
        pointer.height,
        pointer.hotspot_x,
        pointer.hotspot_y,
        pointer.bitmap_data.clone(),
    ) {
        Ok(bitmap) => DesktopCursor::Bitmap(Arc::new(bitmap)),
        Err(_) => DesktopCursor::Default,
    }
}

/// Engine-side presentation state: decoded pixels changed since the last publish and the last cursor sent.
/// Protocol updates only record damage; the engine loop publishes at most once per `PUBLISH_INTERVAL`.
#[derive(Default)]
struct Presentation {
    region: DirtyRegion,
    needs_full: bool,
    pending: bool,
    /// `None` publishes immediately (first frame, reactivation).
    last_publish: Option<tokio::time::Instant>,
    published_size: Option<(u16, u16)>,
    cursor: Option<DesktopCursor>,
    /// Identity of the last converted pointer so cached pointer re-selection skips reconversion.
    cursor_source: Option<Arc<DecodedPointer>>,
    #[cfg(debug_assertions)]
    stats: PublishStats,
}

#[cfg(debug_assertions)]
#[derive(Default)]
struct PublishStats {
    frames: u64,
    patches: u64,
    bytes: u64,
    copy_us: u64,
}

impl Presentation {
    fn mark(&mut self, rect: Option<DesktopRect>) {
        self.pending = true;
        match rect {
            Some(rect) => self.region.add(rect),
            None => self.needs_full = true,
        }
    }

    /// After (re)activation the decode buffer was replaced; publish it in full without waiting.
    fn reset_after_activation(&mut self) {
        self.needs_full = true;
        self.pending = true;
        self.region.clear();
        self.last_publish = None;
        // The rebuilt fast-path processor drops its pointer cache; identity no longer proves equality.
        self.cursor_source = None;
    }

    fn due(&self) -> Option<tokio::time::Instant> {
        if !self.pending {
            return None;
        }
        Some(match self.last_publish {
            Some(last) => last + PUBLISH_INTERVAL,
            None => tokio::time::Instant::now(),
        })
    }

    /// Records damage and pointer changes from one processing batch. Must run before any later
    /// `ActiveStage` processing call, which clears the stage's damage list.
    fn observe(
        &mut self,
        outputs: &[ActiveStageOutput],
        active: &mut ActiveStage,
        image: &DecodedImage,
        events: &EventSink,
    ) {
        let mut updates = Vec::new();
        let mut cursor = None;
        for output in outputs {
            match output {
                ActiveStageOutput::GraphicsUpdate(rect) => updates.push(rect.clone()),
                ActiveStageOutput::PointerDefault
                | ActiveStageOutput::PointerHidden
                | ActiveStageOutput::PointerBitmap(_) => cursor = Some(output),
                // Server-side pointer warps are ignored; the local pointer stays under user control.
                _ => {}
            }
        }
        if !updates.is_empty() {
            // Exact per-rectangle damage (EGFX composites report one bounding box per batch).
            let damage = active.take_damage_regions();
            let rects = if damage.is_empty() { updates } else { damage };
            for rect in rects {
                self.mark(dirty_rect(
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom,
                    image.width(),
                    image.height(),
                ));
            }
        }
        // Only the final pointer state of a batch matters (cached selection emits Hidden then Bitmap).
        if let Some(output) = cursor {
            self.set_cursor(output, events);
        }
    }

    fn set_cursor(&mut self, output: &ActiveStageOutput, events: &EventSink) {
        let next = match output {
            ActiveStageOutput::PointerDefault => DesktopCursor::Default,
            ActiveStageOutput::PointerHidden => DesktopCursor::Hidden,
            ActiveStageOutput::PointerBitmap(pointer) => {
                if matches!(self.cursor, Some(DesktopCursor::Bitmap(_)))
                    && self
                        .cursor_source
                        .as_ref()
                        .is_some_and(|source| Arc::ptr_eq(source, pointer))
                {
                    return;
                }
                self.cursor_source = Some(pointer.clone());
                cursor_from_pointer(pointer)
            }
            _ => return,
        };
        if !matches!(next, DesktopCursor::Bitmap(_)) {
            self.cursor_source = None;
        }
        if self.cursor.as_ref() == Some(&next) {
            return;
        }
        self.cursor = Some(next.clone());
        events(EngineEvent::Cursor(next));
    }

    fn flush(&mut self, image: &DecodedImage, events: &EventSink) -> Result<()> {
        #[cfg(debug_assertions)]
        let copy_started = std::time::Instant::now();
        let (width, height) = (image.width(), image.height());
        let expected = frame_len(width, height)?;
        if image.data().len() != expected {
            return Err(EngineError::Protocol);
        }
        let force_full = self.needs_full || self.published_size != Some((width, height));
        let kind = publish_kind(force_full, &self.region, width, height);
        let mut copied = 0usize;
        match kind {
            PublishKind::Nothing => {}
            PublishKind::Full => {
                copied = expected;
                events(EngineEvent::Frame(Arc::new(DesktopFrame {
                    width,
                    height,
                    rgba: image.data().to_vec(),
                })));
                self.published_size = Some((width, height));
            }
            PublishKind::Patch => {
                let rects: Vec<_> = self
                    .region
                    .rects()
                    .iter()
                    .map(|rect| (*rect, read_rect(image.data(), width, *rect)))
                    .collect();
                copied = rects.iter().map(|(_, rgba)| rgba.len()).sum();
                events(EngineEvent::FramePatch(FramePatch {
                    width,
                    height,
                    rects,
                }));
            }
        }
        self.region.clear();
        self.needs_full = false;
        self.pending = false;
        self.last_publish = Some(tokio::time::Instant::now());
        #[cfg(debug_assertions)]
        {
            let stats = &mut self.stats;
            match kind {
                PublishKind::Full => stats.frames += 1,
                PublishKind::Patch => stats.patches += 1,
                PublishKind::Nothing => {}
            }
            stats.bytes += copied as u64;
            stats.copy_us += copy_started.elapsed().as_micros() as u64;
            let published = stats.frames + stats.patches;
            if kind != PublishKind::Nothing && published.is_multiple_of(60) {
                record_protocol_failure(
                    "publish.60",
                    format_args!(
                        "frames={} patches={} copied_bytes={} copy_us={} size={width}x{height}",
                        stats.frames, stats.patches, stats.bytes, stats.copy_us
                    ),
                );
            }
        }
        #[cfg(not(debug_assertions))]
        let _ = copied;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_outputs(
    outputs: Vec<ActiveStageOutput>,
    wire: &mut Wire,
    image: &mut DecodedImage,
    active: &mut ActiveStage,
    activation: &connector::connection_activation::ConnectionActivationFactory,
    events: &EventSink,
    presentation: &mut Presentation,
    udp: &mut udp::UdpSession,
    soft_sync: bool,
) -> Result<bool> {
    presentation.observe(&outputs, active, image, events);
    for output in outputs {
        match output {
            ActiveStageOutput::ResponseFrame(frame) => wire.write(&frame).await?,
            ActiveStageOutput::Terminate(_) => return Ok(true),
            ActiveStageOutput::MultitransportRequest(request) => {
                let success = udp.bootstrap(request.clone(), soft_sync).await;
                if let Some(response) = udp::response(request.request_id, success, soft_sync) {
                    let frame = active
                        .encode_multitransport_response(&response)
                        .map_err(|_| EngineError::Protocol)?;
                    wire.write(&frame).await?;
                }
                if success {
                    active
                        .enable_reliable_udp_dvc_tunnel()
                        .map_err(|_| EngineError::Protocol)?;
                }
            }
            ActiveStageOutput::DeactivateAll => {
                let mut sequence = activation.create();
                loop {
                    wire.step(&mut sequence).await?;
                    if let connector::connection_activation::ConnectionActivationState::Finalized { desktop_size,share_id,enable_server_pointer, .. }=sequence.connection_activation_state() {
                        frame_len(desktop_size.width,desktop_size.height)?;
                        *image=DecodedImage::new(PixelFormat::RgbA32,desktop_size.width,desktop_size.height);
                        presentation.reset_after_activation();
                        active.set_share_id(share_id);
                        active.set_enable_server_pointer(enable_server_pointer);
                        active.set_fastpath_processor(ironrdp::session::fast_path::ProcessorBuilder { io_channel_id:activation.io_channel_id(),user_channel_id:activation.user_channel_id(),share_id,enable_server_pointer,pointer_software_rendering:POINTER_SOFTWARE_RENDERING }.build());
                        break;
                    }
                }
            }
            // Graphics and pointer outputs were consumed by `observe` above.
            _ => {}
        }
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
async fn process_udp_payload(
    payload: &[u8],
    udp: &mut udp::UdpSession,
    wire: &mut Wire,
    image: &mut DecodedImage,
    active: &mut ActiveStage,
    activation: &connector::connection_activation::ConnectionActivationFactory,
    events: &EventSink,
    presentation: &mut Presentation,
    soft_sync: bool,
    graphics_unavailable: &AtomicBool,
    graphics_seen: &AtomicBool,
    graphics_actual: &AtomicU8,
    required_avc: bool,
) -> Result<()> {
    use ironrdp::dvc::pdu::SoftSyncTunnelType;
    limits::Limits::check_udp_dvc(payload)?;
    graphics_seen.store(false, Ordering::Relaxed);
    let (batch, outputs) = decode_in_place(|| {
        active.process_dvc_tunnel(image, SoftSyncTunnelType::RELIABLE_UDP, payload)
    })
    .map_err(|error| {
        record_protocol_failure("udp.process_dvc_tunnel", error.report());
        EngineError::Protocol
    })?;
    check_graphics_mode(
        &outputs,
        graphics_unavailable,
        graphics_seen,
        graphics_actual,
        required_avc,
        events,
    )?;
    let channel_id = batch.channel_id();
    let messages = batch.into_messages();
    if !messages.is_empty() {
        if active.dvc_tunnel_for_channel(channel_id) == Some(SoftSyncTunnelType::RELIABLE_UDP) {
            for message in messages {
                let bytes = message
                    .encode_unframed_pdu()
                    .map_err(|_| EngineError::Protocol)?;
                udp.send(bytes).await?;
            }
        } else {
            let frame = active
                .encode_dvc_messages(messages)
                .map_err(|_| EngineError::Protocol)?;
            wire.write(&frame).await?;
        }
    }
    if process_outputs(
        outputs,
        wire,
        image,
        active,
        activation,
        events,
        presentation,
        udp,
        soft_sync,
    )
    .await?
    {
        return Err(EngineError::ConnectionLost);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn send_input(
    command: &EngineCommand,
    control: &EngineControl,
    input: &mut Database,
    active: &mut ActiveStage,
    image: &mut DecodedImage,
    wire: &mut Wire,
    udp: &udp::UdpSession,
    clip: &Arc<Mutex<clipboard::State>>,
    events: &EventSink,
    presentation: &mut Presentation,
    clipboard_enabled: bool,
) -> Result<()> {
    if !control.accepts(command) {
        return Err(EngineError::StaleInput);
    }
    match &command.input {
        DesktopInput::Resize {
            width,
            height,
            scale_percent,
        } => {
            if *width < 200 || *height < 200 || width % 2 != 0 {
                return Err(EngineError::InvalidConfiguration);
            }
            // Every request carries an explicit scale so returning to 100% after a HiDPI layout takes effect;
            // an absent field would let the server keep the previous factor. Physical size is optional in
            // MS-RDPEDISP and unknown to the engine, so it is not sent.
            let scale_factor = Some(u32::from(*scale_percent));
            let batch = active
                .prepare_resize(u32::from(*width), u32::from(*height), scale_factor, None)
                .ok_or(EngineError::RdpResolutionUnavailable)?
                .map_err(|_| EngineError::Protocol)?;
            if !control.accepts(command) {
                return Err(EngineError::StaleInput);
            }
            if active.dvc_tunnel_for_channel(batch.channel_id())
                == Some(ironrdp::dvc::pdu::SoftSyncTunnelType::RELIABLE_UDP)
            {
                for message in batch.into_messages() {
                    let bytes = message
                        .encode_unframed_pdu()
                        .map_err(|_| EngineError::Protocol)?;
                    if let Some(epoch) = command.focus_epoch {
                        udp.send_fenced(bytes, control, epoch).await?;
                    } else {
                        udp.send(bytes).await?;
                    }
                }
                Ok(())
            } else if active.dvc_tunnel_for_channel(batch.channel_id()).is_some() {
                Err(EngineError::UnsupportedOperation)
            } else {
                let bytes = active
                    .encode_dvc_messages(batch.into_messages())
                    .map_err(|_| EngineError::Protocol)?;
                if let Some(epoch) = command.focus_epoch {
                    wire.write_fenced(&bytes, control, epoch).await
                } else {
                    wire.write(&bytes).await
                }
            }
        }
        DesktopInput::Clipboard(text) => {
            if !clipboard_enabled {
                return Err(EngineError::UnsupportedOperation);
            }
            if !control.accepts(command) {
                return Err(EngineError::StaleInput);
            }
            {
                let mut state = clip.lock().map_err(|_| EngineError::Protocol)?;
                if !state.ready {
                    return Err(EngineError::UnsupportedOperation);
                }
                state.text = Some(text.clone());
                state.advertise = true;
            }
            flush_clipboard_fenced(
                active,
                wire,
                clip,
                Some((control, command.focus_epoch.ok_or(EngineError::StaleInput)?)),
            )
            .await
        }
        other => {
            let batch = if matches!(other, DesktopInput::ReleaseAll) {
                input.release_all()
            } else {
                input.apply(operations(other, image.width(), image.height())?)
            };
            // FastPath uses a four-bit event count, so split long text; never replay a failed or uncertain write.
            for chunk in batch.chunks(15) {
                if !control.accepts(command) {
                    return Err(EngineError::StaleInput);
                }
                let outputs = active
                    .process_fastpath_input(image, chunk)
                    .map_err(|_| EngineError::Protocol)?;
                presentation.observe(&outputs, active, image, events);
                for output in outputs {
                    if let ActiveStageOutput::ResponseFrame(bytes) = output {
                        if !control.accepts(command) {
                            return Err(EngineError::StaleInput);
                        }
                        wire.write_fenced(
                            &bytes,
                            control,
                            command.focus_epoch.ok_or(EngineError::StaleInput)?,
                        )
                        .await?;
                    }
                }
            }
            Ok(())
        }
    }
}

fn operations(value: &DesktopInput, width: u16, height: u16) -> Result<Vec<Operation>> {
    let mut ops = Vec::new();
    match value {
        DesktopInput::Key {
            scan_code, down, ..
        } => {
            let key = Scancode::from_u8(scan_code & 0x100 != 0, (*scan_code & 0xff) as u8);
            ops.push(if *down {
                Operation::KeyPressed(key)
            } else {
                Operation::KeyReleased(key)
            });
        }
        DesktopInput::Pointer { x, y, buttons } => {
            ops.push(Operation::MouseMove(MousePosition {
                x: (*x).min(width - 1),
                y: (*y).min(height - 1),
            }));
            for (bit, button) in [
                (1, MouseButton::Left),
                (2, MouseButton::Middle),
                (4, MouseButton::Right),
            ] {
                ops.push(if buttons & bit != 0 {
                    Operation::MouseButtonPressed(button)
                } else {
                    Operation::MouseButtonReleased(button)
                });
            }
        }
        DesktopInput::Wheel {
            x,
            y,
            delta_x,
            delta_y,
        } => {
            ops.push(Operation::MouseMove(MousePosition {
                x: (*x).min(width - 1),
                y: (*y).min(height - 1),
            }));
            for (is_vertical, rotation_units) in [(true, *delta_y), (false, *delta_x)] {
                if rotation_units != 0 {
                    ops.push(Operation::WheelRotations(WheelRotations {
                        is_vertical,
                        rotation_units: rotation_units.clamp(-255, 255),
                    }));
                }
            }
        }
        DesktopInput::Text(text) => {
            for ch in text.chars() {
                ops.push(Operation::UnicodeKeyPressed(ch));
                ops.push(Operation::UnicodeKeyReleased(ch));
            }
        }
        _ => return Err(EngineError::UnsupportedOperation),
    }
    Ok(ops)
}

async fn flush_clipboard(
    active: &mut ActiveStage,
    wire: &mut Wire,
    state: &Arc<Mutex<clipboard::State>>,
) -> Result<()> {
    flush_clipboard_fenced(active, wire, state, None).await
}
async fn flush_clipboard_fenced(
    active: &mut ActiveStage,
    wire: &mut Wire,
    state: &Arc<Mutex<clipboard::State>>,
    fence: Option<(&EngineControl, u64)>,
) -> Result<()> {
    use ironrdp::cliprdr::{
        CliprdrClient,
        pdu::{ClipboardFormat, ClipboardFormatId},
    };
    let (advertise, has_text, request, response) = {
        let mut state = state.lock().map_err(|_| EngineError::Protocol)?;
        (
            std::mem::take(&mut state.advertise),
            state.text.is_some(),
            std::mem::take(&mut state.request),
            state.response.take(),
        )
    };
    let Some(channel) = active.get_svc_processor_mut::<CliprdrClient>() else {
        return Ok(());
    };
    let mut messages = Vec::new();
    if advertise {
        messages.push(
            channel
                .initiate_copy(if has_text {
                    &[ClipboardFormat {
                        id: ClipboardFormatId::CF_UNICODETEXT,
                        name: None,
                    }]
                } else {
                    &[]
                })
                .map_err(|_| EngineError::Protocol)?,
        );
    }
    if request {
        messages.push(
            channel
                .initiate_paste(ClipboardFormatId::CF_UNICODETEXT)
                .map_err(|_| EngineError::Protocol)?,
        );
    }
    if let Some(response) = response {
        messages.push(
            channel
                .submit_format_data(response)
                .map_err(|_| EngineError::Protocol)?,
        );
    }
    for message in messages {
        let bytes = active
            .process_svc_processor_messages(message)
            .map_err(|_| EngineError::Protocol)?;
        if let Some((control, epoch)) = fence {
            wire.write_fenced(&bytes, control, epoch).await?;
        } else {
            wire.write(&bytes).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graphics_update_bounds_are_inclusive_and_invalid_bounds_fall_back() {
        assert_eq!(
            dirty_rect(2, 3, 4, 5, 8, 8),
            Some(DesktopRect {
                x: 2,
                y: 3,
                width: 3,
                height: 3
            })
        );
        assert!(dirty_rect(0, 0, u16::MAX, 1, 8, 8).is_none());
        assert!(dirty_rect(4, 0, 3, 1, 8, 8).is_none());
    }
    fn rect(x: u16, y: u16, width: u16, height: u16) -> DesktopRect {
        DesktopRect {
            x,
            y,
            width,
            height,
        }
    }

    fn recording_sink() -> (EventSink, Arc<Mutex<Vec<EngineEvent>>>) {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let sink: EventSink = {
            let recorded = recorded.clone();
            Arc::new(move |event| recorded.lock().unwrap().push(event))
        };
        (sink, recorded)
    }

    #[test]
    fn publish_decision_prefers_patches_for_small_regions() {
        let mut region = DirtyRegion::default();
        assert_eq!(publish_kind(false, &region, 100, 100), PublishKind::Nothing);
        assert_eq!(publish_kind(true, &region, 100, 100), PublishKind::Full);
        region.add(rect(0, 0, 10, 10));
        assert_eq!(publish_kind(false, &region, 100, 100), PublishKind::Patch);
        region.add(rect(0, 0, 100, 59));
        assert_eq!(publish_kind(false, &region, 100, 100), PublishKind::Patch);
        region.add(rect(0, 0, 100, 60));
        assert_eq!(publish_kind(false, &region, 100, 100), PublishKind::Full);
        let mut outside = DirtyRegion::default();
        outside.add(rect(90, 90, 20, 20));
        assert_eq!(publish_kind(false, &outside, 100, 100), PublishKind::Full);
    }

    #[test]
    fn presentation_coalesces_updates_into_frame_then_patches() {
        let (sink, recorded) = recording_sink();
        let image = DecodedImage::new(PixelFormat::RgbA32, 64, 32);
        let mut presentation = Presentation::default();
        assert!(presentation.due().is_none());
        presentation.mark(Some(rect(1, 1, 2, 2)));
        // The first frame is due immediately and must be complete.
        assert!(presentation.due().unwrap() <= tokio::time::Instant::now());
        presentation.flush(&image, &sink).unwrap();
        assert!(presentation.due().is_none());
        presentation.mark(Some(rect(1, 1, 2, 2)));
        presentation.mark(Some(rect(40, 20, 3, 1)));
        let due = presentation.due().unwrap();
        assert!(due > tokio::time::Instant::now());
        presentation.flush(&image, &sink).unwrap();
        presentation.mark(None);
        presentation.flush(&image, &sink).unwrap();
        presentation.reset_after_activation();
        assert!(presentation.due().unwrap() <= tokio::time::Instant::now());
        let events = recorded.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], EngineEvent::Frame(frame) if frame.rgba.len() == 64 * 32 * 4));
        let EngineEvent::FramePatch(patch) = &events[1] else {
            panic!("expected a patch");
        };
        assert_eq!((patch.width, patch.height), (64, 32));
        assert_eq!(patch.rects.len(), 2);
        for (rect, rgba) in &patch.rects {
            assert_eq!(rgba.len(), rect.area() as usize * 4);
        }
        assert!(matches!(&events[2], EngineEvent::Frame(_)));
    }

    #[test]
    fn size_change_forces_full_frame() {
        let (sink, recorded) = recording_sink();
        let mut presentation = Presentation::default();
        presentation.mark(Some(rect(0, 0, 1, 1)));
        presentation
            .flush(&DecodedImage::new(PixelFormat::RgbA32, 8, 8), &sink)
            .unwrap();
        presentation.mark(Some(rect(0, 0, 1, 1)));
        presentation
            .flush(&DecodedImage::new(PixelFormat::RgbA32, 16, 8), &sink)
            .unwrap();
        let events = recorded.lock().unwrap();
        assert!(matches!(&events[1], EngineEvent::Frame(frame) if frame.width == 16));
    }

    #[test]
    fn pointer_conversion_keeps_straight_rgba_and_falls_back_safely() {
        let pointer = DecodedPointer {
            width: 2,
            height: 1,
            hotspot_x: 5,
            hotspot_y: 0,
            bitmap_data: vec![10, 20, 30, 128, 0, 0, 0, 0],
        };
        let DesktopCursor::Bitmap(bitmap) = cursor_from_pointer(&pointer) else {
            panic!("expected a bitmap cursor");
        };
        assert_eq!(bitmap.rgba, pointer.bitmap_data);
        assert_eq!((bitmap.hotspot_x, bitmap.hotspot_y), (1, 0));
        assert_eq!(
            cursor_from_pointer(&DecodedPointer::new_invisible()),
            DesktopCursor::Hidden
        );
        let oversized = DecodedPointer {
            width: 400,
            height: 1,
            hotspot_x: 0,
            hotspot_y: 0,
            bitmap_data: vec![0; 1600],
        };
        assert_eq!(cursor_from_pointer(&oversized), DesktopCursor::Default);
        let truncated = DecodedPointer {
            width: 2,
            height: 2,
            hotspot_x: 0,
            hotspot_y: 0,
            bitmap_data: vec![0; 3],
        };
        assert_eq!(cursor_from_pointer(&truncated), DesktopCursor::Default);
    }

    #[test]
    fn identical_cursors_are_sent_once() {
        let (sink, recorded) = recording_sink();
        let mut presentation = Presentation::default();
        let pointer = Arc::new(DecodedPointer {
            width: 1,
            height: 1,
            hotspot_x: 0,
            hotspot_y: 0,
            bitmap_data: vec![1, 2, 3, 255],
        });
        let same_pixels = Arc::new(DecodedPointer {
            width: 1,
            height: 1,
            hotspot_x: 0,
            hotspot_y: 0,
            bitmap_data: vec![1, 2, 3, 255],
        });
        for output in [
            ActiveStageOutput::PointerBitmap(pointer.clone()),
            ActiveStageOutput::PointerBitmap(pointer.clone()),
            ActiveStageOutput::PointerBitmap(same_pixels),
            ActiveStageOutput::PointerPosition { x: 3, y: 4 },
            ActiveStageOutput::PointerHidden,
            ActiveStageOutput::PointerHidden,
            ActiveStageOutput::PointerBitmap(pointer),
            ActiveStageOutput::PointerDefault,
        ] {
            presentation.set_cursor(&output, &sink);
        }
        let kinds: Vec<_> = recorded
            .lock()
            .unwrap()
            .iter()
            .map(|event| match event {
                EngineEvent::Cursor(DesktopCursor::Bitmap(_)) => "bitmap",
                EngineEvent::Cursor(DesktopCursor::Hidden) => "hidden",
                EngineEvent::Cursor(DesktopCursor::Default) => "default",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["bitmap", "hidden", "bitmap", "default"]);
    }

    #[test]
    fn rdp_uses_local_pointer_rendering() {
        let (_audio_muted_tx, audio_muted) = tokio::sync::watch::channel(AudioMuteState::default());
        let options = RdpOptions {
            server_name: "rdp.test".into(),
            udp_peer: None,
            transport_mode: TransportMode::Auto,
            graphics_mode: GraphicsMode::Auto,
            username: "user".into(),
            domain: None,
            password: Zeroizing::new(String::new()),
            width: 800,
            height: 600,
            clipboard_enabled: false,
            audio_playback_enabled: false,
            audio_muted,
            certificate_approval: None,
        };
        let config = config(&options);
        assert!(!config.pointer_software_rendering);
        assert!(config.enable_server_pointer);
    }

    #[test]
    fn decoding_runs_inline_without_multi_thread_runtime() {
        assert_eq!(decode_in_place(|| 7), 7);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        assert_eq!(
            runtime
                .block_on(async { tokio::spawn(async { decode_in_place(|| 9) }).await })
                .unwrap(),
            9
        );
    }

    #[test]
    fn forced_avc_rejects_legacy_bitmap_before_reporting_a_frame() {
        let outputs = [ActiveStageOutput::GraphicsUpdate(
            ironrdp::pdu::geometry::InclusiveRectangle {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            },
        )];
        let unavailable = AtomicBool::new(false);
        let seen = AtomicBool::new(false);
        let actual = AtomicU8::new(0);
        let reported = Arc::new(Mutex::new(Vec::<RdpGraphicsActual>::new()));
        let sink: EventSink = {
            let reported = reported.clone();
            Arc::new(move |event| {
                if let EngineEvent::RdpGraphics(actual) = event {
                    reported.lock().unwrap().push(actual);
                }
            })
        };
        assert!(matches!(
            check_graphics_mode(&outputs, &unavailable, &seen, &actual, true, &sink),
            Err(EngineError::RdpGraphicsUnavailable)
        ));
        assert!(!seen.load(Ordering::Relaxed));
        assert!(reported.lock().unwrap().is_empty());
        check_graphics_mode(&outputs, &unavailable, &seen, &actual, false, &sink).unwrap();
        assert_eq!(*reported.lock().unwrap(), [RdpGraphicsActual::Bitmap]);
    }
    #[test]
    fn rdp_config_advertises_legacy_wan_graphics_only() {
        use ironrdp::pdu::{gcc::ConnectionType, rdp::capability_sets::CodecProperty};

        let (_audio_muted_tx, audio_muted) = tokio::sync::watch::channel(AudioMuteState::default());
        let options = RdpOptions {
            server_name: "rdp.test".into(),
            udp_peer: None,
            transport_mode: TransportMode::Auto,
            graphics_mode: GraphicsMode::Auto,
            username: "user".into(),
            domain: None,
            password: Zeroizing::new(String::new()),
            width: 800,
            height: 600,
            clipboard_enabled: false,
            audio_playback_enabled: false,
            audio_muted,
            certificate_approval: None,
        };
        let config = config(&options);
        assert_eq!(config.connection_type, ConnectionType::Wan);
        let bitmap = config.bitmap.expect("bitmap capability");
        assert_eq!(bitmap.color_depth, 32);
        assert!(bitmap.lossy_compression);
        assert_eq!(bitmap.codecs.0.len(), 1);
        assert!(matches!(
            bitmap.codecs.0[0].property,
            CodecProperty::RemoteFx(_)
        ));
        assert!(config.compression_type.is_none());
        assert!(config.multitransport_flags.is_none());
        assert!(config.support_dyn_vc_gfx_protocol);
    }

    #[test]
    fn direct_rdp_advertises_only_reliable_udp_and_soft_sync() {
        let (_audio_muted_tx, audio_muted) = tokio::sync::watch::channel(AudioMuteState::default());
        let mut options = RdpOptions {
            server_name: "rdp.test".into(),
            udp_peer: Some("127.0.0.1:3389".parse().unwrap()),
            transport_mode: TransportMode::Auto,
            graphics_mode: GraphicsMode::Auto,
            username: "user".into(),
            domain: None,
            password: Zeroizing::new(String::new()),
            width: 800,
            height: 600,
            clipboard_enabled: false,
            audio_playback_enabled: false,
            audio_muted,
            certificate_approval: None,
        };
        let flags = config(&options).multitransport_flags.unwrap();
        assert_eq!(
            flags,
            ironrdp::pdu::gcc::MultiTransportFlags::TRANSPORT_TYPE_UDP_FECR
                | ironrdp::pdu::gcc::MultiTransportFlags::SOFT_SYNC_TCP_TO_UDP
        );
        options.transport_mode = TransportMode::TcpOnly;
        assert!(config(&options).multitransport_flags.is_none());
        options.graphics_mode = GraphicsMode::Bitmap;
        let bitmap = config(&options);
        assert!(!bitmap.support_dyn_vc_gfx_protocol);
        assert!(
            bitmap
                .bitmap
                .expect("bitmap capability")
                .codecs
                .0
                .is_empty()
        );
    }

    #[test]
    fn engine_future_is_send_for_core_actor() {
        fn assert_send<T: Send>(_: T) {}
        let (io, _peer) = tokio::io::duplex(64);
        let (_tx, rx) = mpsc::channel(1);
        let (_stop, stop) = tokio::sync::watch::channel(false);
        let (_focus, focus_epoch) = tokio::sync::watch::channel(0);
        let (_audio_muted_tx, audio_muted) = tokio::sync::watch::channel(AudioMuteState::default());
        let options = RdpOptions {
            server_name: "rdp.test".into(),
            udp_peer: None,
            transport_mode: TransportMode::Auto,
            graphics_mode: GraphicsMode::Auto,
            username: "user".into(),
            domain: None,
            password: Zeroizing::new("secret".into()),
            width: 800,
            height: 600,
            clipboard_enabled: false,
            audio_playback_enabled: false,
            audio_muted,
            certificate_approval: None,
        };
        assert_send(run(
            Box::new(io),
            options,
            rx,
            EngineControl { stop, focus_epoch },
            Arc::new(|_| {}),
        ));
    }
    #[test]
    fn extended_scancode_and_pointer_release_are_tracked() {
        let mut db = Database::new();
        let key = DesktopInput::Key {
            scan_code: 0x11d,
            keysym: 0xffe4,
            down: true,
        };
        let events = db.apply(operations(&key, 800, 600).unwrap());
        assert_eq!(events.len(), 1);
        assert!(db.is_key_pressed(Scancode::from_u8(true, 0x1d)));
        db.apply(
            operations(
                &DesktopInput::Pointer {
                    x: 900,
                    y: 700,
                    buttons: 1,
                },
                800,
                600,
            )
            .unwrap(),
        );
        assert_eq!(db.mouse_position(), MousePosition { x: 799, y: 599 });
        assert_eq!(db.release_all().len(), 2);
        assert!(db.release_all().is_empty());
    }
    #[tokio::test]
    async fn stopped_engine_does_not_start_handshake() {
        let (io, mut peer) = tokio::io::duplex(64);
        let (_tx, rx) = mpsc::channel(1);
        let (_stop, stop) = tokio::sync::watch::channel(true);
        let (_focus, focus_epoch) = tokio::sync::watch::channel(0);
        let (_audio_muted_tx, audio_muted) = tokio::sync::watch::channel(AudioMuteState::default());
        let options = RdpOptions {
            server_name: "rdp.test".into(),
            udp_peer: None,
            transport_mode: TransportMode::Auto,
            graphics_mode: GraphicsMode::Auto,
            username: "user".into(),
            domain: None,
            password: Zeroizing::new("secret".into()),
            width: 800,
            height: 600,
            clipboard_enabled: false,
            audio_playback_enabled: false,
            audio_muted,
            certificate_approval: None,
        };
        assert_eq!(
            run(
                Box::new(io),
                options,
                rx,
                EngineControl { stop, focus_epoch },
                Arc::new(|_| {})
            )
            .await,
            Err(EngineError::Cancelled)
        );
        use tokio::io::AsyncReadExt;
        assert_eq!(peer.read(&mut [0; 64]).await.unwrap(), 0);
    }
}
