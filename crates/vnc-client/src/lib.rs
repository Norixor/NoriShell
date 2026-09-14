//! NoriShell's shared-contract adapter around the hardened vendored `vnc-rs`
//! transport. Core receives bounded RGBA snapshots and stable public errors;
//! upstream RFB events, server error strings, and password handling stay here.
#![forbid(unsafe_code)]

use std::{collections::BTreeSet, io::Cursor, sync::Arc, time::Duration};

use jpeg_decoder::{Decoder as JpegDecoder, PixelFormat as JpegPixelFormat};
use norishell_desktop_protocol::{
    BoxedDesktopIo, DesktopFrame, DesktopInput, EngineCommand, EngineControl, EngineError,
    EngineEvent, EventSink, Result, frame_len,
};
use tokio::{
    sync::{mpsc, watch},
    time::timeout,
};
use vnc::{
    ClientKeyEvent, ClientMouseEvent, HardenedVncClient, HardenedVncOptions, Rect, VncError,
    VncEvent, X11Event,
};
use zeroize::Zeroizing;

const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_WHEEL_STEPS: usize = 120;
const MAX_JPEG_BYTES: usize = 32 * 1024 * 1024;

/// VNC's legacy challenge-response password is never cloned into UI state or
/// error values. `allow_unauthenticated` is false by caller choice by default.
pub struct VncOptions {
    pub password: Zeroizing<String>,
    pub allow_unauthenticated: bool,
    pub clipboard_enabled: bool,
}

/// Runs one already-connected VNC RFB session.
///
/// The adapter intentionally does not negotiate cursor pseudo-encoding or
/// client-driven desktop resize: `DesktopFrame` has no cursor-shape channel,
/// and RFB ExtendedDesktopSize has a distinct negotiation contract. Server
/// DesktopSize updates are handled and bounded.
pub async fn run(
    stream: BoxedDesktopIo,
    options: VncOptions,
    mut commands: mpsc::Receiver<EngineCommand>,
    mut control: EngineControl,
    events: EventSink,
) -> Result<()> {
    let VncOptions {
        password,
        allow_unauthenticated,
        clipboard_enabled,
    } = options;
    let mut client = HardenedVncClient::connect(
        stream,
        HardenedVncOptions {
            password,
            allow_unauthenticated,
            clipboard_enabled,
        },
        control.stop.clone(),
    )
    .await
    .map_err(map_error)?;

    events(EngineEvent::Ready);
    let result = run_connected(
        &mut client,
        &mut commands,
        &mut control,
        clipboard_enabled,
        Arc::clone(&events),
    )
    .await;
    client.shutdown().await;
    let queued_error = result
        .as_ref()
        .err()
        .copied()
        .unwrap_or(EngineError::Cancelled);
    complete_queued_commands(&mut commands, queued_error);
    result
}

async fn run_connected(
    client: &mut HardenedVncClient,
    commands: &mut mpsc::Receiver<EngineCommand>,
    control: &mut EngineControl,
    clipboard_enabled: bool,
    events: EventSink,
) -> Result<()> {
    let mut frame: Option<DesktopFrame> = None;
    let mut dirty = false;
    let mut input = InputState::default();
    let mut commands_open = true;
    let mut focus_open = true;

    loop {
        tokio::select! {
            biased;
            () = stopped(&mut control.stop) => {
                release_all(client, &mut input).await;
                return Err(EngineError::Cancelled);
            }
            changed = control.focus_epoch.changed(), if focus_open => {
                if changed.is_err() {
                    focus_open = false;
                }
                release_all(client, &mut input).await;
            }
            server_event = client.next_event() => {
                let server_event = server_event.map_err(map_error)?;
                handle_server_event(server_event, &mut frame, &mut dirty, &events)?;
            }
            command = commands.recv(), if commands_open => {
                match command {
                    Some(command) => {
                        handle_command(client, command, control, clipboard_enabled, &mut input).await;
                    }
                    None => commands_open = false,
                }
            }
        }
    }
}

fn handle_server_event(
    event: VncEvent,
    frame: &mut Option<DesktopFrame>,
    dirty: &mut bool,
    sink: &EventSink,
) -> Result<()> {
    match event {
        VncEvent::SetResolution(screen) => {
            *frame = Some(DesktopFrame::new(screen.width, screen.height)?);
            *dirty = true;
        }
        VncEvent::RawImage(rect, bytes) => {
            let image = frame.as_mut().ok_or(EngineError::Protocol)?;
            image.write_rect(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                &bgra_to_rgba(rect, &bytes)?,
            )?;
            *dirty = true;
        }
        VncEvent::Copy(destination, source) => {
            let image = frame.as_mut().ok_or(EngineError::Protocol)?;
            image.copy_rect(
                source.x,
                source.y,
                destination.x,
                destination.y,
                destination.width,
                destination.height,
            )?;
            *dirty = true;
        }
        VncEvent::JpegImage(rect, bytes) => {
            let image = frame.as_mut().ok_or(EngineError::Protocol)?;
            image.write_rect(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                &decode_jpeg(rect, &bytes)?,
            )?;
            *dirty = true;
        }
        VncEvent::FramebufferUpdateComplete => {
            if *dirty {
                let image = frame.as_ref().ok_or(EngineError::Protocol)?;
                // EventSink is Core's one-frame replacement callback. The
                // adapter never queues snapshots or retains prior display frames.
                sink(EngineEvent::Frame(Arc::new(DesktopFrame {
                    width: image.width,
                    height: image.height,
                    rgba: image.rgba.clone(),
                })));
                *dirty = false;
            }
        }
        VncEvent::Text(text) => sink(EngineEvent::Clipboard(text)),
        VncEvent::Bell => {}
        // The hardened vendor path never emits these for this negotiation;
        // retain failure-closed behavior if that invariant changes.
        VncEvent::SetPixelFormat(_) | VncEvent::SetCursor(_, _) => {
            return Err(EngineError::UnsupportedOperation);
        }
        _ => return Err(EngineError::UnsupportedOperation),
    }
    Ok(())
}

async fn handle_command(
    client: &mut HardenedVncClient,
    command: EngineCommand,
    control: &mut EngineControl,
    clipboard_enabled: bool,
    input: &mut InputState,
) {
    if !control.accepts(&command) || focus_changed(control) {
        let _ = command.completion.send(Err(rejection_for(control)));
        return;
    }
    if let Err(error) = command.input.validate() {
        let _ = command.completion.send(Err(error));
        return;
    }

    let planned = match input.plan(&command.input, clipboard_enabled) {
        Ok(planned) => planned,
        Err(error) => {
            let _ = command.completion.send(Err(error));
            return;
        }
    };
    if !control.accepts(&command) || focus_changed(control) {
        let _ = command.completion.send(Err(rejection_for(control)));
        return;
    }

    enum Outcome {
        Written(std::result::Result<(), VncError>),
        Stopped,
        FocusChanged,
    }
    let outcome = {
        let write = dispatch_input(client, planned.priority, planned.events);
        tokio::pin!(write);
        tokio::select! {
            biased;
            () = stopped(&mut control.stop) => Outcome::Stopped,
            changed = control.focus_epoch.changed() => {
                let _ = changed;
                Outcome::FocusChanged
            }
            result = &mut write => Outcome::Written(result),
        }
    };

    match outcome {
        Outcome::Written(Ok(())) => {
            input.settle_success(&command.input);
            let _ = command.completion.send(Ok(()));
        }
        Outcome::Written(Err(error)) => {
            let _ = command.completion.send(Err(map_error(error)));
        }
        Outcome::Stopped => {
            release_all(client, input).await;
            let _ = command.completion.send(Err(EngineError::Cancelled));
        }
        Outcome::FocusChanged => {
            release_all(client, input).await;
            let _ = command.completion.send(Err(EngineError::StaleInput));
        }
    }
}

async fn dispatch_input(
    client: &HardenedVncClient,
    priority: bool,
    events: Vec<X11Event>,
) -> std::result::Result<(), VncError> {
    if priority {
        client.priority_input(events).await
    } else {
        client.input(events).await
    }
}

fn focus_changed(control: &EngineControl) -> bool {
    control.focus_epoch.has_changed().unwrap_or(true)
}

fn rejection_for(control: &EngineControl) -> EngineError {
    if *control.stop.borrow() {
        EngineError::Cancelled
    } else {
        EngineError::StaleInput
    }
}

async fn release_all(client: &mut HardenedVncClient, input: &mut InputState) {
    let release = input.release_events();
    input.clear();
    if release.is_empty() {
        return;
    }
    let _ = timeout(CONTROL_WRITE_TIMEOUT, client.priority_input(release)).await;
}

fn complete_queued_commands(commands: &mut mpsc::Receiver<EngineCommand>, error: EngineError) {
    while let Ok(command) = commands.try_recv() {
        let _ = command.completion.send(Err(error));
    }
}

fn bgra_to_rgba(rect: Rect, bytes: &[u8]) -> Result<Vec<u8>> {
    let expected = frame_len(rect.width, rect.height)?;
    if bytes.len() != expected {
        return Err(EngineError::Protocol);
    }
    let mut rgba = Vec::with_capacity(expected);
    for pixel in bytes.chunks_exact(4) {
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    Ok(rgba)
}

fn decode_jpeg(rect: Rect, bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() > MAX_JPEG_BYTES {
        return Err(EngineError::ResourceLimit);
    }
    let mut decoder = JpegDecoder::new(Cursor::new(bytes));
    // Parse dimensions before JPEG allocates its decoded planes. The RFB
    // rectangle has already passed the vendor bounds check, so a mismatch is
    // a protocol error rather than an image we should scale or crop.
    decoder.read_info().map_err(|_| EngineError::Protocol)?;
    let info = decoder.info().ok_or(EngineError::Protocol)?;
    if info.width != rect.width || info.height != rect.height {
        return Err(EngineError::Protocol);
    }
    let target_len = frame_len(rect.width, rect.height)?;
    match info.pixel_format {
        JpegPixelFormat::RGB24 | JpegPixelFormat::L8 => {}
        JpegPixelFormat::L16 | JpegPixelFormat::CMYK32 => {
            return Err(EngineError::UnsupportedOperation);
        }
    }
    // jpeg-decoder limits its decoded component planes by this value. RGBA
    // output needs four bytes per RFB pixel, which bounds all supported JPEG
    // source formats at the negotiated frame size.
    decoder.set_max_decoding_buffer_size(target_len);
    let pixels = decoder.decode().map_err(|_| EngineError::Protocol)?;
    let mut rgba = Vec::with_capacity(target_len);
    match info.pixel_format {
        JpegPixelFormat::RGB24 => {
            if pixels.len() != usize::from(rect.width) * usize::from(rect.height) * 3 {
                return Err(EngineError::Protocol);
            }
            for pixel in pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        JpegPixelFormat::L8 => {
            if pixels.len() != usize::from(rect.width) * usize::from(rect.height) {
                return Err(EngineError::Protocol);
            }
            for value in pixels {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        JpegPixelFormat::L16 | JpegPixelFormat::CMYK32 => unreachable!(),
    }
    if rgba.len() != target_len {
        return Err(EngineError::Protocol);
    }
    Ok(rgba)
}

fn map_error(error: VncError) -> EngineError {
    match error {
        VncError::AuthenticationRejected => EngineError::AuthenticationRejected,
        VncError::UnsupportedAuthentication => EngineError::UnsupportedAuthentication,
        VncError::UnsupportedOperation => EngineError::UnsupportedOperation,
        VncError::ResourceLimit => EngineError::ResourceLimit,
        VncError::ConnectionLost | VncError::Closed => EngineError::ConnectionLost,
        VncError::Timeout => EngineError::Timeout,
        VncError::Cancelled => EngineError::Cancelled,
        VncError::Protocol | VncError::WrongPixelFormat | VncError::InvalidImageData => {
            EngineError::Protocol
        }
        _ => EngineError::Protocol,
    }
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

#[derive(Default)]
struct InputState {
    pressed: BTreeSet<u32>,
    pointer: PointerState,
}

#[derive(Default)]
struct PointerState {
    x: u16,
    y: u16,
    buttons: u8,
}

struct PlannedInput {
    events: Vec<X11Event>,
    priority: bool,
}

impl InputState {
    fn plan(&mut self, input: &DesktopInput, clipboard_enabled: bool) -> Result<PlannedInput> {
        match input {
            DesktopInput::Key { keysym, down, .. } => {
                if *down {
                    // Record before the write starts so a focus change can
                    // release a partially dispatched key-down.
                    self.pressed.insert(*keysym);
                }
                Ok(PlannedInput {
                    events: vec![key_event(*keysym, *down)],
                    priority: false,
                })
            }
            DesktopInput::Pointer { x, y, buttons } => {
                self.pointer.x = *x;
                self.pointer.y = *y;
                // Preserve both old and requested buttons until the writer
                // acknowledges the transition; either may have reached peer.
                self.pointer.buttons |= *buttons;
                Ok(PlannedInput {
                    events: vec![pointer_event(*x, *y, *buttons)],
                    priority: false,
                })
            }
            DesktopInput::Wheel {
                x,
                y,
                delta_x,
                delta_y,
            } => {
                self.pointer.x = *x;
                self.pointer.y = *y;
                let mut events = Vec::new();
                append_wheel(&mut events, *x, *y, *delta_x, 0x20, 0x40)?;
                append_wheel(&mut events, *x, *y, *delta_y, 0x08, 0x10)?;
                Ok(PlannedInput {
                    events,
                    priority: false,
                })
            }
            DesktopInput::Text(text) => {
                let mut events = Vec::with_capacity(text.chars().count() * 2);
                for character in text.chars() {
                    let keysym = unicode_keysym(character);
                    events.push(key_event(keysym, true));
                    events.push(key_event(keysym, false));
                }
                Ok(PlannedInput {
                    events,
                    priority: false,
                })
            }
            DesktopInput::Clipboard(text) => {
                if !clipboard_enabled || !text.chars().all(|character| character <= '\u{ff}') {
                    return Err(EngineError::UnsupportedOperation);
                }
                Ok(PlannedInput {
                    events: vec![X11Event::CopyText(text.clone())],
                    priority: false,
                })
            }
            DesktopInput::Resize { .. } => Err(EngineError::UnsupportedOperation),
            DesktopInput::ReleaseAll => Ok(PlannedInput {
                events: self.release_events(),
                priority: true,
            }),
        }
    }

    fn settle_success(&mut self, input: &DesktopInput) {
        match input {
            DesktopInput::Key {
                keysym,
                down: false,
                ..
            } => {
                self.pressed.remove(keysym);
            }
            DesktopInput::Pointer { x, y, buttons } => {
                self.pointer = PointerState {
                    x: *x,
                    y: *y,
                    buttons: *buttons,
                };
            }
            DesktopInput::ReleaseAll => self.clear(),
            _ => {}
        }
    }

    fn release_events(&self) -> Vec<X11Event> {
        let mut events = self
            .pressed
            .iter()
            .copied()
            .map(|keysym| key_event(keysym, false))
            .collect::<Vec<_>>();
        if self.pointer.buttons != 0 {
            events.push(pointer_event(self.pointer.x, self.pointer.y, 0));
        }
        events
    }

    fn clear(&mut self) {
        self.pressed.clear();
        self.pointer.buttons = 0;
    }
}

fn key_event(keysym: u32, down: bool) -> X11Event {
    X11Event::KeyEvent(ClientKeyEvent {
        keycode: keysym,
        down,
    })
}

fn pointer_event(x: u16, y: u16, buttons: u8) -> X11Event {
    X11Event::PointerEvent(ClientMouseEvent {
        position_x: x,
        position_y: y,
        bottons: buttons,
    })
}

fn append_wheel(
    events: &mut Vec<X11Event>,
    x: u16,
    y: u16,
    delta: i16,
    negative_button: u8,
    positive_button: u8,
) -> Result<()> {
    let steps = usize::from(delta.unsigned_abs());
    if steps > MAX_WHEEL_STEPS {
        return Err(EngineError::ResourceLimit);
    }
    let button = if delta < 0 {
        negative_button
    } else {
        positive_button
    };
    for _ in 0..steps {
        events.push(pointer_event(x, y, button));
        events.push(pointer_event(x, y, 0));
    }
    Ok(())
}

fn unicode_keysym(character: char) -> u32 {
    let codepoint = u32::from(character);
    if codepoint <= 0xff {
        codepoint
    } else {
        0x0100_0000 | codepoint
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        sync::oneshot,
        time::{sleep, timeout},
    };

    const TEST_TIMEOUT: Duration = Duration::from_secs(2);

    #[test]
    fn jpeg_is_preflighted_and_converted_to_rgba() {
        let jpeg = include_bytes!("testdata/red-1x1.jpg");
        let rgba = decode_jpeg(
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            jpeg,
        )
        .unwrap();
        assert_eq!(rgba.len(), 4);
        assert!(rgba[0] > 200);
        assert!(rgba[1] < 32);
        assert!(rgba[2] < 32);
        assert_eq!(rgba[3], 255);
    }

    #[test]
    fn jpeg_dimensions_must_match_the_rfb_rectangle() {
        let jpeg = include_bytes!("testdata/red-1x1.jpg");
        assert_eq!(
            decode_jpeg(
                Rect {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 1,
                },
                jpeg,
            ),
            Err(EngineError::Protocol)
        );
    }

    #[tokio::test]
    async fn loopback_handshake_composes_raw_and_copyrect_then_releases_input() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            server_handshake_none(&mut stream, 2, 1).await;
            send_raw_then_copy_update(&mut stream).await;

            // The client asks for the next incremental update before it
            // accepts user input; consume that protocol message first.
            let mut incremental_request = [0_u8; 10];
            stream.read_exact(&mut incremental_request).await.unwrap();
            assert_eq!(incremental_request[0], 3);
            assert_eq!(incremental_request[1], 1);

            let mut key_down = [0_u8; 8];
            stream.read_exact(&mut key_down).await.unwrap();
            assert_eq!(key_down, [4, 1, 0, 0, 0, 0, 0, b'a']);

            let mut key_up = [0_u8; 8];
            stream.read_exact(&mut key_up).await.unwrap();
            assert_eq!(key_up, [4, 0, 0, 0, 0, 0, 0, b'a']);
        });

        let (stop_tx, stop_rx) = watch::channel(false);
        let (_epoch_tx, epoch_rx) = watch::channel(0_u64);
        let (command_tx, command_rx) = mpsc::channel(4);
        let (frame_tx, frame_rx) = oneshot::channel();
        let frame_tx = Arc::new(Mutex::new(Some(frame_tx)));
        let sink: EventSink = Arc::new(move |event| {
            if let EngineEvent::Frame(frame) = event
                && let Some(sender) = frame_tx.lock().unwrap().take()
            {
                let _ = sender.send(frame);
            }
        });
        let stream = TcpStream::connect(address).await.unwrap();
        let task = tokio::spawn(run(
            Box::new(stream),
            VncOptions {
                password: Zeroizing::new(String::new()),
                allow_unauthenticated: true,
                clipboard_enabled: false,
            },
            command_rx,
            EngineControl {
                stop: stop_rx,
                focus_epoch: epoch_rx,
            },
            sink,
        ));

        let frame = match timeout(TEST_TIMEOUT, frame_rx).await {
            Ok(Ok(frame)) => frame,
            Ok(Err(_)) => panic!("session ended before frame: {:?}", task.await.unwrap()),
            Err(_) => panic!("session did not publish a frame"),
        };
        assert_eq!(frame.width, 2);
        assert_eq!(frame.height, 1);
        assert_eq!(frame.rgba, vec![255, 0, 0, 255, 255, 0, 0, 255]);

        let (completion_tx, completion_rx) = oneshot::channel();
        command_tx
            .send(EngineCommand {
                input: DesktopInput::Key {
                    scan_code: 4,
                    keysym: u32::from(b'a'),
                    down: true,
                },
                focus_epoch: 0,
                completion: completion_tx,
            })
            .await
            .unwrap();
        assert_eq!(
            timeout(TEST_TIMEOUT, completion_rx).await.unwrap().unwrap(),
            Ok(())
        );

        stop_tx.send(true).unwrap();
        assert_eq!(
            timeout(TEST_TIMEOUT, task).await.unwrap().unwrap(),
            Err(EngineError::Cancelled)
        );
        timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn loopback_tight_jpeg_update_is_decoded_as_a_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            server_handshake_none(&mut stream, 1, 1).await;
            send_tight_jpeg_update(&mut stream).await;
            let mut incremental_request = [0_u8; 10];
            stream.read_exact(&mut incremental_request).await.unwrap();
            assert_eq!(incremental_request[0..2], [3, 1]);
            sleep(Duration::from_secs(1)).await;
        });

        let (stop_tx, stop_rx) = watch::channel(false);
        let (_epoch_tx, epoch_rx) = watch::channel(0_u64);
        let (_command_tx, command_rx) = mpsc::channel(1);
        let (frame_tx, frame_rx) = oneshot::channel();
        let frame_tx = Arc::new(Mutex::new(Some(frame_tx)));
        let sink: EventSink = Arc::new(move |event| {
            if let EngineEvent::Frame(frame) = event
                && let Some(sender) = frame_tx.lock().unwrap().take()
            {
                let _ = sender.send(frame);
            }
        });
        let task = tokio::spawn(run(
            Box::new(TcpStream::connect(address).await.unwrap()),
            VncOptions {
                password: Zeroizing::new(String::new()),
                allow_unauthenticated: true,
                clipboard_enabled: false,
            },
            command_rx,
            EngineControl {
                stop: stop_rx,
                focus_epoch: epoch_rx,
            },
            sink,
        ));

        let frame = timeout(TEST_TIMEOUT, frame_rx).await.unwrap().unwrap();
        assert_eq!((frame.width, frame.height), (1, 1));
        assert!(frame.rgba[0] > 200);
        assert!(frame.rgba[1] < 32);
        assert!(frame.rgba[2] < 32);
        assert_eq!(frame.rgba[3], 255);

        stop_tx.send(true).unwrap();
        assert_eq!(
            timeout(TEST_TIMEOUT, task).await.unwrap().unwrap(),
            Err(EngineError::Cancelled)
        );
        timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn vnc_auth_is_preferred_when_none_is_also_offered() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            server_handshake_vnc(&mut stream, 1, 1).await;
            sleep(Duration::from_secs(1)).await;
        });

        let (stop_tx, stop_rx) = watch::channel(false);
        let (_epoch_tx, epoch_rx) = watch::channel(0_u64);
        let (_command_tx, command_rx) = mpsc::channel(1);
        let (ready_tx, ready_rx) = oneshot::channel();
        let ready_tx = Arc::new(Mutex::new(Some(ready_tx)));
        let sink: EventSink = Arc::new(move |event| {
            if matches!(event, EngineEvent::Ready)
                && let Some(sender) = ready_tx.lock().unwrap().take()
            {
                let _ = sender.send(());
            }
        });
        let task = tokio::spawn(run(
            Box::new(TcpStream::connect(address).await.unwrap()),
            VncOptions {
                password: Zeroizing::new("test-password".into()),
                allow_unauthenticated: true,
                clipboard_enabled: false,
            },
            command_rx,
            EngineControl {
                stop: stop_rx,
                focus_epoch: epoch_rx,
            },
            sink,
        ));

        timeout(TEST_TIMEOUT, ready_rx).await.unwrap().unwrap();
        stop_tx.send(true).unwrap();
        assert_eq!(
            timeout(TEST_TIMEOUT, task).await.unwrap().unwrap(),
            Err(EngineError::Cancelled)
        );
        timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn stale_epoch_is_rejected_before_any_peer_input() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            server_handshake_none(&mut stream, 1, 1).await;
            let mut unexpected = [0_u8; 8];
            // Shutdown may close the connection during this wait. That is
            // acceptable; only a fully received client input message would
            // prove that stale input crossed the peer boundary.
            assert!(!matches!(
                timeout(
                    Duration::from_millis(250),
                    stream.read_exact(&mut unexpected)
                )
                .await,
                Ok(Ok(_))
            ));
        });

        let (stop_tx, stop_rx) = watch::channel(false);
        let (epoch_tx, epoch_rx) = watch::channel(0_u64);
        let (command_tx, command_rx) = mpsc::channel(4);
        let (ready_tx, ready_rx) = oneshot::channel();
        let ready_tx = Arc::new(Mutex::new(Some(ready_tx)));
        let sink: EventSink = Arc::new(move |event| {
            if matches!(event, EngineEvent::Ready)
                && let Some(sender) = ready_tx.lock().unwrap().take()
            {
                let _ = sender.send(());
            }
        });
        let task = tokio::spawn(run(
            Box::new(TcpStream::connect(address).await.unwrap()),
            VncOptions {
                password: Zeroizing::new(String::new()),
                allow_unauthenticated: true,
                clipboard_enabled: false,
            },
            command_rx,
            EngineControl {
                stop: stop_rx,
                focus_epoch: epoch_rx,
            },
            sink,
        ));
        timeout(TEST_TIMEOUT, ready_rx).await.unwrap().unwrap();
        epoch_tx.send(1).unwrap();
        let (completion_tx, completion_rx) = oneshot::channel();
        command_tx
            .send(EngineCommand {
                input: DesktopInput::Pointer {
                    x: 0,
                    y: 0,
                    buttons: 1,
                },
                focus_epoch: 0,
                completion: completion_tx,
            })
            .await
            .unwrap();
        assert_eq!(
            timeout(TEST_TIMEOUT, completion_rx).await.unwrap().unwrap(),
            Err(EngineError::StaleInput)
        );

        stop_tx.send(true).unwrap();
        assert_eq!(
            timeout(TEST_TIMEOUT, task).await.unwrap().unwrap(),
            Err(EngineError::Cancelled)
        );
        timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_stalled_handshake() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted_tx, accepted_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            let _ = accepted_tx.send(());
            sleep(Duration::from_secs(1)).await;
        });

        let (stop_tx, stop_rx) = watch::channel(false);
        let (_epoch_tx, epoch_rx) = watch::channel(0_u64);
        let (_command_tx, command_rx) = mpsc::channel(1);
        let sink: EventSink = Arc::new(|_| {});
        let task = tokio::spawn(run(
            Box::new(TcpStream::connect(address).await.unwrap()),
            VncOptions {
                password: Zeroizing::new(String::new()),
                allow_unauthenticated: true,
                clipboard_enabled: false,
            },
            command_rx,
            EngineControl {
                stop: stop_rx,
                focus_epoch: epoch_rx,
            },
            sink,
        ));
        timeout(TEST_TIMEOUT, accepted_rx).await.unwrap().unwrap();
        stop_tx.send(true).unwrap();
        assert_eq!(
            timeout(TEST_TIMEOUT, task).await.unwrap().unwrap(),
            Err(EngineError::Cancelled)
        );
        timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_partial_raw_rectangle() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (partial_tx, partial_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            server_handshake_none(&mut stream, 1, 1).await;
            let mut update = vec![0, 0];
            update.extend_from_slice(&1_u16.to_be_bytes());
            append_rect_header(&mut update, 0, 0, 1, 1, 0);
            // Raw needs four BGRA bytes, but this peer stalls halfway through
            // the payload. A local cancellation must not wait for its EOF.
            update.extend_from_slice(&[0, 0]);
            stream.write_all(&update).await.unwrap();
            let _ = partial_tx.send(());
            let mut byte = [0_u8; 1];
            let _ = stream.read(&mut byte).await;
        });

        let (stop_tx, stop_rx) = watch::channel(false);
        let (_epoch_tx, epoch_rx) = watch::channel(0_u64);
        let (_command_tx, command_rx) = mpsc::channel(1);
        let (ready_tx, ready_rx) = oneshot::channel();
        let ready_tx = Arc::new(Mutex::new(Some(ready_tx)));
        let sink: EventSink = Arc::new(move |event| {
            if matches!(event, EngineEvent::Ready)
                && let Some(sender) = ready_tx.lock().unwrap().take()
            {
                let _ = sender.send(());
            }
        });
        let task = tokio::spawn(run(
            Box::new(TcpStream::connect(address).await.unwrap()),
            VncOptions {
                password: Zeroizing::new(String::new()),
                allow_unauthenticated: true,
                clipboard_enabled: false,
            },
            command_rx,
            EngineControl {
                stop: stop_rx,
                focus_epoch: epoch_rx,
            },
            sink,
        ));
        timeout(TEST_TIMEOUT, ready_rx).await.unwrap().unwrap();
        timeout(TEST_TIMEOUT, partial_rx).await.unwrap().unwrap();

        stop_tx.send(true).unwrap();
        assert_eq!(
            timeout(Duration::from_millis(400), task)
                .await
                .unwrap()
                .unwrap(),
            Err(EngineError::Cancelled)
        );
        timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn none_authentication_requires_explicit_opt_in() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(b"RFB 003.008\n").await.unwrap();
            let mut client_version = [0_u8; 12];
            stream.read_exact(&mut client_version).await.unwrap();
            stream.write_all(&[1, 1]).await.unwrap();
        });

        let (_stop_tx, stop_rx) = watch::channel(false);
        let (_epoch_tx, epoch_rx) = watch::channel(0_u64);
        let (_command_tx, command_rx) = mpsc::channel(1);
        let sink: EventSink = Arc::new(|_| {});
        let result = run(
            Box::new(TcpStream::connect(address).await.unwrap()),
            VncOptions {
                password: Zeroizing::new(String::new()),
                allow_unauthenticated: false,
                clipboard_enabled: false,
            },
            command_rx,
            EngineControl {
                stop: stop_rx,
                focus_epoch: epoch_rx,
            },
            sink,
        )
        .await;
        assert_eq!(result, Err(EngineError::AuthenticationRejected));
        timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
    }

    async fn server_handshake_none(stream: &mut TcpStream, width: u16, height: u16) {
        stream.write_all(b"RFB 003.008\n").await.unwrap();
        let mut client_version = [0_u8; 12];
        stream.read_exact(&mut client_version).await.unwrap();
        assert_eq!(client_version, *b"RFB 003.008\n");
        stream.write_all(&[1, 1]).await.unwrap();
        let mut selected_security = [0_u8; 1];
        stream.read_exact(&mut selected_security).await.unwrap();
        assert_eq!(selected_security, [1]);
        stream.write_all(&0_u32.to_be_bytes()).await.unwrap();
        server_init(stream, width, height).await;
    }

    async fn server_handshake_vnc(stream: &mut TcpStream, width: u16, height: u16) {
        stream.write_all(b"RFB 003.008\n").await.unwrap();
        let mut client_version = [0_u8; 12];
        stream.read_exact(&mut client_version).await.unwrap();
        assert_eq!(client_version, *b"RFB 003.008\n");
        // A client that permits None must still prefer VNC Auth whenever the
        // peer offers it.
        stream.write_all(&[2, 1, 2]).await.unwrap();
        let mut selected_security = [0_u8; 1];
        stream.read_exact(&mut selected_security).await.unwrap();
        assert_eq!(selected_security, [2]);
        let challenge = [0x5a_u8; 16];
        stream.write_all(&challenge).await.unwrap();
        let mut response = [0_u8; 16];
        stream.read_exact(&mut response).await.unwrap();
        assert_ne!(response, challenge);
        stream.write_all(&0_u32.to_be_bytes()).await.unwrap();
        server_init(stream, width, height).await;
    }

    async fn server_init(stream: &mut TcpStream, width: u16, height: u16) {
        let mut client_init = [0_u8; 1];
        stream.read_exact(&mut client_init).await.unwrap();
        assert_eq!(client_init, [1]);

        let mut init = Vec::new();
        init.extend_from_slice(&width.to_be_bytes());
        init.extend_from_slice(&height.to_be_bytes());
        init.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        init.extend_from_slice(&4_u32.to_be_bytes());
        init.extend_from_slice(b"test");
        stream.write_all(&init).await.unwrap();

        let mut pixel_format = [0_u8; 20];
        stream.read_exact(&mut pixel_format).await.unwrap();
        assert_eq!(pixel_format[0], 0);
        let mut encoding_header = [0_u8; 4];
        stream.read_exact(&mut encoding_header).await.unwrap();
        assert_eq!(encoding_header[..2], [2, 0]);
        let encodings = usize::from(u16::from_be_bytes([encoding_header[2], encoding_header[3]]));
        let mut encoding_values = vec![0_u8; encodings * 4];
        stream.read_exact(&mut encoding_values).await.unwrap();
        let mut request = [0_u8; 10];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(request[0], 3);
        assert_eq!(request[1], 0);
    }

    async fn send_raw_then_copy_update(stream: &mut TcpStream) {
        let mut update = vec![0, 0];
        update.extend_from_slice(&2_u16.to_be_bytes());
        append_rect_header(&mut update, 0, 0, 1, 1, 0);
        update.extend_from_slice(&[0, 0, 255, 0]);
        append_rect_header(&mut update, 1, 0, 1, 1, 1);
        update.extend_from_slice(&0_u16.to_be_bytes());
        update.extend_from_slice(&0_u16.to_be_bytes());
        stream.write_all(&update).await.unwrap();
    }

    async fn send_tight_jpeg_update(stream: &mut TcpStream) {
        let jpeg = include_bytes!("testdata/red-1x1.jpg");
        let mut update = vec![0, 0];
        update.extend_from_slice(&1_u16.to_be_bytes());
        append_rect_header(&mut update, 0, 0, 1, 1, 7);
        // Tight JPEG subencoding, with no zlib stream reset bits.
        update.push(0x90);
        append_tight_length(&mut update, jpeg.len());
        update.extend_from_slice(jpeg);
        stream.write_all(&update).await.unwrap();
    }

    fn append_tight_length(target: &mut Vec<u8>, length: usize) {
        assert!(length < (1 << 22));
        target.push(((length & 0x7f) as u8) | if length >= (1 << 7) { 0x80 } else { 0 });
        if length < (1 << 7) {
            return;
        }
        target.push((((length >> 7) & 0x7f) as u8) | if length >= (1 << 14) { 0x80 } else { 0 });
        if length >= (1 << 14) {
            target.push((length >> 14) as u8);
        }
    }

    fn append_rect_header(
        target: &mut Vec<u8>,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        encoding: i32,
    ) {
        target.extend_from_slice(&x.to_be_bytes());
        target.extend_from_slice(&y.to_be_bytes());
        target.extend_from_slice(&width.to_be_bytes());
        target.extend_from_slice(&height.to_be_bytes());
        target.extend_from_slice(&encoding.to_be_bytes());
    }
}
