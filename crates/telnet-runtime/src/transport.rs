use std::{collections::VecDeque, io, time::Duration};

use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, lookup_host, tcp::OwnedReadHalf, tcp::OwnedWriteHalf},
    time::timeout,
};

use crate::{NegotiatedOption, ProtocolError, TelnetCodec, encode_naws, encode_user_input};

pub const DEFAULT_TELNET_PORT: u16 = 23;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ADDRESS_BYTES: usize = 253;
const MAX_INPUT_BYTES: usize = 1024 * 1024;
const READ_BUFFER_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetConnectConfig {
    pub address: String,
    pub port: u16,
    pub rows: u16,
    pub cols: u16,
    /// Must be supplied by the user for this exact connection attempt. The
    /// runtime never persists or reuses this acknowledgement.
    pub cleartext_risk_accepted: bool,
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
}

impl TelnetConnectConfig {
    #[must_use]
    pub fn direct(address: impl Into<String>, rows: u16, cols: u16) -> Self {
        Self {
            address: address.into(),
            port: DEFAULT_TELNET_PORT,
            rows,
            cols,
            cleartext_risk_accepted: false,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            io_timeout: DEFAULT_IO_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelnetEvent {
    Data(Vec<u8>),
    Closed,
}

#[derive(Debug, Error)]
pub enum TelnetRuntimeError {
    #[error("cleartext Telnet risk was not accepted for this connection attempt")]
    CleartextRiskNotAccepted,
    #[error("invalid Telnet endpoint")]
    InvalidEndpoint,
    #[error("invalid Telnet terminal dimensions")]
    InvalidTerminalSize,
    #[error("Telnet input exceeds the bounded write limit")]
    InputTooLarge,
    #[error("Telnet endpoint resolution failed")]
    ResolveFailed,
    #[error("Telnet direct TCP connect timed out")]
    ConnectTimeout,
    #[error("Telnet direct TCP connect failed")]
    ConnectFailed(#[source] io::Error),
    #[error("Telnet I/O timed out")]
    IoTimeout,
    #[error("Telnet I/O failed")]
    Io(#[source] io::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("Telnet connection is closed")]
    ConnectionClosed,
}

pub struct TelnetConnection {
    read: OwnedReadHalf,
    write: OwnedWriteHalf,
    codec: TelnetCodec,
    rows: u16,
    cols: u16,
    io_timeout: Duration,
    pending_protocol_write: Vec<u8>,
    pending_protocol_offset: usize,
    pending_data: VecDeque<Vec<u8>>,
    closed: bool,
}

impl TelnetConnection {
    pub async fn connect(config: TelnetConnectConfig) -> Result<Self, TelnetRuntimeError> {
        validate_config(&config)?;
        let resolved = timeout(
            config.connect_timeout,
            lookup_host((config.address.as_str(), config.port)),
        )
        .await
        .map_err(|_| TelnetRuntimeError::ConnectTimeout)?
        .map_err(|_| TelnetRuntimeError::ResolveFailed)?;
        let addresses: Vec<_> = resolved.collect();
        if addresses.is_empty() {
            return Err(TelnetRuntimeError::ResolveFailed);
        }
        let stream = timeout(
            config.connect_timeout,
            TcpStream::connect(addresses.as_slice()),
        )
        .await
        .map_err(|_| TelnetRuntimeError::ConnectTimeout)?
        .map_err(TelnetRuntimeError::ConnectFailed)?;
        stream
            .set_nodelay(true)
            .map_err(TelnetRuntimeError::ConnectFailed)?;
        let (read, write) = stream.into_split();
        Ok(Self {
            read,
            write,
            codec: TelnetCodec::new(),
            rows: config.rows,
            cols: config.cols,
            io_timeout: config.io_timeout,
            pending_protocol_write: Vec::new(),
            pending_protocol_offset: 0,
            pending_data: VecDeque::new(),
            closed: false,
        })
    }

    pub async fn send_input(&mut self, data: &[u8]) -> Result<(), TelnetRuntimeError> {
        self.ensure_open()?;
        if data.len() > MAX_INPUT_BYTES {
            return Err(TelnetRuntimeError::InputTooLarge);
        }
        self.flush_protocol_write().await?;
        let encoded = encode_user_input(data);
        write_all_within(&mut self.write, &encoded, self.io_timeout).await
    }

    pub async fn resize(&mut self, rows: u16, cols: u16) -> Result<(), TelnetRuntimeError> {
        self.ensure_open()?;
        if rows == 0 || cols == 0 {
            return Err(TelnetRuntimeError::InvalidTerminalSize);
        }
        self.rows = rows;
        self.cols = cols;
        self.flush_protocol_write().await?;
        if self.codec.local_naws_enabled() {
            write_all_within(&mut self.write, &encode_naws(rows, cols), self.io_timeout).await?;
        }
        Ok(())
    }

    pub async fn next_event(&mut self) -> Result<TelnetEvent, TelnetRuntimeError> {
        self.ensure_open()?;
        self.flush_protocol_write().await?;
        if let Some(data) = self.pending_data.pop_front() {
            return Ok(TelnetEvent::Data(data));
        }
        let mut buffer = [0_u8; READ_BUFFER_BYTES];
        loop {
            // An idle interactive terminal is healthy. Reads therefore remain
            // pending until bytes, EOF, or owner cancellation; only bounded
            // writes and shutdown use the I/O deadline.
            let count = self
                .read
                .read(&mut buffer)
                .await
                .map_err(TelnetRuntimeError::Io)?;
            if count == 0 {
                self.closed = true;
                return Ok(TelnetEvent::Closed);
            }
            let decoded = self.codec.decode(&buffer[..count])?;
            let enabled_naws = decoded.enabled.contains(&NegotiatedOption::LocalNaws);
            if !decoded.responses.is_empty() {
                self.pending_protocol_write
                    .extend_from_slice(&decoded.responses);
            }
            if enabled_naws {
                self.pending_protocol_write
                    .extend_from_slice(&encode_naws(self.rows, self.cols));
            }
            if !decoded.data.is_empty() {
                self.pending_data.push_back(decoded.data);
            }
            // `next_event` is selected against actor commands. Persisting both
            // protocol replies and terminal bytes before this await makes a
            // cancelled read future safe to resume without losing or
            // reordering data.
            self.flush_protocol_write().await?;
            if let Some(data) = self.pending_data.pop_front() {
                return Ok(TelnetEvent::Data(data));
            }
        }
    }

    pub async fn disconnect(mut self) -> Result<(), TelnetRuntimeError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        timeout(self.io_timeout, self.write.shutdown())
            .await
            .map_err(|_| TelnetRuntimeError::IoTimeout)?
            .map_err(TelnetRuntimeError::Io)
    }

    fn ensure_open(&self) -> Result<(), TelnetRuntimeError> {
        if self.closed {
            Err(TelnetRuntimeError::ConnectionClosed)
        } else {
            Ok(())
        }
    }

    async fn flush_protocol_write(&mut self) -> Result<(), TelnetRuntimeError> {
        while self.pending_protocol_offset < self.pending_protocol_write.len() {
            let count = timeout(
                self.io_timeout,
                self.write
                    .write(&self.pending_protocol_write[self.pending_protocol_offset..]),
            )
            .await
            .map_err(|_| TelnetRuntimeError::IoTimeout)?
            .map_err(TelnetRuntimeError::Io)?;
            if count == 0 {
                return Err(TelnetRuntimeError::Io(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Telnet protocol response write returned zero bytes",
                )));
            }
            self.pending_protocol_offset = self.pending_protocol_offset.saturating_add(count);
        }
        self.pending_protocol_write.clear();
        self.pending_protocol_offset = 0;
        Ok(())
    }
}

fn validate_config(config: &TelnetConnectConfig) -> Result<(), TelnetRuntimeError> {
    let address = config.address.trim();
    if !config.cleartext_risk_accepted {
        return Err(TelnetRuntimeError::CleartextRiskNotAccepted);
    }
    if address.is_empty()
        || address.len() > MAX_ADDRESS_BYTES
        || address.chars().any(char::is_control)
        || config.port == 0
    {
        return Err(TelnetRuntimeError::InvalidEndpoint);
    }
    if config.rows == 0 || config.cols == 0 {
        return Err(TelnetRuntimeError::InvalidTerminalSize);
    }
    Ok(())
}

async fn write_all_within(
    write: &mut OwnedWriteHalf,
    bytes: &[u8],
    io_timeout: Duration,
) -> Result<(), TelnetRuntimeError> {
    timeout(io_timeout, write.write_all(bytes))
        .await
        .map_err(|_| TelnetRuntimeError::IoTimeout)?
        .map_err(TelnetRuntimeError::Io)
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    const TEST_IAC: u8 = 255;
    const TEST_DO: u8 = 253;
    const TEST_WILL: u8 = 251;
    const TEST_SB: u8 = 250;
    const TEST_SE: u8 = 240;
    const TEST_TTYPE: u8 = 24;
    const TEST_NAWS: u8 = 31;
    const TEST_TTYPE_SEND: u8 = 1;
    const TEST_TERMINAL_TYPE: &[u8] = b"xterm-256color";

    #[tokio::test]
    async fn rejects_missing_risk_acknowledgement_before_connect() {
        let config = TelnetConnectConfig::direct("127.0.0.1", 24, 80);
        assert!(matches!(
            TelnetConnection::connect(config).await,
            Err(TelnetRuntimeError::CleartextRiskNotAccepted)
        ));
    }

    #[tokio::test]
    async fn negotiates_naws_and_preserves_raw_terminal_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let endpoint = listener.local_addr().expect("local address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            socket
                .write_all(&[
                    TEST_IAC,
                    TEST_DO,
                    TEST_NAWS,
                    TEST_IAC,
                    TEST_DO,
                    TEST_TTYPE,
                    TEST_IAC,
                    TEST_SB,
                    TEST_TTYPE,
                    TEST_TTYPE_SEND,
                    TEST_IAC,
                    TEST_SE,
                    b'h',
                    b'i',
                    TEST_IAC,
                    TEST_IAC,
                    0xfe,
                ])
                .await
                .expect("server write");
            let mut response = vec![0_u8; 256];
            let count = timeout(Duration::from_secs(2), socket.read(&mut response))
                .await
                .expect("response timeout")
                .expect("response read");
            response.truncate(count);
            response
        });

        let mut config = TelnetConnectConfig::direct("127.0.0.1", 24, 255);
        config.port = endpoint.port();
        config.cleartext_risk_accepted = true;
        let mut connection = TelnetConnection::connect(config).await.expect("connect");
        assert_eq!(
            connection.next_event().await.expect("terminal data"),
            TelnetEvent::Data(vec![b'h', b'i', TEST_IAC, 0xfe])
        );

        let response = server.await.expect("server task");
        assert!(
            response
                .windows(3)
                .any(|part| part == [TEST_IAC, TEST_WILL, TEST_NAWS])
        );
        assert!(
            response
                .windows(3)
                .any(|part| part == [TEST_IAC, TEST_WILL, TEST_TTYPE])
        );
        assert!(
            response
                .windows(3)
                .any(|part| part == [TEST_IAC, TEST_SB, TEST_NAWS])
        );
        assert!(
            response
                .windows(TEST_TERMINAL_TYPE.len())
                .any(|part| part == TEST_TERMINAL_TYPE)
        );
    }
}
