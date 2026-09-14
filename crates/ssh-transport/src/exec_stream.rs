//! One dynamically authorized, non-PTY SSH exec stream.
//!
//! The caller transfers an already authenticated [`RemoteExecTransport`] into this type. The
//! transport crate never decides whether a command is authorized; it only runs the final caller
//! fence immediately before `exec` and bounds the resulting byte streams.

use std::time::Duration;

use bytes::Bytes;
use russh::{ChannelMsg, client};
use tokio::time::timeout;

use super::{
    ChannelReadHalf, ChannelWriteHalf, Disconnect, HostKeyVerifier, RemoteExecTransport, Result,
    TransportError, complete_disconnect_within, map_protocol_error,
};

/// Limits are deliberately per stream so stdout cannot consume stderr's evidence budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteExecStreamLimits {
    pub max_stdin_bytes: usize,
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
}

impl RemoteExecStreamLimits {
    pub fn validate(self) -> Result<()> {
        if self.max_stdin_bytes == 0 || self.max_stdout_bytes == 0 || self.max_stderr_bytes == 0 {
            return Err(TransportError::InvalidRemoteExecCommand);
        }
        Ok(())
    }
}

/// One message from the remote exec channel. `Eof` is terminal for this stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteExecStreamEvent {
    Stdout(Bytes),
    Stderr(Bytes),
    ExitStatus(Option<u32>),
    Eof,
}

/// Consumes an authenticated exec transport and owns exactly one non-PTY exec channel.
pub struct RemoteExecStream<V>
where
    V: HostKeyVerifier,
{
    session: client::Handle<super::ClientHandler<V>>,
    reader: ChannelReadHalf,
    writer: ChannelWriteHalf<client::Msg>,
    limits: RemoteExecStreamLimits,
    stdin_bytes: usize,
    stdout_bytes: usize,
    stderr_bytes: usize,
    stdin_closed: bool,
    finished: bool,
    disconnect_timeout: Duration,
}

impl<V> std::fmt::Debug for RemoteExecStream<V>
where
    V: HostKeyVerifier,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteExecStream")
            .field("stdin_closed", &self.stdin_closed)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl<V> RemoteExecTransport<V>
where
    V: HostKeyVerifier,
{
    /// Opens one fresh session channel. No PTY or shell request is made. `before_dispatch` runs
    /// only after channel creation and immediately before the command crosses the SSH boundary.
    pub async fn open_stream(
        mut self,
        command: &[u8],
        limits: RemoteExecStreamLimits,
        before_dispatch: impl FnOnce() -> bool,
    ) -> Result<RemoteExecStream<V>> {
        limits.validate()?;
        if self.poisoned || command.is_empty() || command.len() > 16 * 1024 || command.contains(&0)
        {
            return Err(if self.poisoned {
                TransportError::ConnectionLost
            } else {
                TransportError::InvalidRemoteExecCommand
            });
        }
        let channel = self
            .session
            .channel_open_session()
            .await
            .map_err(map_protocol_error)?;
        if !before_dispatch() {
            let _ = channel.close().await;
            self.poison().await;
            return Err(TransportError::RemoteExecRejected);
        }
        if let Err(error) = channel
            .exec(true, command)
            .await
            .map_err(map_protocol_error)
        {
            self.poison().await;
            return Err(error);
        }
        let (reader, writer) = channel.split();
        Ok(RemoteExecStream {
            session: self.session,
            reader,
            writer,
            limits,
            stdin_bytes: 0,
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdin_closed: false,
            finished: false,
            disconnect_timeout: self.disconnect_timeout,
        })
    }
}

impl<V> RemoteExecStream<V>
where
    V: HostKeyVerifier,
{
    /// ACKs only after russh accepts the raw bytes for the exact channel writer.
    pub async fn send_stdin(&mut self, bytes: &[u8]) -> Result<()> {
        if self.finished || self.stdin_closed {
            return Err(TransportError::ConnectionLost);
        }
        let next = self
            .stdin_bytes
            .checked_add(bytes.len())
            .ok_or(TransportError::InvalidRemoteExecCommand)?;
        if bytes.is_empty() || next > self.limits.max_stdin_bytes {
            return Err(TransportError::InvalidRemoteExecCommand);
        }
        self.writer.data(bytes).await.map_err(map_protocol_error)?;
        self.stdin_bytes = next;
        Ok(())
    }

    pub async fn close_stdin(&mut self) -> Result<()> {
        if self.finished || self.stdin_closed {
            return Ok(());
        }
        self.writer.eof().await.map_err(map_protocol_error)?;
        self.stdin_closed = true;
        Ok(())
    }

    /// Reads the next bounded stdout/stderr, exit status, or EOF. Overflow disconnects the whole
    /// dedicated transport before returning an error; no later remote bytes are exposed.
    pub async fn next_event(&mut self) -> Result<Option<RemoteExecStreamEvent>> {
        if self.finished {
            return Ok(None);
        }
        while let Some(message) = self.reader.wait().await {
            match message {
                ChannelMsg::Data { data } => {
                    self.stdout_bytes = self
                        .stdout_bytes
                        .checked_add(data.len())
                        .ok_or(TransportError::RemoteExecOutputTooLarge)?;
                    if self.stdout_bytes > self.limits.max_stdout_bytes {
                        self.poison().await;
                        return Err(TransportError::RemoteExecOutputTooLarge);
                    }
                    return Ok(Some(RemoteExecStreamEvent::Stdout(data)));
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    self.stderr_bytes = self
                        .stderr_bytes
                        .checked_add(data.len())
                        .ok_or(TransportError::RemoteExecOutputTooLarge)?;
                    if self.stderr_bytes > self.limits.max_stderr_bytes {
                        self.poison().await;
                        return Err(TransportError::RemoteExecOutputTooLarge);
                    }
                    return Ok(Some(RemoteExecStreamEvent::Stderr(data)));
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    return Ok(Some(RemoteExecStreamEvent::ExitStatus(Some(exit_status))));
                }
                ChannelMsg::Eof | ChannelMsg::Close => {
                    self.finished = true;
                    return Ok(Some(RemoteExecStreamEvent::Eof));
                }
                ChannelMsg::Failure => {
                    self.poison().await;
                    return Err(TransportError::RemoteExecRejected);
                }
                _ => {}
            }
        }
        self.finished = true;
        Ok(Some(RemoteExecStreamEvent::Eof))
    }

    /// Explicitly tears down this independent transport. It cannot affect terminal, SFTP, or
    /// forwarding transports because none were moved into this stream.
    pub async fn disconnect(mut self) -> Result<()> {
        self.finished = true;
        let disconnect_timeout = self.disconnect_timeout;
        complete_disconnect_within(disconnect_timeout, async move {
            self.session
                .disconnect(Disconnect::ByApplication, "", "")
                .await
                .map_err(map_protocol_error)?;
            let _completion = (&mut self.session).await;
            Ok(())
        })
        .await
    }

    async fn poison(&mut self) {
        self.finished = true;
        if self
            .session
            .disconnect(Disconnect::ByApplication, "", "")
            .await
            .is_ok()
        {
            let _ = timeout(self.disconnect_timeout, &mut self.session).await;
        }
    }
}
