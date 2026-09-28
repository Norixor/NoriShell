//! Bound server-declared PDU lengths before allocation; preserve received bytes when a read is cancelled.
use ironrdp::{
    connector::{ConnectorError, ConnectorErrorKind, Sequence},
    core::WriteBuf,
    pdu::{Action, PduHint},
};
use norishell_desktop_protocol::{BoxedDesktopIo, EngineControl, EngineError, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_PDU: usize = 4 * 1024 * 1024;
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Classifies an input write interrupted by a focus change: nothing sent is harmless, a partial message is fatal.
pub(crate) fn stale_or_torn(written: usize) -> EngineError {
    if written == 0 {
        EngineError::StaleInput
    } else {
        EngineError::ConnectionLost
    }
}
pub(crate) struct Wire {
    pub stream: BoxedDesktopIo,
    pub buffer: Vec<u8>,
}
impl Wire {
    pub fn new(stream: BoxedDesktopIo) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
        }
    }
    async fn more(&mut self) -> Result<()> {
        if self.buffer.len() >= MAX_PDU {
            return Err(EngineError::ResourceLimit);
        }
        let mut chunk = [0; 8192];
        let n = self
            .stream
            .read(&mut chunk)
            .await
            .map_err(|_| EngineError::ConnectionLost)?;
        if n == 0 {
            return Err(EngineError::ConnectionLost);
        }
        self.buffer.extend_from_slice(&chunk[..n]);
        Ok(())
    }
    async fn exact(&mut self, size: usize) -> Result<Vec<u8>> {
        if size == 0 || size > MAX_PDU {
            return Err(EngineError::ResourceLimit);
        }
        while self.buffer.len() < size {
            self.more().await?;
        }
        Ok(self.buffer.drain(..size).collect())
    }
    pub async fn hint(&mut self, hint: &dyn PduHint) -> Result<Vec<u8>> {
        loop {
            if let Some((matched, size)) = hint
                .find_size(&self.buffer)
                .map_err(|_| EngineError::Protocol)?
            {
                let bytes = self.exact(size).await?;
                if matched {
                    return Ok(bytes);
                }
            } else {
                self.more().await?;
            }
        }
    }
    pub async fn pdu(&mut self) -> Result<(Action, Vec<u8>)> {
        loop {
            if let Some(info) =
                ironrdp::pdu::find_size(&self.buffer).map_err(|_| EngineError::Protocol)?
            {
                return Ok((info.action, self.exact(info.length).await?));
            }
            self.more().await?;
        }
    }
    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        tokio::time::timeout(WRITE_TIMEOUT, async {
            self.stream.write_all(data).await?;
            self.stream.flush().await
        })
        .await
        .map_err(|_| EngineError::Timeout)?
        .map_err(|_| EngineError::ConnectionLost)
    }
    /// Writes one complete protocol message unless the focus epoch changes first.
    ///
    /// A focus change observed before the stream accepted any byte leaves the stream intact and returns
    /// `StaleInput`. Once some but not all bytes were accepted the message is torn, so the stream can no
    /// longer be trusted and the result is `ConnectionLost`. After every byte is accepted the message is
    /// committed; the trailing flush is not fenced.
    pub async fn write_fenced(
        &mut self,
        data: &[u8],
        control: &EngineControl,
        epoch: u64,
    ) -> Result<()> {
        let mut focus = control.focus_epoch.clone();
        if *control.stop.borrow() || *focus.borrow_and_update() != epoch {
            return Err(EngineError::StaleInput);
        }
        let stream = &mut self.stream;
        tokio::time::timeout(WRITE_TIMEOUT, async {
            let mut written = 0;
            while written < data.len() {
                tokio::select! { biased;
                    changed = focus.changed() => {
                        // A notification that keeps the same epoch does not invalidate the message.
                        if changed.is_ok() && *focus.borrow_and_update() == epoch {
                            continue;
                        }
                        return Err(stale_or_torn(written));
                    }
                    // `poll_write` returning Pending accepts no bytes, so dropping it here is safe.
                    result = stream.write(&data[written..]) => match result {
                        Ok(0) | Err(_) => return Err(EngineError::ConnectionLost),
                        Ok(n) => written += n,
                    },
                }
            }
            stream
                .flush()
                .await
                .map_err(|_| EngineError::ConnectionLost)
        })
        .await
        .map_err(|_| EngineError::Timeout)?
    }
    pub async fn step(&mut self, sequence: &mut dyn Sequence) -> Result<()> {
        let mut output = WriteBuf::new();
        let written = if let Some(hint) = sequence.next_pdu_hint() {
            let bytes = self.hint(hint).await?;
            // This boxed transport has no driver-owned monotonic clock epoch.
            sequence.step(&bytes, None, &mut output)
        } else {
            sequence.step_no_input(&mut output)
        }
        .map_err(connector_error)?;
        if written.size().is_some() {
            self.write(output.filled()).await?;
        }
        Ok(())
    }
}
pub(crate) fn connector_error(error: ConnectorError) -> EngineError {
    match error.kind() {
        ConnectorErrorKind::AccessDenied | ConnectorErrorKind::Credssp(_) => {
            EngineError::AuthenticationRejected
        }
        _ => EngineError::Protocol,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::watch;
    #[tokio::test]
    async fn stale_epoch_cannot_write() {
        let (io, mut peer) = tokio::io::duplex(64);
        let (_stop, stop) = watch::channel(false);
        let (_focus, focus_epoch) = watch::channel(2);
        let control = EngineControl { stop, focus_epoch };
        let mut wire = Wire::new(Box::new(io));
        assert_eq!(
            wire.write_fenced(b"private input", &control, 1).await,
            Err(EngineError::StaleInput)
        );
        let mut bytes = [0; 64];
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), peer.read(&mut bytes))
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn excessive_declared_length_fails_before_read_or_allocation() {
        let (io, _peer) = tokio::io::duplex(1);
        let mut wire = Wire::new(Box::new(io));
        assert_eq!(
            wire.exact(usize::MAX).await,
            Err(EngineError::ResourceLimit)
        );
        assert!(wire.buffer.is_empty());
    }
    #[tokio::test]
    async fn focus_change_during_backpressure_aborts_uncertain_writer() {
        let (io, _peer) = tokio::io::duplex(1);
        let (_stop, stop) = watch::channel(false);
        let (focus, focus_epoch) = watch::channel(1);
        let control = EngineControl { stop, focus_epoch };
        let mut wire = Wire::new(Box::new(io));
        let writer = wire.write_fenced(b"input longer than capacity", &control, 1);
        let update = async {
            tokio::task::yield_now().await;
            focus.send_replace(2);
        };
        let (result, ()) = tokio::join!(writer, update);
        assert_eq!(result, Err(EngineError::ConnectionLost));
    }
    #[tokio::test]
    async fn focus_change_before_any_byte_is_stale_not_fatal() {
        let (io, mut peer) = tokio::io::duplex(4);
        let (_stop, stop) = watch::channel(false);
        let (focus, focus_epoch) = watch::channel(1);
        let control = EngineControl { stop, focus_epoch };
        let mut wire = Wire::new(Box::new(io));
        // Fill the pipe so the fenced write cannot accept a single byte.
        wire.write(b"full").await.unwrap();
        let writer = wire.write_fenced(b"input", &control, 1);
        let update = async {
            tokio::task::yield_now().await;
            focus.send_replace(2);
        };
        let (result, ()) = tokio::join!(writer, update);
        assert_eq!(result, Err(EngineError::StaleInput));
        let mut bytes = [0; 16];
        assert_eq!(peer.read(&mut bytes).await.unwrap(), 4);
        assert_eq!(&bytes[..4], b"full");
    }
    #[tokio::test]
    async fn same_epoch_notification_does_not_abort_write() {
        let (io, mut peer) = tokio::io::duplex(4);
        let (_stop, stop) = watch::channel(false);
        let (focus, focus_epoch) = watch::channel(1);
        let control = EngineControl { stop, focus_epoch };
        let mut wire = Wire::new(Box::new(io));
        wire.write(b"full").await.unwrap();
        let writer = wire.write_fenced(b"ok", &control, 1);
        let drain = async {
            tokio::task::yield_now().await;
            focus.send_replace(1);
            let mut bytes = [0; 8];
            let mut total = 0;
            while total < 6 {
                total += peer.read(&mut bytes).await.unwrap();
            }
            total
        };
        let (result, total) = tokio::join!(writer, drain);
        assert_eq!(result, Ok(()));
        assert_eq!(total, 6);
    }
    #[test]
    fn interrupted_write_classification() {
        assert_eq!(stale_or_torn(0), EngineError::StaleInput);
        assert_eq!(stale_or_torn(1), EngineError::ConnectionLost);
    }
}
