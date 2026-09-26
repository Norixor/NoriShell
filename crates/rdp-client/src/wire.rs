//! Bound server-declared PDU lengths before allocation; preserve received bytes when a read is cancelled.
use ironrdp::{
    connector::{ConnectorError, ConnectorErrorKind, Sequence},
    core::WriteBuf,
    pdu::{Action, PduHint},
};
use norishell_desktop_protocol::{BoxedDesktopIo, EngineControl, EngineError, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_PDU: usize = 4 * 1024 * 1024;
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
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            self.stream.write_all(data).await?;
            self.stream.flush().await
        })
        .await
        .map_err(|_| EngineError::Timeout)?
        .map_err(|_| EngineError::ConnectionLost)
    }
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
        tokio::select! { biased;
            _=focus.changed()=>Err(EngineError::ConnectionLost),
            result=self.write(data)=>result,
        }
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
}
