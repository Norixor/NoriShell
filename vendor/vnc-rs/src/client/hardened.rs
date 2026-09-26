//! Bounded, cancellation-aware client transport built on the upstream
//! `vnc-rs` RFB codecs. The historical connector is deliberately not exposed:
//! it accepted unauthenticated sessions implicitly, allocated from untrusted
//! lengths, and detached decoder tasks without a join path.

use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};
use zeroize::{Zeroize, Zeroizing};

use super::{messages::ClientMsg, security};
use crate::{
    MAX_DIMENSION, MAX_PIXELS, MAX_TEXT_BYTES, PixelFormat, Rect, VncEncoding, VncError, VncEvent,
    VncVersion, X11Event, codec,
};

const EVENT_QUEUE_CAPACITY: usize = 32;
const INPUT_QUEUE_CAPACITY: usize = 1;
const MAX_SERVER_NAME_BYTES: usize = 4 * 1024;
const TASK_JOIN_TIMEOUT: Duration = Duration::from_secs(1);

pub struct HardenedVncOptions {
    /// Owned only for the initial VNC challenge-response exchange.
    pub password: Zeroizing<String>,
    /// RFB None is intentionally opt-in because the base RFB protocol does
    /// not protect screen, input, or clipboard traffic.
    pub allow_unauthenticated: bool,
    /// Controls both explicit ClientCutText sends and ServerCutText delivery.
    pub clipboard_enabled: bool,
    /// None follows a known server banner; an explicit version uses that
    /// handshake only when the server advertises an equal or newer RFB 3.x.
    pub version: Option<VncVersion>,
}

/// A session uses two small bounded request queues. Priority traffic is kept
/// separate so focus-loss release events never sit behind regular input.
pub struct HardenedVncClient {
    normal_tx: mpsc::Sender<WriteRequest>,
    priority_tx: mpsc::Sender<WriteRequest>,
    events: mpsc::Receiver<Result<VncEvent, VncError>>,
    stop_tx: watch::Sender<bool>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    resize: Arc<Mutex<ResizeState>>,
}

enum WriteRequest {
    Input {
        events: Vec<X11Event>,
        completion: oneshot::Sender<Result<(), VncError>>,
    },
    Update {
        width: u16,
        height: u16,
        incremental: bool,
    },
    Resize {
        width: u16,
        height: u16,
        completion: oneshot::Sender<Result<(), VncError>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScreenInfo {
    id: u32,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    flags: u32,
}

#[derive(Clone, Copy, Debug)]
struct RequestedLayout {
    width: u16,
    height: u16,
    screen_id: u32,
    flags: u32,
    forwarded: bool,
}

#[derive(Default)]
struct ResizeState {
    // None means the server has not confirmed ExtendedDesktopSize support.
    layout: Option<(Dimensions, Vec<ScreenInfo>)>,
    pending: Option<RequestedLayout>,
}

#[derive(Clone, Copy)]
struct Dimensions {
    width: u16,
    height: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WireSecurity {
    None,
    VncAuth,
}

impl HardenedVncClient {
    pub async fn connect<S>(
        mut stream: S,
        options: HardenedVncOptions,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<Self, VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let clipboard_enabled = options.clipboard_enabled;
        let dimensions = handshake(&mut stream, options, &mut cancellation).await?;

        let (reader, writer) = tokio::io::split(stream);
        let (normal_tx, normal_rx) = mpsc::channel(INPUT_QUEUE_CAPACITY);
        let (priority_tx, priority_rx) = mpsc::channel(INPUT_QUEUE_CAPACITY);
        let (update_tx, update_rx) = mpsc::channel(INPUT_QUEUE_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        // The ServerInit dimensions establish the first framebuffer before a
        // server sends any rectangle. Queue this before the reader starts so
        // an initial Raw/CopyRect update can never race the surface setup.
        event_tx
            .try_send(Ok(VncEvent::SetResolution(
                (dimensions.width, dimensions.height).into(),
            )))
            .map_err(|_| VncError::Closed)?;
        let (stop_tx, stop_rx) = watch::channel(false);
        let resize = Arc::new(Mutex::new(ResizeState::default()));

        let reader_events = event_tx.clone();
        let reader_stop_tx = stop_tx.clone();
        let reader_stop = stop_rx.clone();
        let reader_resize = Arc::clone(&resize);
        let reader_task = tokio::spawn(async move {
            let result = read_loop(
                reader,
                dimensions,
                clipboard_enabled,
                update_tx,
                reader_events.clone(),
                reader_stop,
                reader_resize,
            )
            .await;
            if let Err(error) = result {
                let _ = reader_events.send(Err(error)).await;
                let _ = reader_stop_tx.send(true);
            }
        });

        let writer_events = event_tx;
        let writer_stop_tx = stop_tx.clone();
        let writer_stop = stop_rx;
        let writer_resize = Arc::clone(&resize);
        let writer_task = tokio::spawn(async move {
            let result = write_loop(
                writer,
                priority_rx,
                normal_rx,
                update_rx,
                writer_stop,
                writer_resize,
            )
            .await;
            if let Err(error) = result {
                let _ = writer_events.send(Err(error)).await;
                let _ = writer_stop_tx.send(true);
            }
        });

        Ok(Self {
            normal_tx,
            priority_tx,
            events: event_rx,
            stop_tx,
            reader: Some(reader_task),
            writer: Some(writer_task),
            resize,
        })
    }

    /// Completes only once the writer has finished the corresponding RFB
    /// bytes. A dropped caller future suppresses queued, not-yet-written input.
    pub async fn input(&self, events: Vec<X11Event>) -> Result<(), VncError> {
        self.dispatch(&self.normal_tx, events).await
    }

    /// High-priority input is reserved for focus-loss key/button releases.
    pub async fn priority_input(&self, events: Vec<X11Event>) -> Result<(), VncError> {
        self.dispatch(&self.priority_tx, events).await
    }

    /// A resize is permitted only after the server has returned an extended
    /// layout. This adapter changes a single full-frame screen and preserves
    /// its server-assigned ID and opaque flags.
    pub async fn resize(&self, width: u16, height: u16) -> Result<(), VncError> {
        validate_dimensions(width, height)?;
        {
            let state = self.resize.lock().unwrap();
            let Some((dimensions, screens)) = &state.layout else {
                return Err(VncError::UnsupportedOperation);
            };
            if screens.len() != 1
                || screens[0].x != 0
                || screens[0].y != 0
                || screens[0].width != dimensions.width
                || screens[0].height != dimensions.height
            {
                return Err(VncError::UnsupportedOperation);
            }
            if state.pending.is_some() {
                return Err(VncError::Timeout);
            }
        }
        let (completion_tx, completion_rx) = oneshot::channel();
        self.normal_tx
            .send(WriteRequest::Resize {
                width,
                height,
                completion: completion_tx,
            })
            .await
            .map_err(|_| VncError::Closed)?;
        completion_rx.await.map_err(|_| VncError::Closed)?
    }

    pub async fn next_event(&mut self) -> Result<VncEvent, VncError> {
        match self.events.recv().await {
            Some(event) => event,
            None => Err(VncError::Closed),
        }
    }

    /// Cancels workers and waits for them. A stuck peer cannot leave a decoder
    /// task behind: it is aborted after the bounded join interval.
    pub async fn shutdown(&mut self) {
        let _ = self.stop_tx.send(true);
        join_or_abort(&mut self.reader).await;
        join_or_abort(&mut self.writer).await;
    }

    async fn dispatch(
        &self,
        sender: &mpsc::Sender<WriteRequest>,
        events: Vec<X11Event>,
    ) -> Result<(), VncError> {
        let (completion_tx, completion_rx) = oneshot::channel();
        sender
            .send(WriteRequest::Input {
                events,
                completion: completion_tx,
            })
            .await
            .map_err(|_| VncError::Closed)?;
        completion_rx.await.map_err(|_| VncError::Closed)?
    }
}

impl Drop for HardenedVncClient {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(true);
        if let Some(task) = self.reader.take() {
            task.abort();
        }
        if let Some(task) = self.writer.take() {
            task.abort();
        }
    }
}

async fn join_or_abort(task: &mut Option<JoinHandle<()>>) {
    if let Some(mut task) = task.take() {
        if timeout(TASK_JOIN_TIMEOUT, &mut task).await.is_err() {
            task.abort();
            let _ = task.await;
        }
    }
}

async fn handshake<S>(
    stream: &mut S,
    options: HardenedVncOptions,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<Dimensions, VncError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let HardenedVncOptions {
        mut password,
        allow_unauthenticated,
        clipboard_enabled: _,
        version,
    } = options;

    let result = async {
        let mut greeting = [0_u8; 12];
        read_exact(stream, &mut greeting, cancellation).await?;
        let version = negotiate_version(greeting, version)?;
        write_all(stream, version_bytes(version), cancellation).await?;

        let security =
            choose_security(stream, version, allow_unauthenticated, cancellation).await?;
        if security == WireSecurity::VncAuth {
            authenticate_vnc(stream, password.as_bytes(), cancellation).await?;
        }
        password.zeroize();
        read_security_result(stream, version, security, cancellation).await?;

        write_all(stream, &[1], cancellation).await?;
        let dimensions = read_server_init(stream, cancellation).await?;
        write_message(
            stream,
            ClientMsg::SetPixelFormat(PixelFormat::bgra()),
            cancellation,
        )
        .await?;
        write_message(
            stream,
            ClientMsg::SetEncodings(vec![
                VncEncoding::Tight,
                VncEncoding::Zrle,
                VncEncoding::CopyRect,
                VncEncoding::Raw,
                VncEncoding::DesktopSizePseudo,
                VncEncoding::ExtendedDesktopSizePseudo,
            ]),
            cancellation,
        )
        .await?;
        write_message(
            stream,
            ClientMsg::FramebufferUpdateRequest(
                Rect {
                    x: 0,
                    y: 0,
                    width: dimensions.width,
                    height: dimensions.height,
                },
                0,
            ),
            cancellation,
        )
        .await?;
        Ok(dimensions)
    }
    .await;
    password.zeroize();
    result
}

async fn choose_security<S>(
    stream: &mut S,
    version: VncVersion,
    allow_unauthenticated: bool,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<WireSecurity, VncError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let offered = if version == VncVersion::RFB33 {
        let value = read_u32(stream, cancellation).await?;
        match value {
            0 => {
                discard_failure_reason(stream, cancellation).await?;
                return Err(VncError::AuthenticationRejected);
            }
            1 => vec![WireSecurity::None],
            2 => vec![WireSecurity::VncAuth],
            _ => return Err(VncError::UnsupportedAuthentication),
        }
    } else {
        let count = read_u8(stream, cancellation).await?;
        if count == 0 {
            discard_failure_reason(stream, cancellation).await?;
            return Err(VncError::AuthenticationRejected);
        }
        let mut offered = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            match read_u8(stream, cancellation).await? {
                1 => offered.push(WireSecurity::None),
                2 => offered.push(WireSecurity::VncAuth),
                _ => {}
            }
        }
        offered
    };

    let security = if offered.contains(&WireSecurity::VncAuth) {
        WireSecurity::VncAuth
    } else if allow_unauthenticated && offered.contains(&WireSecurity::None) {
        WireSecurity::None
    } else if offered.contains(&WireSecurity::None) {
        return Err(VncError::AuthenticationRejected);
    } else {
        return Err(VncError::UnsupportedAuthentication);
    };

    if version != VncVersion::RFB33 {
        let byte = match security {
            WireSecurity::None => 1,
            WireSecurity::VncAuth => 2,
        };
        write_all(stream, &[byte], cancellation).await?;
    }
    Ok(security)
}

async fn authenticate_vnc<S>(
    stream: &mut S,
    password: &[u8],
    cancellation: &mut watch::Receiver<bool>,
) -> Result<(), VncError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut challenge = [0_u8; 16];
    read_exact(stream, &mut challenge, cancellation).await?;
    let mut key = [0_u8; 8];
    for (index, byte) in password.iter().copied().take(key.len()).enumerate() {
        key[index] = byte.reverse_bits();
    }
    let mut response = security::des::encrypt(&challenge, &key);
    let result = write_all(stream, response.as_slice(), cancellation).await;
    key.zeroize();
    response.zeroize();
    result
}

async fn read_security_result<S>(
    stream: &mut S,
    version: VncVersion,
    security: WireSecurity,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<(), VncError>
where
    S: AsyncRead + Unpin,
{
    if security == WireSecurity::None && version != VncVersion::RFB38 {
        return Ok(());
    }
    if read_u32(stream, cancellation).await? == 0 {
        return Ok(());
    }
    if version == VncVersion::RFB38 {
        discard_failure_reason(stream, cancellation).await?;
    }
    Err(VncError::AuthenticationRejected)
}

async fn read_server_init<S>(
    stream: &mut S,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<Dimensions, VncError>
where
    S: AsyncRead + Unpin,
{
    let width = read_u16(stream, cancellation).await?;
    let height = read_u16(stream, cancellation).await?;
    let dimensions = validate_dimensions(width, height)?;

    let mut pixel_format = [0_u8; 16];
    read_exact(stream, &mut pixel_format, cancellation).await?;
    let _ = PixelFormat::try_from(pixel_format)?;

    let name_length = read_u32(stream, cancellation).await? as usize;
    if name_length > MAX_SERVER_NAME_BYTES {
        return Err(VncError::ResourceLimit);
    }
    discard_exact(stream, name_length, cancellation).await?;
    Ok(dimensions)
}

async fn read_loop<R>(
    mut reader: R,
    mut dimensions: Dimensions,
    clipboard_enabled: bool,
    update_tx: mpsc::Sender<WriteRequest>,
    event_tx: mpsc::Sender<Result<VncEvent, VncError>>,
    mut stop: watch::Receiver<bool>,
    resize: Arc<Mutex<ResizeState>>,
) -> Result<(), VncError>
where
    R: AsyncRead + Unpin,
{
    let mut raw = codec::RawDecoder::new();
    let mut zrle = codec::ZrleDecoder::new();
    let mut tight = codec::TightDecoder::new();
    let emit = |event: VncEvent| {
        let event_tx = event_tx.clone();
        async move { event_tx.send(Ok(event)).await.map_err(|_| VncError::Closed) }
    };

    loop {
        match read_u8(&mut reader, &mut stop).await? {
            0 => {
                let _padding = read_u8(&mut reader, &mut stop).await?;
                let rectangles = read_u16(&mut reader, &mut stop).await?;
                for _ in 0..rectangles {
                    let rect = read_rect(&mut reader, &mut stop).await?;
                    let encoding = VncEncoding::try_from(read_i32(&mut reader, &mut stop).await?)?;
                    match encoding {
                        VncEncoding::Raw => {
                            validate_rect(rect, dimensions)?;
                            cancelable(
                                raw.decode(&PixelFormat::bgra(), &rect, &mut reader, &emit),
                                &stop,
                            )
                            .await?;
                        }
                        VncEncoding::CopyRect => {
                            validate_rect(rect, dimensions)?;
                            let source = Rect {
                                x: read_u16(&mut reader, &mut stop).await?,
                                y: read_u16(&mut reader, &mut stop).await?,
                                width: rect.width,
                                height: rect.height,
                            };
                            validate_rect(source, dimensions)?;
                            emit(VncEvent::Copy(rect, source)).await?;
                        }
                        VncEncoding::Tight => {
                            validate_rect(rect, dimensions)?;
                            cancelable(
                                tight.decode(&PixelFormat::bgra(), &rect, &mut reader, &emit),
                                &stop,
                            )
                            .await?;
                        }
                        VncEncoding::Zrle => {
                            validate_rect(rect, dimensions)?;
                            cancelable(
                                zrle.decode(&PixelFormat::bgra(), &rect, &mut reader, &emit),
                                &stop,
                            )
                            .await?;
                        }
                        VncEncoding::DesktopSizePseudo => {
                            if rect.x != 0 || rect.y != 0 {
                                return Err(VncError::Protocol);
                            }
                            dimensions = validate_dimensions(rect.width, rect.height)?;
                            resize.lock().unwrap().layout = None;
                            emit(VncEvent::SetResolution(
                                (dimensions.width, dimensions.height).into(),
                            ))
                            .await?;
                        }
                        VncEncoding::ExtendedDesktopSizePseudo => {
                            let screens = read_extended_screens(&mut reader, &mut stop).await?;
                            let mut result = None;
                            if rect.x == 1 {
                                let mut state = resize.lock().unwrap();
                                if rect.y == 4 {
                                    if let Some(pending) = state.pending.as_mut() {
                                        pending.forwarded = true;
                                    }
                                } else if let Some(pending) = state.pending.take() {
                                    let applied = rect.y == 0
                                        && rect.width == pending.width
                                        && rect.height == pending.height
                                        && screens.as_slice()
                                            == [ScreenInfo {
                                                id: pending.screen_id,
                                                x: 0,
                                                y: 0,
                                                width: pending.width,
                                                height: pending.height,
                                                flags: pending.flags,
                                            }];
                                    result = Some((applied, rect.y));
                                }
                            } else if let Some(pending) = resize.lock().unwrap().pending {
                                if pending.forwarded
                                    && rect.width == pending.width
                                    && rect.height == pending.height
                                {
                                    resize.lock().unwrap().pending = None;
                                    result = Some((true, 0));
                                }
                            }
                            if rect.x != 1 || rect.y == 0 {
                                dimensions = validate_dimensions(rect.width, rect.height)?;
                                resize.lock().unwrap().layout = Some((dimensions, screens));
                                emit(VncEvent::SetResolution(
                                    (dimensions.width, dimensions.height).into(),
                                ))
                                .await?;
                            }
                            if let Some((applied, status)) = result {
                                emit(VncEvent::ResizeResult { applied, status }).await?;
                            }
                        }
                        // Cursor is intentionally not negotiated because the
                        // shared desktop contract has no cursor-shape event.
                        VncEncoding::CursorPseudo
                        | VncEncoding::LastRectPseudo
                        | VncEncoding::Trle => return Err(VncError::UnsupportedOperation),
                    }
                }
                emit(VncEvent::FramebufferUpdateComplete).await?;
                send_with_stop(
                    &update_tx,
                    WriteRequest::Update {
                        width: dimensions.width,
                        height: dimensions.height,
                        incremental: true,
                    },
                    &mut stop,
                )
                .await?;
            }
            1 => return Err(VncError::UnsupportedOperation),
            2 => emit(VncEvent::Bell).await?,
            3 => {
                let mut padding = [0_u8; 3];
                read_exact(&mut reader, &mut padding, &mut stop).await?;
                let length = read_u32(&mut reader, &mut stop).await? as usize;
                if length > MAX_TEXT_BYTES {
                    return Err(VncError::ResourceLimit);
                }
                let text = read_latin1(&mut reader, length, &mut stop).await?;
                if clipboard_enabled {
                    emit(VncEvent::Text(text)).await?;
                }
            }
            _ => return Err(VncError::Protocol),
        }
    }
}

async fn write_loop<W>(
    mut writer: W,
    mut priority_rx: mpsc::Receiver<WriteRequest>,
    mut normal_rx: mpsc::Receiver<WriteRequest>,
    mut update_rx: mpsc::Receiver<WriteRequest>,
    mut stop: watch::Receiver<bool>,
    resize: Arc<Mutex<ResizeState>>,
) -> Result<(), VncError>
where
    W: AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            biased;
            () = cancelled(&mut stop) => return Ok(()),
            Some(request) = priority_rx.recv() => write_request(&mut writer, request, &resize).await?,
            Some(request) = normal_rx.recv() => write_request(&mut writer, request, &resize).await?,
            Some(request) = update_rx.recv() => write_request(&mut writer, request, &resize).await?,
        }
    }
}

async fn write_request<W>(
    writer: &mut W,
    request: WriteRequest,
    resize: &Arc<Mutex<ResizeState>>,
) -> Result<(), VncError>
where
    W: AsyncWrite + Unpin,
{
    match request {
        WriteRequest::Input { events, completion } => {
            // If the caller was cancelled before the writer started, the
            // command must not reach the peer. Once started, finish the whole
            // batch so synthesized text key-up events cannot be stranded.
            if completion.is_closed() {
                return Ok(());
            }
            let result = write_input_events(writer, events).await;
            if !completion.is_closed() {
                let _ = completion.send(result);
            }
            result
        }
        WriteRequest::Update {
            width,
            height,
            incremental,
        } => {
            ClientMsg::FramebufferUpdateRequest(
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                u8::from(incremental),
            )
            .write(writer)
            .await
        }
        WriteRequest::Resize {
            width,
            height,
            completion,
        } => {
            if completion.is_closed() {
                return Ok(());
            }
            let request = {
                let mut state = resize.lock().unwrap();
                let Some((dimensions, screens)) = &state.layout else {
                    let _ = completion.send(Err(VncError::UnsupportedOperation));
                    return Ok(());
                };
                if state.pending.is_some()
                    || screens.len() != 1
                    || screens[0].x != 0
                    || screens[0].y != 0
                    || screens[0].width != dimensions.width
                    || screens[0].height != dimensions.height
                {
                    let _ = completion.send(Err(VncError::UnsupportedOperation));
                    return Ok(());
                }
                let request = RequestedLayout {
                    width,
                    height,
                    screen_id: screens[0].id,
                    flags: screens[0].flags,
                    forwarded: false,
                };
                state.pending = Some(request);
                request
            };
            let result = ClientMsg::SetDesktopSize {
                width,
                height,
                screen_id: request.screen_id,
                flags: request.flags,
            }
            .write(writer)
            .await;
            if result.is_err() {
                resize.lock().unwrap().pending = None;
            }
            let completed = result.is_ok();
            let _ = completion.send(result);
            if completed {
                Ok(())
            } else {
                Err(VncError::ConnectionLost)
            }
        }
    }
}

async fn write_input_events<W>(writer: &mut W, events: Vec<X11Event>) -> Result<(), VncError>
where
    W: AsyncWrite + Unpin,
{
    for event in events {
        let message = match event {
            X11Event::Refresh | X11Event::FullRefresh => {
                return Err(VncError::UnsupportedOperation);
            }
            X11Event::KeyEvent(key) => ClientMsg::KeyEvent(key.keycode, key.down),
            X11Event::PointerEvent(pointer) => {
                ClientMsg::PointerEvent(pointer.position_x, pointer.position_y, pointer.bottons)
            }
            X11Event::CopyText(text) => {
                if text.len() > MAX_TEXT_BYTES || text.contains('\0') {
                    return Err(VncError::ResourceLimit);
                }
                ClientMsg::ClientCutText(text)
            }
        };
        message.write(writer).await?;
    }
    Ok(())
}

fn parse_version(greeting: [u8; 12]) -> Result<VncVersion, VncError> {
    match &greeting {
        b"RFB 003.003\n" => Ok(VncVersion::RFB33),
        b"RFB 003.007\n" => Ok(VncVersion::RFB37),
        b"RFB 003.008\n" => Ok(VncVersion::RFB38),
        // The hardened adapter deliberately has no best-effort fallback for
        // arbitrary RFB banners: its security-result rules are versioned and
        // accepting an unknown version as 3.3 could skip an authentication
        // result sent by the peer.
        _ => Err(VncError::Protocol),
    }
}

fn negotiate_version(
    greeting: [u8; 12],
    requested: Option<VncVersion>,
) -> Result<VncVersion, VncError> {
    let Some(requested) = requested else {
        return parse_version(greeting);
    };
    if &greeting[..4] != b"RFB " || greeting[7] != b'.' || greeting[11] != b'\n' {
        return Err(VncError::Protocol);
    }
    let digits = [&greeting[4..7], &greeting[8..11]];
    if !digits
        .iter()
        .all(|part| part.iter().all(u8::is_ascii_digit))
    {
        return Err(VncError::Protocol);
    }
    let major = u16::from(greeting[4] - b'0') * 100
        + u16::from(greeting[5] - b'0') * 10
        + u16::from(greeting[6] - b'0');
    let minor = u16::from(greeting[8] - b'0') * 100
        + u16::from(greeting[9] - b'0') * 10
        + u16::from(greeting[10] - b'0');
    let minimum = match requested {
        VncVersion::RFB33 => 3,
        VncVersion::RFB37 => 7,
        VncVersion::RFB38 => 8,
    };
    if major != 3 || minor < minimum {
        return Err(VncError::Protocol);
    }
    Ok(requested)
}

fn version_bytes(version: VncVersion) -> &'static [u8; 12] {
    version.into()
}

fn validate_dimensions(width: u16, height: u16) -> Result<Dimensions, VncError> {
    let pixels = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or(VncError::ResourceLimit)?;
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || pixels > MAX_PIXELS
    {
        return Err(VncError::ResourceLimit);
    }
    Ok(Dimensions { width, height })
}

fn validate_rect(rect: Rect, dimensions: Dimensions) -> Result<(), VncError> {
    if rect.width == 0
        || rect.height == 0
        || u32::from(rect.x) + u32::from(rect.width) > u32::from(dimensions.width)
        || u32::from(rect.y) + u32::from(rect.height) > u32::from(dimensions.height)
    {
        return Err(VncError::Protocol);
    }
    Ok(())
}

async fn discard_failure_reason<R>(
    reader: &mut R,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<(), VncError>
where
    R: AsyncRead + Unpin,
{
    let length = read_u32(reader, cancellation).await? as usize;
    if length > MAX_SERVER_NAME_BYTES {
        return Err(VncError::ResourceLimit);
    }
    discard_exact(reader, length, cancellation).await
}

async fn read_rect<R>(
    reader: &mut R,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<Rect, VncError>
where
    R: AsyncRead + Unpin,
{
    Ok(Rect {
        x: read_u16(reader, cancellation).await?,
        y: read_u16(reader, cancellation).await?,
        width: read_u16(reader, cancellation).await?,
        height: read_u16(reader, cancellation).await?,
    })
}

async fn read_extended_screens<R>(
    reader: &mut R,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<Vec<ScreenInfo>, VncError>
where
    R: AsyncRead + Unpin,
{
    let count = read_u8(reader, cancellation).await?;
    let mut padding = [0_u8; 3];
    read_exact(reader, &mut padding, cancellation).await?;
    let mut screens = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        screens.push(ScreenInfo {
            id: read_u32(reader, cancellation).await?,
            x: read_u16(reader, cancellation).await?,
            y: read_u16(reader, cancellation).await?,
            width: read_u16(reader, cancellation).await?,
            height: read_u16(reader, cancellation).await?,
            flags: read_u32(reader, cancellation).await?,
        });
    }
    Ok(screens)
}

async fn read_latin1<R>(
    reader: &mut R,
    length: usize,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<String, VncError>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = vec![0_u8; length];
    read_exact(reader, &mut bytes, cancellation).await?;
    if bytes.contains(&0) {
        return Err(VncError::Protocol);
    }
    Ok(bytes.into_iter().map(char::from).collect())
}

async fn discard_exact<R>(
    reader: &mut R,
    mut length: usize,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<(), VncError>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 1024];
    while length > 0 {
        let take = length.min(buffer.len());
        read_exact(reader, &mut buffer[..take], cancellation).await?;
        length -= take;
    }
    Ok(())
}

async fn read_u8<R>(
    reader: &mut R,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<u8, VncError>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = [0_u8; 1];
    read_exact(reader, &mut bytes, cancellation).await?;
    Ok(bytes[0])
}

async fn read_u16<R>(
    reader: &mut R,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<u16, VncError>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = [0_u8; 2];
    read_exact(reader, &mut bytes, cancellation).await?;
    Ok(u16::from_be_bytes(bytes))
}

async fn read_u32<R>(
    reader: &mut R,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<u32, VncError>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = [0_u8; 4];
    read_exact(reader, &mut bytes, cancellation).await?;
    Ok(u32::from_be_bytes(bytes))
}

async fn read_i32<R>(
    reader: &mut R,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<i32, VncError>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = [0_u8; 4];
    read_exact(reader, &mut bytes, cancellation).await?;
    Ok(i32::from_be_bytes(bytes))
}

async fn read_exact<R>(
    reader: &mut R,
    bytes: &mut [u8],
    cancellation: &mut watch::Receiver<bool>,
) -> Result<(), VncError>
where
    R: AsyncRead + Unpin,
{
    tokio::select! {
        biased;
        () = cancelled(cancellation) => Err(VncError::Cancelled),
        result = reader.read_exact(bytes) => result.map(|_| ()).map_err(Into::into),
    }
}

async fn write_all<W>(
    writer: &mut W,
    bytes: &[u8],
    cancellation: &mut watch::Receiver<bool>,
) -> Result<(), VncError>
where
    W: AsyncWrite + Unpin,
{
    tokio::select! {
        biased;
        () = cancelled(cancellation) => Err(VncError::Cancelled),
        result = writer.write_all(bytes) => result.map_err(Into::into),
    }
}

async fn write_message<W>(
    writer: &mut W,
    message: ClientMsg,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<(), VncError>
where
    W: AsyncWrite + Unpin,
{
    tokio::select! {
        biased;
        () = cancelled(cancellation) => Err(VncError::Cancelled),
        result = message.write(writer) => result,
    }
}

async fn send_with_stop<T>(
    sender: &mpsc::Sender<T>,
    value: T,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<(), VncError>
where
    T: Send,
{
    tokio::select! {
        biased;
        () = cancelled(cancellation) => Err(VncError::Cancelled),
        result = sender.send(value) => result.map_err(|_| VncError::Closed),
    }
}

/// Upstream codecs own the reader while consuming an encoded rectangle. Race
/// that whole operation against the session stop signal so a peer that sends a
/// partial Raw/Tight/ZRLE payload cannot stretch shutdown to the join timeout.
async fn cancelable<T, F>(future: F, stop: &watch::Receiver<bool>) -> Result<T, VncError>
where
    F: Future<Output = Result<T, VncError>>,
{
    let mut cancellation = stop.clone();
    tokio::select! {
        biased;
        () = cancelled(&mut cancellation) => Err(VncError::Cancelled),
        result = future => result,
    }
}

async fn cancelled(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_or_unbounded_greetings_and_sizes() {
        assert_eq!(parse_version(*b"RFB 003.008\n"), Ok(VncVersion::RFB38));
        assert_eq!(parse_version(*b"NOPE003.008\n"), Err(VncError::Protocol));
        assert_eq!(parse_version(*b"RFB 003.889\n"), Err(VncError::Protocol));
        assert_eq!(
            negotiate_version(*b"RFB 003.889\n", Some(VncVersion::RFB33)),
            Ok(VncVersion::RFB33)
        );
        assert_eq!(
            negotiate_version(*b"RFB 003.003\n", Some(VncVersion::RFB38)),
            Err(VncError::Protocol)
        );
        assert_eq!(
            negotiate_version(*b"RFB 003.889\n", None),
            Err(VncError::Protocol)
        );
        assert!(matches!(
            validate_dimensions(0, 1),
            Err(VncError::ResourceLimit)
        ));
        assert!(matches!(
            validate_dimensions(u16::MAX, u16::MAX),
            Err(VncError::ResourceLimit)
        ));
    }
}
