//! NoriShell's shared-contract adapter around the hardened vendored `vnc-rs`
//! transport. Core receives bounded RGBA snapshots and stable public errors;
//! upstream RFB events, server error strings, and password handling stay here.
#![forbid(unsafe_code)]

use std::{collections::BTreeSet, io::Cursor, sync::Arc, time::Duration};

use jpeg_decoder::{Decoder as JpegDecoder, PixelFormat as JpegPixelFormat};
use norishell_desktop_protocol::{
    BoxedDesktopIo, CursorBitmap, DesktopCursor, DesktopFrame, DesktopInput, DesktopRect,
    DirtyRegion, EngineCommand, EngineControl, EngineError, EngineEvent, EventSink,
    MAX_CURSOR_DIMENSION, PUBLISH_INTERVAL, Result, frame_len,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{Instant, timeout},
};
use vnc::{
    ClientKeyEvent, ClientMouseEvent, HardenedVncClient, HardenedVncOptions, Rect, VncError,
    VncEvent, X11Event,
};
use zeroize::Zeroizing;

pub use vnc::VncVersion;

const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_WHEEL_STEPS: usize = 120;
const MAX_JPEG_BYTES: usize = 32 * 1024 * 1024;
const RESIZE_TIMEOUT: Duration = Duration::from_secs(5);

/// VNC's legacy challenge-response password is never cloned into UI state or
/// error values. `allow_unauthenticated` is false by caller choice by default.
pub struct VncOptions {
    pub password: Zeroizing<String>,
    pub allow_unauthenticated: bool,
    pub clipboard_enabled: bool,
    pub version: Option<VncVersion>,
    /// Fixed mode requests this size before the session becomes Ready.
    pub initial_resize: Option<(u16, u16)>,
}

/// Runs one already-connected VNC RFB session.
///
/// The server-side cursor shape is negotiated through the Cursor
/// pseudo-encoding and forwarded as `EngineEvent::Cursor`; it is never
/// composited into frame pixels. Remote resize requires a server
/// ExtendedDesktopSize advertisement and a matching server result.
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
        version,
        initial_resize,
    } = options;
    let mut client = HardenedVncClient::connect(
        stream,
        HardenedVncOptions {
            password,
            allow_unauthenticated,
            clipboard_enabled,
            version,
        },
        control.stop.clone(),
    )
    .await
    .map_err(map_error)?;

    let mut publisher = FramePublisher::default();
    let result = if let Some((width, height)) = initial_resize {
        await_initial_resize(
            &mut client,
            &mut control,
            &mut publisher,
            &events,
            width,
            height,
        )
        .await
    } else {
        Ok(())
    };
    let result = if result.is_ok() {
        events(EngineEvent::Ready);
        run_connected(
            &mut client,
            &mut commands,
            &mut control,
            &mut publisher,
            clipboard_enabled,
            Arc::clone(&events),
        )
        .await
    } else {
        result
    };
    client.shutdown().await;
    let queued_error = result
        .as_ref()
        .err()
        .copied()
        .unwrap_or(EngineError::Cancelled);
    complete_queued_commands(&mut commands, queued_error);
    result
}

async fn await_initial_resize(
    client: &mut HardenedVncClient,
    control: &mut EngineControl,
    publisher: &mut FramePublisher,
    events: &EventSink,
    width: u16,
    height: u16,
) -> Result<()> {
    // The first non-incremental update must contain ExtendedDesktopSize if
    // the server supports SetDesktopSize. A stalled initial update is not a
    // resize capability advertisement.
    timeout(RESIZE_TIMEOUT, async {
        loop {
            tokio::select! {
                () = stopped(&mut control.stop) => return Err(EngineError::Cancelled),
                event = client.next_event() => {
                    let event = event.map_err(map_error)?;
                    let complete = matches!(event, VncEvent::FramebufferUpdateComplete);
                    handle_server_event(event, publisher, events)?;
                    if complete { break; }
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| EngineError::VncResolutionUnavailable)??;
    timeout(CONTROL_WRITE_TIMEOUT, client.resize(width, height))
        .await
        .map_err(|_| EngineError::VncResolutionNotApplied)?
        .map_err(map_resize_write_error)?;
    timeout(RESIZE_TIMEOUT, async {
        loop {
            tokio::select! {
                () = stopped(&mut control.stop) => return Err(EngineError::Cancelled),
                event = client.next_event() => {
                    let event = event.map_err(map_error)?;
                    if let VncEvent::ResizeResult { applied, status } = event {
                        return resize_result(applied, status);
                    }
                    handle_server_event(event, publisher, events)?;
                }
            }
        }
    })
    .await
    .map_err(|_| EngineError::VncResolutionNotApplied)?
}

struct PendingResize {
    completion: oneshot::Sender<Result<()>>,
    deadline: Instant,
}

fn resize_result(applied: bool, status: u16) -> Result<()> {
    match (applied, status) {
        (true, 0) => Ok(()),
        (false, 0) => Err(EngineError::VncResolutionNotApplied),
        _ => Err(EngineError::VncResolutionRejected),
    }
}

fn map_resize_write_error(error: VncError) -> EngineError {
    match error {
        VncError::UnsupportedOperation => EngineError::VncResolutionUnavailable,
        VncError::Timeout => EngineError::VncResolutionNotApplied,
        other => map_error(other),
    }
}

async fn run_connected(
    client: &mut HardenedVncClient,
    commands: &mut mpsc::Receiver<EngineCommand>,
    control: &mut EngineControl,
    publisher: &mut FramePublisher,
    clipboard_enabled: bool,
    events: EventSink,
) -> Result<()> {
    let mut input = InputState::default();
    let mut commands_open = true;
    let mut focus_open = true;
    let mut pending_resize: Option<PendingResize> = None;

    loop {
        tokio::select! {
            biased;
            () = stopped(&mut control.stop) => {
                if let Some(pending) = pending_resize.take() {
                    let _ = pending.completion.send(Err(EngineError::Cancelled));
                }
                release_all(client, &mut input).await;
                return Err(EngineError::Cancelled);
            }
            changed = control.focus_epoch.changed(), if focus_open => {
                if changed.is_err() {
                    focus_open = false;
                }
                release_all(client, &mut input).await;
            }
            () = async {
                if let Some(pending) = &pending_resize {
                    tokio::time::sleep_until(pending.deadline).await;
                }
            }, if pending_resize.is_some() => {
                let pending = pending_resize.take().unwrap();
                let _ = pending.completion.send(Err(EngineError::VncResolutionNotApplied));
                // The vendor writer still owns this request. Close the session so a
                // late server result cannot be mistaken for a later resize.
                return Err(EngineError::VncResolutionNotApplied);
            }
            // Flushes coalesced updates that arrived inside the publish
            // interval; it runs only between complete framebuffer updates.
            () = tokio::time::sleep_until(publisher.deadline()), if publisher.flush_ready() => {
                if let Err(error) = publisher.publish(Instant::now(), &events) {
                    if let Some(pending) = pending_resize.take() {
                        let _ = pending.completion.send(Err(error));
                    }
                    return Err(error);
                }
            }
            server_event = client.next_event() => {
                let server_event = match server_event {
                    Ok(event) => event,
                    Err(error) => {
                        let error = map_error(error);
                        if let Some(pending) = pending_resize.take() {
                            let _ = pending.completion.send(Err(error));
                        }
                        return Err(error);
                    }
                };
                if let VncEvent::ResizeResult { applied, status } = server_event {
                    if let Some(pending) = pending_resize.take() {
                        let _ = pending.completion.send(resize_result(applied, status));
                    }
                } else {
                    if let Err(error) = handle_server_event(server_event, publisher, &events) {
                        if let Some(pending) = pending_resize.take() {
                            let _ = pending.completion.send(Err(error));
                        }
                        return Err(error);
                    }
                }
            }
            command = commands.recv(), if commands_open => {
                match command {
                    Some(command) => {
                        // VNC SetDesktopSize has no UI scale field; `scale_percent` is RDP-only.
                        if let DesktopInput::Resize { width, height, .. } = command.input {
                            if !control.accepts(&command) || command.input.validate().is_err() {
                                let error = if !control.accepts(&command) { rejection_for(control) } else { command.input.validate().unwrap_err() };
                                let _ = command.completion.send(Err(error));
                            } else if pending_resize.is_some() {
                                let _ = command.completion.send(Err(EngineError::VncResolutionNotApplied));
                            } else {
                                let result = timeout(CONTROL_WRITE_TIMEOUT, client.resize(width, height))
                                    .await.map_err(|_| EngineError::VncResolutionNotApplied)
                                    .and_then(|result| result.map_err(map_resize_write_error));
                                match result {
                                    Ok(()) => pending_resize = Some(PendingResize {
                                        completion: command.completion,
                                        deadline: Instant::now() + RESIZE_TIMEOUT,
                                    }),
                                    Err(error) => {
                                        let _ = command.completion.send(Err(error));
                                        if error != EngineError::VncResolutionUnavailable {
                                            return Err(error);
                                        }
                                    }
                                }
                            }
                        } else {
                            handle_command(client, command, control, clipboard_enabled, &mut input).await;
                        }
                    }
                    None => commands_open = false,
                }
            }
        }
    }
}

fn handle_server_event(
    event: VncEvent,
    publisher: &mut FramePublisher,
    sink: &EventSink,
) -> Result<()> {
    match event {
        VncEvent::SetResolution(screen) => publisher.set_resolution(screen.width, screen.height)?,
        VncEvent::RawImage(rect, mut bytes) => {
            if bytes.len() != frame_len(rect.width, rect.height)? {
                return Err(EngineError::Protocol);
            }
            // The negotiated RGBX wire format leaves the fourth byte undefined.
            for pixel in bytes.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
            publisher
                .frame_mut()?
                .write_rect(rect.x, rect.y, rect.width, rect.height, &bytes)?;
            publisher.mark(rect);
        }
        VncEvent::Copy(destination, source) => {
            publisher.frame_mut()?.copy_rect(
                source.x,
                source.y,
                destination.x,
                destination.y,
                destination.width,
                destination.height,
            )?;
            publisher.mark(destination);
        }
        VncEvent::JpegImage(rect, bytes) => {
            write_jpeg(publisher.frame_mut()?, rect, &bytes)?;
            publisher.mark(rect);
        }
        VncEvent::FramebufferUpdateComplete => publisher.complete(Instant::now(), sink)?,
        VncEvent::SetCursor(rect, bytes) => {
            sink(EngineEvent::Cursor(cursor_from_rfb(rect, &bytes)))
        }
        VncEvent::Text(text) => sink(EngineEvent::Clipboard(text)),
        VncEvent::Bell => {}
        VncEvent::ResizeResult { .. } => {}
        // The hardened vendor path always sends SetPixelFormat and never
        // emits this; retain failure-closed behavior if that invariant changes.
        VncEvent::SetPixelFormat(_) => return Err(EngineError::UnsupportedOperation),
        _ => return Err(EngineError::UnsupportedOperation),
    }
    Ok(())
}

/// Owns the decode surface and turns completed RFB updates into bounded,
/// rate-limited `Frame`/`FramePatch` events. Core keeps its own projection, so
/// the full surface is copied only for the first frame, a size change, or an
/// update covering most of the frame.
#[derive(Default)]
struct FramePublisher {
    frame: Option<DesktopFrame>,
    /// Rectangles written by the FramebufferUpdate currently being received.
    in_progress: DirtyRegion,
    /// Rectangles from completed updates that have not been published yet.
    committed: DirtyRegion,
    /// The next publish must replace Core's frame (first frame or new size).
    requires_full: bool,
    /// Set only by a FramebufferUpdateComplete, so neither a blank surface
    /// after a size change nor a half-received update is ever published.
    ready: bool,
    last_publish: Option<Instant>,
}

impl FramePublisher {
    fn set_resolution(&mut self, width: u16, height: u16) -> Result<()> {
        if self
            .frame
            .as_ref()
            .is_some_and(|frame| frame.width == width && frame.height == height)
        {
            // An ExtendedDesktopSize echo of the current size keeps pixels.
            return Ok(());
        }
        self.frame = Some(DesktopFrame::new(width, height)?);
        self.in_progress.clear();
        self.committed.clear();
        self.requires_full = true;
        self.ready = false;
        Ok(())
    }

    fn frame_mut(&mut self) -> Result<&mut DesktopFrame> {
        self.frame.as_mut().ok_or(EngineError::Protocol)
    }

    fn mark(&mut self, rect: Rect) {
        self.in_progress.add(DesktopRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        });
    }

    fn has_pending(&self) -> bool {
        self.ready && self.frame.is_some()
    }

    /// A timer flush publishes only complete updates; while a later update is
    /// still arriving, its FramebufferUpdateComplete publishes instead.
    fn flush_ready(&self) -> bool {
        self.has_pending() && self.in_progress.is_empty()
    }

    fn deadline(&self) -> Instant {
        self.last_publish
            .map_or_else(Instant::now, |last| last + PUBLISH_INTERVAL)
    }

    fn complete(&mut self, now: Instant, sink: &EventSink) -> Result<()> {
        self.committed.extend(&self.in_progress);
        self.in_progress.clear();
        self.ready = self.frame.is_some() && (self.requires_full || !self.committed.is_empty());
        // The first frame is never delayed; later ones wait for the interval.
        if self
            .last_publish
            .is_none_or(|last| now >= last + PUBLISH_INTERVAL)
        {
            self.publish(now, sink)?;
        }
        Ok(())
    }

    fn publish(&mut self, now: Instant, sink: &EventSink) -> Result<()> {
        if !self.has_pending() {
            return Ok(());
        }
        let frame = self.frame.as_ref().ok_or(EngineError::Protocol)?;
        let total = u64::from(frame.width) * u64::from(frame.height);
        if self.requires_full || self.committed.area() * 10 >= total * 6 {
            sink(EngineEvent::Frame(Arc::new(frame.clone())));
        } else {
            sink(EngineEvent::FramePatch(
                frame.extract_patch(&self.committed)?,
            ));
        }
        self.requires_full = false;
        self.ready = false;
        self.committed.clear();
        self.last_publish = Some(now);
        Ok(())
    }
}

/// Converts an RFB Cursor pseudo-rectangle (RGBX pixels plus a 1-bpp mask)
/// into a straight-alpha bitmap. A 0x0 or fully transparent shape hides the
/// pointer; any malformed or oversized shape falls back to the default arrow
/// instead of failing the session.
fn cursor_from_rfb(rect: Rect, bytes: &[u8]) -> DesktopCursor {
    if rect.width == 0 || rect.height == 0 {
        return DesktopCursor::Hidden;
    }
    if rect.width > MAX_CURSOR_DIMENSION || rect.height > MAX_CURSOR_DIMENSION {
        return DesktopCursor::Default;
    }
    let width = usize::from(rect.width);
    let height = usize::from(rect.height);
    let pixel_len = width * height * 4;
    let mask_row = width.div_ceil(8);
    if bytes.len() != pixel_len + mask_row * height {
        return DesktopCursor::Default;
    }
    let (pixels, mask) = bytes.split_at(pixel_len);
    let mut rgba = vec![0_u8; pixel_len];
    let mut visible = false;
    for (y, (target_row, source_row)) in rgba
        .chunks_exact_mut(width * 4)
        .zip(pixels.chunks_exact(width * 4))
        .enumerate()
    {
        let mask_bits = &mask[y * mask_row..(y + 1) * mask_row];
        for (x, (target, source)) in target_row
            .chunks_exact_mut(4)
            .zip(source_row.chunks_exact(4))
            .enumerate()
        {
            // Transparent pixels stay all-zero so the bitmap is straight alpha.
            if mask_bits[x / 8] & (0x80 >> (x % 8)) != 0 {
                target[..3].copy_from_slice(&source[..3]);
                target[3] = 255;
                visible = true;
            }
        }
    }
    if !visible {
        return DesktopCursor::Hidden;
    }
    match CursorBitmap::new(rect.width, rect.height, rect.x, rect.y, rgba) {
        Ok(bitmap) => DesktopCursor::Bitmap(Arc::new(bitmap)),
        Err(_) => DesktopCursor::Default,
    }
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

/// Decodes a Tight JPEG rectangle straight into the frame rows, avoiding an
/// intermediate RGBA buffer beyond the decoder's own output.
fn write_jpeg(frame: &mut DesktopFrame, rect: Rect, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_JPEG_BYTES {
        return Err(EngineError::ResourceLimit);
    }
    let target = DesktopRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    };
    if !target.fits(frame.width, frame.height) {
        return Err(EngineError::Protocol);
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
    let channels = match info.pixel_format {
        JpegPixelFormat::RGB24 => 3,
        JpegPixelFormat::L8 => 1,
        JpegPixelFormat::L16 | JpegPixelFormat::CMYK32 => {
            return Err(EngineError::UnsupportedOperation);
        }
    };
    // jpeg-decoder limits its decoded component planes by this value. RGBA
    // output needs four bytes per RFB pixel, which bounds all supported JPEG
    // source formats at the negotiated frame size.
    decoder.set_max_decoding_buffer_size(frame_len(rect.width, rect.height)?);
    let pixels = decoder.decode().map_err(|_| EngineError::Protocol)?;
    let width = usize::from(rect.width);
    if pixels.len() != width * usize::from(rect.height) * channels {
        return Err(EngineError::Protocol);
    }
    let stride = usize::from(frame.width) * 4;
    for (row, source) in pixels.chunks_exact(width * channels).enumerate() {
        let start = (usize::from(rect.y) + row) * stride + usize::from(rect.x) * 4;
        let target = &mut frame.rgba[start..start + width * 4];
        for (pixel, source) in target
            .chunks_exact_mut(4)
            .zip(source.chunks_exact(channels))
        {
            if channels == 3 {
                pixel[..3].copy_from_slice(source);
            } else {
                pixel[..3].fill(source[0]);
            }
            pixel[3] = 255;
        }
    }
    Ok(())
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
    fn jpeg_is_preflighted_and_written_as_rgba() {
        let jpeg = include_bytes!("testdata/red-1x1.jpg");
        let mut frame = DesktopFrame::new(2, 1).unwrap();
        write_jpeg(&mut frame, rect(1, 0, 1, 1), jpeg).unwrap();
        assert_eq!(frame.rgba[..4], [0, 0, 0, 0]);
        let rgba = &frame.rgba[4..];
        assert!(rgba[0] > 200);
        assert!(rgba[1] < 32);
        assert!(rgba[2] < 32);
        assert_eq!(rgba[3], 255);
    }

    #[test]
    fn jpeg_dimensions_must_match_the_rfb_rectangle() {
        let jpeg = include_bytes!("testdata/red-1x1.jpg");
        let mut frame = DesktopFrame::new(2, 1).unwrap();
        assert_eq!(
            write_jpeg(&mut frame, rect(0, 0, 2, 1), jpeg),
            Err(EngineError::Protocol)
        );
        assert_eq!(
            write_jpeg(&mut frame, rect(2, 0, 1, 1), jpeg),
            Err(EngineError::Protocol)
        );
    }

    fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[derive(Default)]
    struct Recorded {
        frames: Vec<(u16, u16)>,
        patches: Vec<Vec<(DesktopRect, Vec<u8>)>>,
        cursors: Vec<DesktopCursor>,
    }

    fn recording_sink() -> (EventSink, Arc<Mutex<Recorded>>) {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let target = Arc::clone(&recorded);
        let sink: EventSink = Arc::new(move |event| {
            let mut recorded = target.lock().unwrap();
            match event {
                EngineEvent::Frame(frame) => recorded.frames.push((frame.width, frame.height)),
                EngineEvent::FramePatch(patch) => recorded.patches.push(patch.rects),
                EngineEvent::Cursor(cursor) => recorded.cursors.push(cursor),
                _ => {}
            }
        });
        (sink, recorded)
    }

    fn raw(x: u16, y: u16, width: u16, height: u16, value: u8) -> VncEvent {
        VncEvent::RawImage(
            rect(x, y, width, height),
            vec![value; usize::from(width) * usize::from(height) * 4],
        )
    }

    #[test]
    fn publisher_sends_first_frame_immediately_then_rate_limited_patches() {
        let (sink, recorded) = recording_sink();
        let mut publisher = FramePublisher::default();
        let start = Instant::now();
        publisher.set_resolution(10, 10).unwrap();
        // A blank surface is never flushed before its first update completes.
        assert!(!publisher.flush_ready());
        publisher.mark(rect(0, 0, 10, 10));
        publisher.complete(start, &sink).unwrap();
        assert_eq!(recorded.lock().unwrap().frames, [(10, 10)]);
        assert!(!publisher.flush_ready());

        // Two small updates inside one interval coalesce into one patch.
        handle_server_event(raw(1, 1, 2, 2, 7), &mut publisher, &sink).unwrap();
        publisher
            .complete(start + Duration::from_millis(1), &sink)
            .unwrap();
        handle_server_event(raw(8, 8, 1, 1, 9), &mut publisher, &sink).unwrap();
        assert!(
            !publisher.flush_ready(),
            "an update in flight defers the timer flush"
        );
        publisher
            .complete(start + Duration::from_millis(2), &sink)
            .unwrap();
        assert!(recorded.lock().unwrap().patches.is_empty());
        assert!(publisher.flush_ready());
        assert_eq!(publisher.deadline(), start + PUBLISH_INTERVAL);

        publisher.publish(start + PUBLISH_INTERVAL, &sink).unwrap();
        let recorded = recorded.lock().unwrap();
        assert_eq!(recorded.frames.len(), 1);
        assert_eq!(recorded.patches.len(), 1);
        let patch = &recorded.patches[0];
        assert_eq!(patch.len(), 2);
        let (first, pixels) = patch.iter().find(|(rect, _)| rect.x == 1).unwrap();
        assert_eq!((first.width, first.height), (2, 2));
        // Raw pixels keep RGB and force opaque alpha.
        assert_eq!(pixels[..4], [7, 7, 7, 255]);
        assert!(
            patch
                .iter()
                .any(|(rect, pixels)| rect.x == 8 && pixels[..] == [9, 9, 9, 255])
        );
        assert!(!publisher.flush_ready());
    }

    #[test]
    fn publisher_sends_full_frames_for_large_regions_and_size_changes() {
        let (sink, recorded) = recording_sink();
        let mut publisher = FramePublisher::default();
        let start = Instant::now();
        publisher.set_resolution(10, 10).unwrap();
        publisher.complete(start, &sink).unwrap();

        handle_server_event(raw(0, 0, 10, 6, 1), &mut publisher, &sink).unwrap();
        publisher.complete(start + PUBLISH_INTERVAL, &sink).unwrap();
        assert_eq!(recorded.lock().unwrap().frames.len(), 2);
        assert!(recorded.lock().unwrap().patches.is_empty());

        // CopyRect marks only its destination.
        handle_server_event(
            VncEvent::Copy(rect(5, 5, 2, 2), rect(0, 0, 2, 2)),
            &mut publisher,
            &sink,
        )
        .unwrap();
        publisher
            .complete(start + PUBLISH_INTERVAL * 2, &sink)
            .unwrap();
        let patch = recorded.lock().unwrap().patches.pop().unwrap();
        assert_eq!(
            patch[0].0,
            DesktopRect {
                x: 5,
                y: 5,
                width: 2,
                height: 2
            }
        );
        assert_eq!(patch[0].1[..4], [1, 1, 1, 255]);

        // The same size keeps the surface; a new size forces a full frame.
        publisher.set_resolution(10, 10).unwrap();
        assert!(!publisher.flush_ready());
        publisher.set_resolution(4, 3).unwrap();
        publisher
            .complete(start + PUBLISH_INTERVAL * 3, &sink)
            .unwrap();
        assert_eq!(recorded.lock().unwrap().frames.last(), Some(&(4, 3)));
    }

    fn cursor_payload(pixels: &[[u8; 4]], mask: &[u8]) -> Vec<u8> {
        let mut payload = pixels.concat();
        payload.extend_from_slice(mask);
        payload
    }

    #[test]
    fn cursor_mask_becomes_straight_alpha() {
        let payload = cursor_payload(
            &[[10, 20, 30, 0], [40, 50, 60, 0], [1, 2, 3, 0], [4, 5, 6, 0]],
            &[0b1000_0000, 0b0100_0000],
        );
        let DesktopCursor::Bitmap(bitmap) = cursor_from_rfb(rect(1, 9, 2, 2), &payload) else {
            panic!("expected a bitmap cursor");
        };
        assert_eq!((bitmap.width, bitmap.height), (2, 2));
        // Hotspot comes from the rectangle position and is clamped to the shape.
        assert_eq!((bitmap.hotspot_x, bitmap.hotspot_y), (1, 1));
        assert_eq!(
            bitmap.rgba,
            [10, 20, 30, 255, 0, 0, 0, 0, 0, 0, 0, 0, 4, 5, 6, 255]
        );
    }

    #[test]
    fn empty_invalid_and_oversized_cursors_do_not_fail_the_session() {
        assert_eq!(
            cursor_from_rfb(rect(0, 0, 0, 0), &[]),
            DesktopCursor::Hidden
        );
        let transparent = cursor_payload(&[[9, 9, 9, 9]], &[0]);
        assert_eq!(
            cursor_from_rfb(rect(0, 0, 1, 1), &transparent),
            DesktopCursor::Hidden
        );
        assert_eq!(
            cursor_from_rfb(rect(0, 0, 1, 1), &[1, 2, 3]),
            DesktopCursor::Default
        );
        let big = MAX_CURSOR_DIMENSION + 1;
        // The vendor discards shapes it will not buffer and reports no payload.
        assert_eq!(
            cursor_from_rfb(rect(0, 0, big, 1), &[]),
            DesktopCursor::Default
        );
    }

    #[tokio::test]
    async fn fixed_resize_waits_for_a_matching_server_ack_before_ready() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            server_handshake_none(&mut stream, 1, 1).await;
            send_extended_update(&mut stream, 0, 0, 1, 1, 7, 9).await;
            let request = read_resize_request(&mut stream).await;
            assert_eq!(request[..8], [251, 0, 0, 2, 0, 2, 1, 0]);
            assert_eq!(request[8..12], 7_u32.to_be_bytes());
            assert_eq!(request[20..24], 9_u32.to_be_bytes());
            send_extended_update(&mut stream, 1, 0, 2, 2, 7, 9).await;
            sleep(Duration::from_millis(200)).await;
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
                version: None,
                initial_resize: Some((2, 2)),
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
    async fn fixed_resize_distinguishes_absent_rejected_and_unapplied() {
        for (extended, status, expected) in [
            (false, 0, EngineError::VncResolutionUnavailable),
            (true, 1, EngineError::VncResolutionRejected),
            (true, 0, EngineError::VncResolutionNotApplied),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                server_handshake_none(&mut stream, 1, 1).await;
                if extended {
                    send_extended_update(&mut stream, 0, 0, 1, 1, 7, 0).await;
                    let _request = read_resize_request(&mut stream).await;
                    send_extended_update(&mut stream, 1, status, 1, 1, 7, 0).await;
                } else {
                    stream.write_all(&[0, 0, 0, 0]).await.unwrap();
                }
            });
            let (_stop_tx, stop_rx) = watch::channel(false);
            let (_epoch_tx, epoch_rx) = watch::channel(0_u64);
            let (_command_tx, command_rx) = mpsc::channel(1);
            let result = timeout(
                TEST_TIMEOUT,
                run(
                    Box::new(TcpStream::connect(address).await.unwrap()),
                    VncOptions {
                        password: Zeroizing::new(String::new()),
                        allow_unauthenticated: true,
                        clipboard_enabled: false,
                        version: None,
                        initial_resize: Some((2, 2)),
                    },
                    command_rx,
                    EngineControl {
                        stop: stop_rx,
                        focus_epoch: epoch_rx,
                    },
                    Arc::new(|_| {}),
                ),
            )
            .await
            .unwrap();
            assert_eq!(result, Err(expected));
            timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
        }
    }

    #[tokio::test]
    async fn runtime_resize_completes_only_after_remote_result() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = oneshot::channel();
        let (ack_tx, ack_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            server_handshake_none(&mut stream, 1, 1).await;
            send_extended_update(&mut stream, 0, 0, 1, 1, 7, 0).await;
            let request = read_resize_request(&mut stream).await;
            assert_eq!(request[..8], [251, 0, 0, 2, 0, 2, 1, 0]);
            let _ = request_tx.send(());
            let _ = ack_rx.await;
            send_extended_update(&mut stream, 1, 0, 2, 2, 7, 0).await;
            sleep(Duration::from_millis(200)).await;
        });
        let (stop_tx, stop_rx) = watch::channel(false);
        let (_epoch_tx, epoch_rx) = watch::channel(0_u64);
        let (command_tx, command_rx) = mpsc::channel(1);
        let (frame_tx, frame_rx) = oneshot::channel();
        let frame_tx = Arc::new(Mutex::new(Some(frame_tx)));
        let sink: EventSink = Arc::new(move |event| {
            if let EngineEvent::Frame(_) = event
                && let Some(sender) = frame_tx.lock().unwrap().take()
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
                version: None,
                initial_resize: None,
            },
            command_rx,
            EngineControl {
                stop: stop_rx,
                focus_epoch: epoch_rx,
            },
            sink,
        ));
        timeout(TEST_TIMEOUT, frame_rx).await.unwrap().unwrap();
        let (completion_tx, mut completion_rx) = oneshot::channel();
        command_tx
            .send(EngineCommand {
                input: DesktopInput::Resize {
                    width: 2,
                    height: 2,
                    scale_percent: 150,
                },
                focus_epoch: None,
                completion: completion_tx,
            })
            .await
            .unwrap();
        timeout(TEST_TIMEOUT, request_rx).await.unwrap().unwrap();
        assert!(
            timeout(Duration::from_millis(50), &mut completion_rx)
                .await
                .is_err()
        );
        ack_tx.send(()).unwrap();
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

    async fn send_extended_update(
        stream: &mut TcpStream,
        reason: u16,
        status: u16,
        width: u16,
        height: u16,
        id: u32,
        flags: u32,
    ) {
        let mut update = vec![0, 0, 0, 1];
        append_rect_header(&mut update, reason, status, width, height, -308);
        update.extend_from_slice(&[1, 0, 0, 0]);
        update.extend_from_slice(&id.to_be_bytes());
        update.extend_from_slice(&[0; 4]);
        update.extend_from_slice(&width.to_be_bytes());
        update.extend_from_slice(&height.to_be_bytes());
        update.extend_from_slice(&flags.to_be_bytes());
        stream.write_all(&update).await.unwrap();
    }

    async fn read_resize_request(stream: &mut TcpStream) -> [u8; 24] {
        loop {
            let mut kind = [0_u8; 1];
            stream.read_exact(&mut kind).await.unwrap();
            match kind[0] {
                3 => {
                    let mut update = [0_u8; 9];
                    stream.read_exact(&mut update).await.unwrap();
                }
                251 => {
                    let mut request = [0_u8; 24];
                    request[0] = 251;
                    stream.read_exact(&mut request[1..]).await.unwrap();
                    return request;
                }
                other => panic!("unexpected client message {other}"),
            }
        }
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
                version: None,
                initial_resize: None,
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
                focus_epoch: Some(0),
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
                version: None,
                initial_resize: None,
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

    enum Observed {
        Frame(Arc<DesktopFrame>),
        Patch(norishell_desktop_protocol::FramePatch),
        Cursor(DesktopCursor),
    }

    /// Runs a session against a scripted loopback server and forwards frame
    /// and cursor events.
    async fn start_loopback<F, Fut>(
        width: u16,
        height: u16,
        script: F,
    ) -> (
        tokio::task::JoinHandle<Result<()>>,
        tokio::task::JoinHandle<()>,
        mpsc::UnboundedReceiver<Observed>,
        watch::Sender<bool>,
    )
    where
        F: FnOnce(TcpStream) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            server_handshake_none(&mut stream, width, height).await;
            script(stream).await;
        });
        let (stop_tx, stop_rx) = watch::channel(false);
        let (_epoch_tx, epoch_rx) = watch::channel(0_u64);
        let (_command_tx, command_rx) = mpsc::channel(1);
        let (observed_tx, observed_rx) = mpsc::unbounded_channel();
        let sink: EventSink = Arc::new(move |event| {
            let observed = match event {
                EngineEvent::Frame(frame) => Observed::Frame(frame),
                EngineEvent::FramePatch(patch) => Observed::Patch(patch),
                EngineEvent::Cursor(cursor) => Observed::Cursor(cursor),
                _ => return,
            };
            let _ = observed_tx.send(observed);
        });
        let task = tokio::spawn(run(
            Box::new(TcpStream::connect(address).await.unwrap()),
            VncOptions {
                password: Zeroizing::new(String::new()),
                allow_unauthenticated: true,
                clipboard_enabled: false,
                version: None,
                initial_resize: None,
            },
            command_rx,
            EngineControl {
                stop: stop_rx,
                focus_epoch: epoch_rx,
            },
            sink,
        ));
        (task, server, observed_rx, stop_tx)
    }

    async fn next_observed(observed: &mut mpsc::UnboundedReceiver<Observed>) -> Observed {
        timeout(TEST_TIMEOUT, observed.recv())
            .await
            .expect("session did not publish")
            .expect("session ended")
    }

    async fn read_incremental_request(stream: &mut TcpStream) {
        let mut request = [0_u8; 10];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(request[0..2], [3, 1]);
    }

    #[tokio::test]
    async fn loopback_cursor_and_small_update_publish_cursor_then_patch() {
        let (task, server, mut observed, stop_tx) = start_loopback(2, 1, |mut stream| async move {
            let mut update = vec![0, 0];
            update.extend_from_slice(&2_u16.to_be_bytes());
            append_rect_header(&mut update, 0, 0, 2, 1, 0);
            update.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0]);
            // Cursor pseudo-rectangle: position is the hotspot, 1x1 RGBX
            // pixel followed by a one-byte row mask.
            append_rect_header(&mut update, 0, 0, 1, 1, -239);
            update.extend_from_slice(&[1, 2, 3, 0, 0x80]);
            stream.write_all(&update).await.unwrap();
            read_incremental_request(&mut stream).await;

            let mut update = vec![0, 0];
            update.extend_from_slice(&1_u16.to_be_bytes());
            append_rect_header(&mut update, 1, 0, 1, 1, 0);
            update.extend_from_slice(&[0, 255, 0, 0]);
            stream.write_all(&update).await.unwrap();
            read_incremental_request(&mut stream).await;
            sleep(Duration::from_millis(500)).await;
        })
        .await;

        // Cursor shapes are forwarded as decoded; frames wait for the end of
        // the FramebufferUpdate.
        let Observed::Cursor(DesktopCursor::Bitmap(cursor)) = next_observed(&mut observed).await
        else {
            panic!("expected a bitmap cursor");
        };
        assert_eq!(cursor.rgba, [1, 2, 3, 255]);
        let Observed::Frame(frame) = next_observed(&mut observed).await else {
            panic!("expected the first full frame");
        };
        // The cursor is not composited into frame pixels.
        assert_eq!(frame.rgba, [255, 0, 0, 255, 255, 0, 0, 255]);
        let Observed::Patch(patch) = next_observed(&mut observed).await else {
            panic!("expected a patch for the small update");
        };
        assert_eq!((patch.width, patch.height), (2, 1));
        assert_eq!(patch.rects.len(), 1);
        assert_eq!(
            patch.rects[0].0,
            DesktopRect {
                x: 1,
                y: 0,
                width: 1,
                height: 1
            }
        );
        assert_eq!(patch.rects[0].1, [0, 255, 0, 255]);

        stop_tx.send(true).unwrap();
        assert_eq!(
            timeout(TEST_TIMEOUT, task).await.unwrap().unwrap(),
            Err(EngineError::Cancelled)
        );
        timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn loopback_zrle_tiles_compose_into_one_rectangle() {
        let (task, server, mut observed, stop_tx) =
            start_loopback(65, 1, |mut stream| async move {
                // Two raw true-colour ZRLE tiles (64 px + 1 px) with 3-byte CPIXELs.
                let mut tiles = vec![0_u8];
                for index in 0..64_u8 {
                    tiles.extend_from_slice(&[index, 1, 2]);
                }
                tiles.push(0);
                tiles.extend_from_slice(&[200, 201, 202]);
                // One uncompressed, non-final deflate block keeps the shared zlib
                // stream open exactly like a real server's sync-flushed output.
                let mut zlib = vec![0x78, 0x01, 0x00];
                let length = u16::try_from(tiles.len()).unwrap();
                zlib.extend_from_slice(&length.to_le_bytes());
                zlib.extend_from_slice(&(!length).to_le_bytes());
                zlib.extend_from_slice(&tiles);

                let mut update = vec![0, 0];
                update.extend_from_slice(&1_u16.to_be_bytes());
                append_rect_header(&mut update, 0, 0, 65, 1, 16);
                update.extend_from_slice(&u32::try_from(zlib.len()).unwrap().to_be_bytes());
                update.extend_from_slice(&zlib);
                stream.write_all(&update).await.unwrap();
                read_incremental_request(&mut stream).await;
                sleep(Duration::from_millis(500)).await;
            })
            .await;

        let Observed::Frame(frame) = next_observed(&mut observed).await else {
            panic!("expected a full frame");
        };
        assert_eq!((frame.width, frame.height), (65, 1));
        assert_eq!(frame.rgba[..8], [0, 1, 2, 255, 1, 1, 2, 255]);
        assert_eq!(frame.rgba[64 * 4..], [200, 201, 202, 255]);

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
                version: None,
                initial_resize: None,
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
    async fn explicit_rfb33_uses_password_auth_with_custom_server_banner() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(b"RFB 003.889\n").await.unwrap();
            let mut client_version = [0_u8; 12];
            stream.read_exact(&mut client_version).await.unwrap();
            assert_eq!(client_version, *b"RFB 003.003\n");
            stream.write_all(&2_u32.to_be_bytes()).await.unwrap();
            stream.write_all(&[0x5a; 16]).await.unwrap();
            let mut response = [0_u8; 16];
            stream.read_exact(&mut response).await.unwrap();
            assert_ne!(response, [0x5a; 16]);
            stream.write_all(&0_u32.to_be_bytes()).await.unwrap();
            server_init(&mut stream, 1, 1).await;
            sleep(Duration::from_millis(200)).await;
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
                password: Zeroizing::new("example-password".into()),
                allow_unauthenticated: false,
                clipboard_enabled: false,
                version: Some(VncVersion::RFB33),
                initial_resize: None,
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
                version: None,
                initial_resize: None,
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
                focus_epoch: Some(0),
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
                version: None,
                initial_resize: None,
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
            // Raw needs four RGBX bytes, but this peer stalls halfway through
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
                version: None,
                initial_resize: None,
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
                version: None,
                initial_resize: None,
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
        // 32bpp little-endian true colour with R/G/B shifts 0/8/16 puts wire
        // pixels in RGBX byte order.
        assert_eq!(
            pixel_format[4..],
            [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0]
        );
        let mut encoding_header = [0_u8; 4];
        stream.read_exact(&mut encoding_header).await.unwrap();
        assert_eq!(encoding_header[..2], [2, 0]);
        let encodings = usize::from(u16::from_be_bytes([encoding_header[2], encoding_header[3]]));
        let mut encoding_values = vec![0_u8; encodings * 4];
        stream.read_exact(&mut encoding_values).await.unwrap();
        for expected in [-308_i32, -239] {
            assert!(
                encoding_values
                    .chunks_exact(4)
                    .any(|encoding| encoding == expected.to_be_bytes())
            );
        }
        let mut request = [0_u8; 10];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(request[0], 3);
        assert_eq!(request[1], 0);
    }

    async fn send_raw_then_copy_update(stream: &mut TcpStream) {
        let mut update = vec![0, 0];
        update.extend_from_slice(&2_u16.to_be_bytes());
        append_rect_header(&mut update, 0, 0, 1, 1, 0);
        // RGBX with an undefined padding byte; the client forces alpha.
        update.extend_from_slice(&[255, 0, 0, 0]);
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
