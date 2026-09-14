//! Core-owned RDP engine. Consumes an existing transport without opening DNS, KDC, or other network connections.
//! TLS signature verification and one-shot approval of unknown certificates precede transmission of credentials.
mod audio;
mod clipboard;
mod limits;
mod tls;
mod wire;
use ironrdp::{
    connector::{self, ClientConnector, ClientConnectorState, Credentials},
    core::WriteBuf,
    graphics::image_processing::PixelFormat,
    input::{Database, MouseButton, MousePosition, Operation, Scancode, WheelRotations},
    session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput, image::DecodedImage},
};
use norishell_desktop_protocol::{
    AudioMuteState, BoxedDesktopIo, DesktopFrame, DesktopInput, EngineCommand, EngineControl,
    EngineError, EngineEvent, EventSink, Result, frame_len,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
pub use tls::{CertificateApproval, CertificateChallenge};
use tokio::sync::mpsc;
use wire::{Wire, connector_error};
use zeroize::Zeroizing;

pub struct RdpOptions {
    pub server_name: String,
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
    if options.server_name.is_empty()
        || options.username.is_empty()
        || options.username.len() > 1024
        || options.domain.as_ref().is_some_and(|s| s.len() > 1024)
        || options.password.len() > 65536
    {
        return Err(EngineError::InvalidConfiguration);
    }
    let clip = Arc::new(Mutex::new(clipboard::State::default()));
    let (mut wire, result) = tokio::time::timeout(
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
        pointer_software_rendering: result.pointer_software_rendering,
    }
    .build();
    let mut input = Database::new();
    (events)(EngineEvent::Ready);
    loop {
        tokio::select! { biased;
            changed=control.focus_epoch.changed()=> {
                if changed.is_err() { return Err(EngineError::Cancelled); }
                let released=input.release_all();
                for chunk in released.chunks(15) {
                    let outputs=active.process_fastpath_input(&mut image,chunk).map_err(|_|EngineError::Protocol)?;
                    process_outputs(outputs,&mut wire,&mut image,&mut active,&activation,&events).await?;
                }
            }
            command=commands.recv()=> {
                let Some(command)=command else { return Ok(()); };
                if !control.accepts(&command) { let _=command.completion.send(Err(EngineError::StaleInput)); continue; }
                if let Err(error)=command.input.validate() { let _=command.completion.send(Err(error)); continue; }
                let result=send_input(&command,control,&mut input,&mut active,&mut image,&mut wire,&clip,&events,clipboard_enabled).await;
                let fatal=matches!(result,Err(EngineError::ConnectionLost|EngineError::Timeout|EngineError::Protocol));
                let _=command.completion.send(result);
                if fatal { return result; }
            }
            packet=wire.pdu()=> {
                let (action,bytes)=packet?;
                limits.check(action,&bytes)?;
                let outputs=active.process(&mut image,action,&bytes).map_err(|_|EngineError::Protocol)?;
                if process_outputs(outputs,&mut wire,&mut image,&mut active,&activation,&events).await? { return Ok(()); }
                flush_clipboard(&mut active,&mut wire,&clip).await?;
            }
        }
    }
}

async fn connect(
    stream: BoxedDesktopIo,
    options: &RdpOptions,
    clip: Arc<Mutex<clipboard::State>>,
    events: EventSink,
    audio: Option<Arc<audio::AudioPlayback>>,
) -> Result<(Wire, connector::ConnectionResult)> {
    let mut connector = ClientConnector::new(
        config(options),
        "0.0.0.0:0"
            .parse()
            .map_err(|_| EngineError::InvalidConfiguration)?,
    );
    connector.attach_static_channel(ironrdp::dvc::DrdynvcClient::new().with_dynamic_channel(
        ironrdp::displaycontrol::client::DisplayControlClient::new(|_| Ok(Vec::new())),
    ));
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
    let (stream, key) = tls::upgrade(
        wire.stream,
        &options.server_name,
        options.certificate_approval.as_ref(),
    )
    .await?;
    let mut wire = Wire::new(stream);
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
        wire.step(&mut connector).await?;
        if let ClientConnectorState::Connected { mut result } = connector.state {
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
            return Ok((wire, result));
        }
    }
}

fn config(options: &RdpOptions) -> connector::Config {
    use ironrdp::pdu::{
        gcc::KeyboardType,
        rdp::{
            capability_sets::MajorPlatformType,
            client_info::{PerformanceFlags, TimezoneInfo},
        },
    };
    connector::Config {
        credentials: Credentials::UsernamePassword {
            username: options.username.clone(),
            password: options.password.to_string(),
        },
        domain: options.domain.clone(),
        enable_tls: true,
        enable_credssp: true,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: 0,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: connector::DesktopSize {
            width: options.width,
            height: options.height,
        },
        bitmap: None,
        client_build: 0,
        client_name: "NoriShell".into(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".into(),
        platform: MajorPlatformType::UNSPECIFIED,
        enable_server_pointer: true,
        request_data: None,
        autologon: true,
        enable_audio_playback: options.audio_playback_enabled,
        compression_type: None,
        pointer_software_rendering: true,
        multitransport_flags: None,
        performance_flags: PerformanceFlags::default(),
        desktop_scale_factor: 0,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        alternate_shell: String::new(),
        work_dir: String::new(),
    }
}

fn publish(image: &DecodedImage, events: &EventSink) -> Result<()> {
    let expected = frame_len(image.width(), image.height())?;
    if image.data().len() != expected {
        return Err(EngineError::Protocol);
    }
    events(EngineEvent::Frame(Arc::new(DesktopFrame {
        width: image.width(),
        height: image.height(),
        rgba: image.data().to_vec(),
    })));
    Ok(())
}

async fn process_outputs(
    outputs: Vec<ActiveStageOutput>,
    wire: &mut Wire,
    image: &mut DecodedImage,
    active: &mut ActiveStage,
    activation: &connector::connection_activation::ConnectionActivationFactory,
    events: &EventSink,
) -> Result<bool> {
    let mut changed = false;
    for output in outputs {
        match output {
            ActiveStageOutput::ResponseFrame(frame) => wire.write(&frame).await?,
            ActiveStageOutput::GraphicsUpdate(_) => changed = true,
            ActiveStageOutput::Terminate(_) => return Ok(true),
            ActiveStageOutput::DeactivateAll => {
                let mut sequence = activation.create();
                loop {
                    wire.step(&mut sequence).await?;
                    if let connector::connection_activation::ConnectionActivationState::Finalized { desktop_size,share_id,enable_server_pointer,pointer_software_rendering }=sequence.connection_activation_state() {
                        frame_len(desktop_size.width,desktop_size.height)?;
                        *image=DecodedImage::new(PixelFormat::RgbA32,desktop_size.width,desktop_size.height);
                        active.set_share_id(share_id);
                        active.set_enable_server_pointer(enable_server_pointer);
                        active.set_fastpath_processor(ironrdp::session::fast_path::ProcessorBuilder { io_channel_id:activation.io_channel_id(),user_channel_id:activation.user_channel_id(),share_id,bulk_decompressor:None,enable_server_pointer,pointer_software_rendering }.build());
                        changed=true; break;
                    }
                }
            }
            _ => {}
        }
    }
    if changed {
        publish(image, events)?;
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
async fn send_input(
    command: &EngineCommand,
    control: &EngineControl,
    input: &mut Database,
    active: &mut ActiveStage,
    image: &mut DecodedImage,
    wire: &mut Wire,
    clip: &Arc<Mutex<clipboard::State>>,
    events: &EventSink,
    clipboard_enabled: bool,
) -> Result<()> {
    if !control.accepts(command) {
        return Err(EngineError::StaleInput);
    }
    match &command.input {
        DesktopInput::Resize { width, height } => {
            if *width < 200 || *height < 200 || width % 2 != 0 {
                return Err(EngineError::InvalidConfiguration);
            }
            let bytes = active
                .encode_resize(u32::from(*width), u32::from(*height), None, None)
                .ok_or(EngineError::UnsupportedOperation)?
                .map_err(|_| EngineError::Protocol)?;
            if !control.accepts(command) {
                return Err(EngineError::StaleInput);
            }
            wire.write_fenced(&bytes, control, command.focus_epoch)
                .await
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
            flush_clipboard_fenced(active, wire, clip, Some((control, command.focus_epoch))).await
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
                for output in outputs {
                    match output {
                        ActiveStageOutput::ResponseFrame(bytes) => {
                            if !control.accepts(command) {
                                return Err(EngineError::StaleInput);
                            }
                            wire.write_fenced(&bytes, control, command.focus_epoch)
                                .await?;
                        }
                        ActiveStageOutput::GraphicsUpdate(_) => publish(image, events)?,
                        _ => {}
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
    fn engine_future_is_send_for_core_actor() {
        fn assert_send<T: Send>(_: T) {}
        let (io, _peer) = tokio::io::duplex(64);
        let (_tx, rx) = mpsc::channel(1);
        let (_stop, stop) = tokio::sync::watch::channel(false);
        let (_focus, focus_epoch) = tokio::sync::watch::channel(0);
        let (_audio_muted_tx, audio_muted) = tokio::sync::watch::channel(AudioMuteState::default());
        let options = RdpOptions {
            server_name: "rdp.test".into(),
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
