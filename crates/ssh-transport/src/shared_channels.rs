//! Child channels on a user-owned authenticated connection. Closing any value
//! here closes only its channel/listener; none can disconnect the parent.

use super::*;
mod forward;
pub use forward::SharedRemoteForward;
pub(super) use forward::SharedRemoteRouter;

#[derive(Clone)]
pub struct SharedSessionChannels {
    handle: client::SessionChannelHandle,
    closed: watch::Receiver<bool>,
    timeouts: ConnectionTimeouts,
    remote: Arc<Mutex<SharedRemoteRouter>>,
    pending_opens: Arc<tokio::sync::Semaphore>,
}

impl fmt::Debug for SharedSessionChannels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedSessionChannels")
            .finish_non_exhaustive()
    }
}

impl<V: HostKeyVerifier> AuthenticatedTransport<V> {
    #[must_use]
    pub fn shared_channels(&self) -> SharedSessionChannels {
        SharedSessionChannels {
            handle: self.session.session_channel_handle(),
            closed: self.transport_close_observer.clone(),
            timeouts: self.timeouts,
            remote: Arc::clone(&self.shared_remote_forwards),
            pending_opens: Arc::new(tokio::sync::Semaphore::new(64)),
        }
    }
}

impl SharedSessionChannels {
    #[must_use]
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow() || self.handle.is_closed()
    }

    pub async fn wait_closed(&self) {
        let mut closed = self.closed.clone();
        while !*closed.borrow() {
            if closed.changed().await.is_err() {
                break;
            }
        }
    }

    async fn session_channel(&self) -> Result<Channel<client::Msg>> {
        if self.is_closed() {
            return Err(TransportError::ConnectionLost);
        }
        let permit = timeout(
            self.timeouts.channel_open,
            self.pending_opens.clone().acquire_owned(),
        )
        .await
        .map_err(|_| TransportError::ChannelOpenTimeout)?
        .map_err(|_| TransportError::ConnectionLost)?;
        let handle = self.handle.clone();
        let deadline = self.timeouts.channel_request;
        let (send, receive) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _permit = permit;
            let result = handle
                .channel_open_session()
                .await
                .map_err(map_protocol_error);
            if let Err(Ok(channel)) = send.send(result) {
                let _ = timeout(deadline, channel.close()).await;
            }
        });
        timeout(self.timeouts.channel_open, receive)
            .await
            .map_err(|_| TransportError::ChannelOpenTimeout)?
            .map_err(|_| TransportError::ConnectionLost)?
    }

    pub async fn execute_capture_request(
        &self,
        command: &[u8],
        stdin: Option<&[u8]>,
        deadline: Duration,
        maximum: usize,
        before_dispatch: impl FnOnce() -> bool,
        mut cancelled: watch::Receiver<bool>,
    ) -> Result<RemoteExecOutput> {
        if command.is_empty()
            || command.len() > 64 * 1024
            || command.contains(&0)
            || stdin.is_some_and(|value| value.len() > 256 * 1024)
            || deadline.is_zero()
            || maximum == 0
        {
            return Err(TransportError::InvalidRemoteExecCommand);
        }
        let channel = tokio::select! {
            biased;
            () = cancellation(&mut cancelled) => return Err(TransportError::RemoteExecRejected),
            value = self.session_channel() => value?,
        };
        let mut cleanup =
            ChildChannelGuard::new(channel.write_handle(), self.timeouts.channel_request);
        let (mut reader, writer) = channel.split();
        let execution = async {
            if *cancelled.borrow() || !before_dispatch() {
                return Err(TransportError::RemoteExecRejected);
            }
            writer
                .exec(true, command)
                .await
                .map_err(map_protocol_error)?;
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut exit_status = None;
            let reading = async {
                while let Some(message) = reader.wait().await {
                    match message {
                        ChannelMsg::Data { data } => {
                            extend_bounded(&mut stdout, &stderr, &data, maximum)?
                        }
                        ChannelMsg::ExtendedData { data, .. } => {
                            extend_bounded(&mut stderr, &stdout, &data, maximum)?
                        }
                        ChannelMsg::ExitStatus { exit_status: value } => exit_status = Some(value),
                        ChannelMsg::Failure => return Err(TransportError::RemoteExecRejected),
                        _ => {}
                    }
                }
                Ok(RemoteExecOutput {
                    stdout,
                    stderr,
                    exit_status,
                })
            };
            let writing = async {
                if let Some(stdin) = stdin {
                    writer.data(stdin).await.map_err(map_protocol_error)?;
                }
                writer.eof().await.map_err(map_protocol_error)
            };
            let (_, output) = tokio::try_join!(writing, reading)?;
            Ok(output)
        };
        let mut execution_cancel = cancelled.clone();
        let result = tokio::select! {
            biased;
            () = cancellation(&mut execution_cancel) => Err(TransportError::RemoteExecRejected),
            () = self.wait_closed() => Err(TransportError::ConnectionLost),
            result = timeout(deadline, execution) => result.unwrap_or(Err(TransportError::RemoteExecTimeout)),
        };
        // The child is always closed; timeout never poisons the user's transport.
        cleanup.close().await?;
        result
    }

    pub async fn open_sftp_channel(&self) -> Result<SharedSftpChannel> {
        let mut channel = self.session_channel().await?;
        let cleanup = ChildChannelGuard::new(channel.write_handle(), self.timeouts.channel_request);
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(map_protocol_error)?;
        await_channel_request_reply(
            &mut channel,
            self.timeouts.channel_request,
            ChannelRequestKind::SftpSubsystem,
            &mut VecDeque::new(),
        )
        .await?;
        let sftp = timeout(
            self.timeouts.channel_request,
            russh_sftp::client::SftpSession::new(channel.into_stream()),
        )
        .await
        .map_err(|_| TransportError::SftpSubsystemTimeout)?
        .map_err(|_| TransportError::SftpProtocol)?;
        Ok(SharedSftpChannel {
            cleanup,
            sftp,
            parent: self.clone(),
        })
    }

    pub async fn open_terminal_command(
        &self,
        size: PtySize,
        command: &str,
        before_dispatch: impl FnOnce() -> bool,
    ) -> Result<SharedTerminalChannel> {
        let size = size.validate()?;
        let mut channel = self.session_channel().await?;
        let cleanup = ChildChannelGuard::new(channel.write_handle(), self.timeouts.channel_request);
        let mut pending = VecDeque::new();
        channel
            .request_pty(
                true,
                "xterm-256color",
                size.columns,
                size.rows,
                size.pixel_width,
                size.pixel_height,
                &[],
            )
            .await
            .map_err(map_protocol_error)?;
        await_channel_request_reply(
            &mut channel,
            self.timeouts.channel_request,
            ChannelRequestKind::Pty,
            &mut pending,
        )
        .await?;
        if !before_dispatch() {
            let _ = channel.close().await;
            return Err(TransportError::RemoteExecRejected);
        }
        channel
            .exec(true, command.as_bytes())
            .await
            .map_err(map_protocol_error)?;
        await_channel_request_reply(
            &mut channel,
            self.timeouts.channel_request,
            ChannelRequestKind::Exec,
            &mut pending,
        )
        .await?;
        let (read, write) = channel.split();
        Ok(SharedTerminalChannel {
            cleanup,
            read,
            write,
            pending,
            parent: self.clone(),
        })
    }

    pub async fn open_direct_tcpip(
        &self,
        target: &str,
        port: u16,
        originator: &str,
        originator_port: u16,
    ) -> Result<ForwardChannel> {
        if self.is_closed() || target.is_empty() || port == 0 {
            return Err(TransportError::ConnectionLost);
        }
        let permit = timeout(
            self.timeouts.channel_open,
            self.pending_opens.clone().acquire_owned(),
        )
        .await
        .map_err(|_| TransportError::ForwardChannelOpenTimeout)?
        .map_err(|_| TransportError::ConnectionLost)?;
        let handle = self.handle.clone();
        let deadline = self.timeouts.channel_request;
        let target = target.to_owned();
        let originator = originator.to_owned();
        let (send, receive) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _permit = permit;
            let result = handle
                .channel_open_direct_tcpip(
                    target,
                    u32::from(port),
                    originator,
                    u32::from(originator_port),
                )
                .await
                .map_err(map_protocol_error);
            if let Err(Ok(channel)) = send.send(result) {
                let _ = timeout(deadline, channel.close()).await;
            }
        });
        let channel = timeout(self.timeouts.channel_open, receive)
            .await
            .map_err(|_| TransportError::ForwardChannelOpenTimeout)?
            .map_err(|_| TransportError::ConnectionLost)??;
        Ok(ForwardChannel {
            stream: channel.into_stream(),
        })
    }
}

async fn cancellation(cancelled: &mut watch::Receiver<bool>) {
    while !*cancelled.borrow() {
        if cancelled.changed().await.is_err() {
            return;
        }
    }
}

pub struct SharedSftpChannel {
    cleanup: ChildChannelGuard,
    sftp: russh_sftp::client::SftpSession,
    parent: SharedSessionChannels,
}
impl SharedSftpChannel {
    pub fn client(&self) -> &russh_sftp::client::SftpSession {
        &self.sftp
    }
    pub fn is_closed(&self) -> bool {
        self.parent.is_closed()
    }
    pub async fn wait_closed(&self) {
        self.parent.wait_closed().await;
    }
    pub async fn close(mut self) -> Result<()> {
        if self.parent.is_closed() {
            return Ok(());
        }
        let result = timeout(self.parent.timeouts.channel_request, self.sftp.close()).await;
        if self.parent.is_closed() {
            return Ok(());
        }
        let closed = self.cleanup.close().await;
        result
            .map_err(|_| TransportError::SftpSubsystemTimeout)?
            .map_err(|_| TransportError::SftpProtocol)?;
        closed
    }
}

pub struct SharedTerminalChannel {
    cleanup: ChildChannelGuard,
    read: ChannelReadHalf,
    write: ChannelWriteHalf<client::Msg>,
    pending: VecDeque<ChannelMsg>,
    parent: SharedSessionChannels,
}
impl SharedTerminalChannel {
    pub async fn send_input(&self, data: impl Into<Bytes>) -> Result<()> {
        self.write
            .data_bytes(data)
            .await
            .map_err(map_protocol_error)
    }
    pub async fn resize(&self, size: PtySize) -> Result<()> {
        let size = size.validate()?;
        self.write
            .window_change(size.columns, size.rows, size.pixel_width, size.pixel_height)
            .await
            .map_err(map_protocol_error)
    }
    pub async fn next_event(&mut self) -> Result<ShellEvent> {
        if let Some(event) = self.pending.pop_front() {
            return Ok(ShellEvent::from(event));
        }
        tokio::select! { () = self.parent.wait_closed() => Err(TransportError::ConnectionLost), event = self.read.wait() => event.map(ShellEvent::from).ok_or(TransportError::ConnectionLost) }
    }
    pub async fn disconnect(mut self) -> Result<()> {
        if self.parent.is_closed() {
            return Ok(());
        }
        self.cleanup.close().await
    }
}

struct ChildChannelGuard {
    writer: Option<ChannelWriteHalf<client::Msg>>,
    deadline: Duration,
}
impl ChildChannelGuard {
    fn new(writer: ChannelWriteHalf<client::Msg>, deadline: Duration) -> Self {
        Self {
            writer: Some(writer),
            deadline,
        }
    }
    async fn close(&mut self) -> Result<()> {
        let Some(writer) = self.writer.as_ref() else {
            return Ok(());
        };
        timeout(self.deadline, writer.close())
            .await
            .map_err(|_| TransportError::DisconnectTimeout)?
            .map_err(map_protocol_error)?;
        self.writer.take();
        Ok(())
    }
}
impl Drop for ChildChannelGuard {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            let deadline = self.deadline;
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = timeout(deadline, writer.close()).await;
                });
            }
        }
    }
}
