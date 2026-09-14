use std::{fmt, net::IpAddr, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use norishell_ssh_domain::Endpoint;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, lookup_host},
    time::timeout,
};
use zeroize::Zeroizing;

const MAX_HTTP_RESPONSE_HEADER_BYTES: usize = 16 * 1024;
const MAX_PROXY_CREDENTIAL_COMPONENT_BYTES: usize = u8::MAX as usize;

/// The local ingress used to reach the first SSH endpoint in a route.
pub enum RouteIngress {
    DirectTcp,
    HttpConnect {
        proxy: Endpoint,
        credentials: Option<ProxyCredentials>,
    },
    Socks5 {
        proxy: Endpoint,
        dns_mode: Socks5DnsMode,
        credentials: Option<ProxyCredentials>,
    },
}

impl fmt::Debug for RouteIngress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DirectTcp => formatter.write_str("DirectTcp"),
            Self::HttpConnect { proxy, credentials } => formatter
                .debug_struct("HttpConnect")
                .field("proxy", proxy)
                .field("credentials", credentials)
                .finish(),
            Self::Socks5 {
                proxy,
                dns_mode,
                credentials,
            } => formatter
                .debug_struct("Socks5")
                .field("proxy", proxy)
                .field("dns_mode", dns_mode)
                .field("credentials", credentials)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Socks5DnsMode {
    /// Resolve DNS on this machine and send one resolved IP address to the proxy.
    Local,
    /// Send the normalized DNS name to the proxy without resolving it locally.
    Proxy,
}

/// Bounded proxy credentials. Debug output never exposes either component.
pub struct ProxyCredentials {
    username: Zeroizing<String>,
    password: Zeroizing<Vec<u8>>,
}

impl ProxyCredentials {
    pub fn new(
        username: impl Into<String>,
        password: Vec<u8>,
    ) -> Result<Self, ProxyCredentialError> {
        let username = username.into();
        if username.is_empty()
            || username.len() > MAX_PROXY_CREDENTIAL_COMPONENT_BYTES
            || username.contains(':')
            || username.chars().any(char::is_control)
            || password.len() > MAX_PROXY_CREDENTIAL_COMPONENT_BYTES
        {
            return Err(ProxyCredentialError);
        }
        Ok(Self {
            username: Zeroizing::new(username),
            password: Zeroizing::new(password),
        })
    }
}

impl fmt::Debug for ProxyCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyCredentials")
            .field("username", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyCredentialError;

impl fmt::Display for ProxyCredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("proxy credentials are invalid or exceed protocol limits")
    }
}

impl std::error::Error for ProxyCredentialError {}

impl From<ProxyCredentialError> for RouteIngressError {
    fn from(_: ProxyCredentialError) -> Self {
        Self::new(
            IngressStage::Configuration,
            IngressFailureKind::InvalidConfiguration,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressStage {
    Configuration,
    DirectTcpConnect,
    ProxyTcpConnect,
    LocalDnsResolution,
    HttpRequest,
    HttpResponse,
    HttpStatus,
    SocksGreeting,
    SocksAuthentication,
    SocksConnectRequest,
    SocksConnectReply,
}

impl fmt::Display for IngressStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Configuration => "configuration",
            Self::DirectTcpConnect => "direct TCP connect",
            Self::ProxyTcpConnect => "proxy TCP connect",
            Self::LocalDnsResolution => "local DNS resolution",
            Self::HttpRequest => "HTTP CONNECT request",
            Self::HttpResponse => "HTTP CONNECT response",
            Self::HttpStatus => "HTTP CONNECT status",
            Self::SocksGreeting => "SOCKS5 greeting",
            Self::SocksAuthentication => "SOCKS5 authentication",
            Self::SocksConnectRequest => "SOCKS5 connect request",
            Self::SocksConnectReply => "SOCKS5 connect reply",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressFailureKind {
    InvalidConfiguration,
    CredentialLocked,
    CredentialUnavailable,
    Timeout,
    Io,
    Protocol,
    ResponseTooLarge,
    Rejected,
}

impl fmt::Display for IngressFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "invalid configuration",
            Self::CredentialLocked => "credential store is locked",
            Self::CredentialUnavailable => "credential is unavailable",
            Self::Timeout => "timed out",
            Self::Io => "I/O failed",
            Self::Protocol => "invalid protocol response",
            Self::ResponseTooLarge => "response exceeded its size limit",
            Self::Rejected => "proxy rejected the request",
        })
    }
}

/// A credential-free route error suitable for logs and user-facing mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteIngressError {
    pub stage: IngressStage,
    pub kind: IngressFailureKind,
}

impl RouteIngressError {
    const fn new(stage: IngressStage, kind: IngressFailureKind) -> Self {
        Self { stage, kind }
    }
}

impl fmt::Display for RouteIngressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "route ingress {} {}", self.stage, self.kind)
    }
}

impl std::error::Error for RouteIngressError {}

/// Opens the route ingress and returns the tunneled byte stream expected by
/// `russh::client::connect_stream`.
pub async fn connect_route_stream(
    target: &Endpoint,
    ingress: &RouteIngress,
    stage_timeout: Duration,
) -> Result<TcpStream, RouteIngressError> {
    if stage_timeout.is_zero() {
        return Err(RouteIngressError::new(
            IngressStage::Configuration,
            IngressFailureKind::InvalidConfiguration,
        ));
    }
    let stream = match ingress {
        RouteIngress::DirectTcp => {
            connect_tcp(target, stage_timeout, IngressStage::DirectTcpConnect).await?
        }
        RouteIngress::HttpConnect { proxy, credentials } => {
            let mut stream =
                connect_tcp(proxy, stage_timeout, IngressStage::ProxyTcpConnect).await?;
            establish_http_connect(&mut stream, target, credentials.as_ref(), stage_timeout)
                .await?;
            stream
        }
        RouteIngress::Socks5 {
            proxy,
            dns_mode,
            credentials,
        } => {
            let mut stream =
                connect_tcp(proxy, stage_timeout, IngressStage::ProxyTcpConnect).await?;
            establish_socks5(
                &mut stream,
                target,
                *dns_mode,
                credentials.as_ref(),
                stage_timeout,
            )
            .await?;
            stream
        }
    };
    stream.set_nodelay(true).map_err(|_| {
        RouteIngressError::new(
            match ingress {
                RouteIngress::DirectTcp => IngressStage::DirectTcpConnect,
                RouteIngress::HttpConnect { .. } | RouteIngress::Socks5 { .. } => {
                    IngressStage::ProxyTcpConnect
                }
            },
            IngressFailureKind::Io,
        )
    })?;
    Ok(stream)
}

async fn connect_tcp(
    endpoint: &Endpoint,
    stage_timeout: Duration,
    stage: IngressStage,
) -> Result<TcpStream, RouteIngressError> {
    timeout(
        stage_timeout,
        TcpStream::connect((endpoint.normalized_address(), endpoint.port())),
    )
    .await
    .map_err(|_| RouteIngressError::new(stage, IngressFailureKind::Timeout))?
    .map_err(|_| RouteIngressError::new(stage, IngressFailureKind::Io))
}

async fn establish_http_connect(
    stream: &mut TcpStream,
    target: &Endpoint,
    credentials: Option<&ProxyCredentials>,
    stage_timeout: Duration,
) -> Result<(), RouteIngressError> {
    let authority = target.trust_key();
    let mut request = Zeroizing::new(format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n"
    ));
    if let Some(credentials) = credentials {
        let mut plain = Zeroizing::new(Vec::with_capacity(
            credentials.username.len() + credentials.password.len() + 1,
        ));
        plain.extend_from_slice(credentials.username.as_bytes());
        plain.push(b':');
        plain.extend_from_slice(credentials.password.as_slice());
        request.push_str("Proxy-Authorization: Basic ");
        let encoded = Zeroizing::new(STANDARD.encode(plain.as_slice()));
        request.push_str(encoded.as_str());
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    write_all(
        stream,
        request.as_bytes(),
        stage_timeout,
        IngressStage::HttpRequest,
    )
    .await?;

    let header = read_http_header(stream, stage_timeout).await?;
    let status_line = header.split(|byte| *byte == b'\n').next().ok_or_else(|| {
        RouteIngressError::new(IngressStage::HttpResponse, IngressFailureKind::Protocol)
    })?;
    let status_line = std::str::from_utf8(status_line)
        .map_err(|_| {
            RouteIngressError::new(IngressStage::HttpResponse, IngressFailureKind::Protocol)
        })?
        .trim_end_matches('\r');
    let mut fields = status_line.split_ascii_whitespace();
    let version = fields.next().unwrap_or_default();
    let status = fields.next().unwrap_or_default();
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        || status.len() != 3
        || !status.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(RouteIngressError::new(
            IngressStage::HttpResponse,
            IngressFailureKind::Protocol,
        ));
    }
    let status = status.parse::<u16>().map_err(|_| {
        RouteIngressError::new(IngressStage::HttpResponse, IngressFailureKind::Protocol)
    })?;
    if !(200..300).contains(&status) {
        return Err(RouteIngressError::new(
            IngressStage::HttpStatus,
            IngressFailureKind::Rejected,
        ));
    }
    Ok(())
}

async fn read_http_header(
    stream: &mut TcpStream,
    stage_timeout: Duration,
) -> Result<Vec<u8>, RouteIngressError> {
    let read = async {
        let mut header = Vec::with_capacity(512);
        while header.len() < MAX_HTTP_RESPONSE_HEADER_BYTES {
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).await.map_err(|_| {
                RouteIngressError::new(IngressStage::HttpResponse, IngressFailureKind::Io)
            })?;
            header.push(byte[0]);
            if header.ends_with(b"\r\n\r\n") {
                return Ok(header);
            }
        }
        Err(RouteIngressError::new(
            IngressStage::HttpResponse,
            IngressFailureKind::ResponseTooLarge,
        ))
    };
    timeout(stage_timeout, read).await.map_err(|_| {
        RouteIngressError::new(IngressStage::HttpResponse, IngressFailureKind::Timeout)
    })?
}

async fn establish_socks5(
    stream: &mut TcpStream,
    target: &Endpoint,
    dns_mode: Socks5DnsMode,
    credentials: Option<&ProxyCredentials>,
    stage_timeout: Duration,
) -> Result<(), RouteIngressError> {
    let method = if credentials.is_some() { 0x02 } else { 0x00 };
    write_all(
        stream,
        &[0x05, 0x01, method],
        stage_timeout,
        IngressStage::SocksGreeting,
    )
    .await?;
    let mut greeting = [0_u8; 2];
    read_exact(
        stream,
        &mut greeting,
        stage_timeout,
        IngressStage::SocksGreeting,
    )
    .await?;
    if greeting[0] != 0x05 {
        return Err(RouteIngressError::new(
            IngressStage::SocksGreeting,
            IngressFailureKind::Protocol,
        ));
    }
    if greeting[1] == 0xff || greeting[1] != method {
        return Err(RouteIngressError::new(
            IngressStage::SocksGreeting,
            IngressFailureKind::Rejected,
        ));
    }

    if let Some(credentials) = credentials {
        if credentials.password.is_empty() {
            return Err(RouteIngressError::new(
                IngressStage::Configuration,
                IngressFailureKind::InvalidConfiguration,
            ));
        }
        let username_length = u8::try_from(credentials.username.len()).map_err(|_| {
            RouteIngressError::new(
                IngressStage::Configuration,
                IngressFailureKind::InvalidConfiguration,
            )
        })?;
        let password_length = u8::try_from(credentials.password.len()).map_err(|_| {
            RouteIngressError::new(
                IngressStage::Configuration,
                IngressFailureKind::InvalidConfiguration,
            )
        })?;
        let mut auth_request = Zeroizing::new(Vec::with_capacity(
            credentials.username.len() + credentials.password.len() + 3,
        ));
        auth_request.extend_from_slice(&[0x01, username_length]);
        auth_request.extend_from_slice(credentials.username.as_bytes());
        auth_request.push(password_length);
        auth_request.extend_from_slice(credentials.password.as_slice());
        write_all(
            stream,
            auth_request.as_slice(),
            stage_timeout,
            IngressStage::SocksAuthentication,
        )
        .await?;
        let mut auth_reply = [0_u8; 2];
        read_exact(
            stream,
            &mut auth_reply,
            stage_timeout,
            IngressStage::SocksAuthentication,
        )
        .await?;
        if auth_reply[0] != 0x01 {
            return Err(RouteIngressError::new(
                IngressStage::SocksAuthentication,
                IngressFailureKind::Protocol,
            ));
        }
        if auth_reply[1] != 0x00 {
            return Err(RouteIngressError::new(
                IngressStage::SocksAuthentication,
                IngressFailureKind::Rejected,
            ));
        }
    }

    let mut connect_request = vec![0x05, 0x01, 0x00];
    match socks_target_address(target, dns_mode, stage_timeout).await? {
        SocksTargetAddress::Ip(IpAddr::V4(address)) => {
            connect_request.push(0x01);
            connect_request.extend_from_slice(&address.octets());
        }
        SocksTargetAddress::Ip(IpAddr::V6(address)) => {
            connect_request.push(0x04);
            connect_request.extend_from_slice(&address.octets());
        }
        SocksTargetAddress::Domain(domain) => {
            let length = u8::try_from(domain.len()).map_err(|_| {
                RouteIngressError::new(
                    IngressStage::Configuration,
                    IngressFailureKind::InvalidConfiguration,
                )
            })?;
            connect_request.extend_from_slice(&[0x03, length]);
            connect_request.extend_from_slice(domain.as_bytes());
        }
    }
    connect_request.extend_from_slice(&target.port().to_be_bytes());
    write_all(
        stream,
        &connect_request,
        stage_timeout,
        IngressStage::SocksConnectRequest,
    )
    .await?;
    read_socks5_reply(stream, stage_timeout).await
}

enum SocksTargetAddress<'a> {
    Ip(IpAddr),
    Domain(&'a str),
}

async fn socks_target_address(
    target: &Endpoint,
    dns_mode: Socks5DnsMode,
    stage_timeout: Duration,
) -> Result<SocksTargetAddress<'_>, RouteIngressError> {
    if let Ok(address) = target.normalized_address().parse::<IpAddr>() {
        return Ok(SocksTargetAddress::Ip(address));
    }
    match dns_mode {
        Socks5DnsMode::Proxy => Ok(SocksTargetAddress::Domain(target.normalized_address())),
        Socks5DnsMode::Local => {
            let mut addresses = timeout(
                stage_timeout,
                lookup_host((target.normalized_address(), target.port())),
            )
            .await
            .map_err(|_| {
                RouteIngressError::new(
                    IngressStage::LocalDnsResolution,
                    IngressFailureKind::Timeout,
                )
            })?
            .map_err(|_| {
                RouteIngressError::new(IngressStage::LocalDnsResolution, IngressFailureKind::Io)
            })?;
            addresses
                .next()
                .map(|address| SocksTargetAddress::Ip(address.ip()))
                .ok_or_else(|| {
                    RouteIngressError::new(IngressStage::LocalDnsResolution, IngressFailureKind::Io)
                })
        }
    }
}

async fn read_socks5_reply(
    stream: &mut TcpStream,
    stage_timeout: Duration,
) -> Result<(), RouteIngressError> {
    let stage = IngressStage::SocksConnectReply;
    let mut fixed = [0_u8; 4];
    read_exact(stream, &mut fixed, stage_timeout, stage).await?;
    if fixed[0] != 0x05 || fixed[2] != 0x00 {
        return Err(RouteIngressError::new(stage, IngressFailureKind::Protocol));
    }
    if fixed[1] != 0x00 {
        return Err(RouteIngressError::new(stage, IngressFailureKind::Rejected));
    }
    let address_length = match fixed[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut length = [0_u8; 1];
            read_exact(stream, &mut length, stage_timeout, stage).await?;
            if length[0] == 0 {
                return Err(RouteIngressError::new(stage, IngressFailureKind::Protocol));
            }
            usize::from(length[0])
        }
        _ => {
            return Err(RouteIngressError::new(stage, IngressFailureKind::Protocol));
        }
    };
    let mut address_and_port = vec![0_u8; address_length + 2];
    read_exact(stream, &mut address_and_port, stage_timeout, stage).await
}

async fn write_all(
    stream: &mut TcpStream,
    data: &[u8],
    stage_timeout: Duration,
    stage: IngressStage,
) -> Result<(), RouteIngressError> {
    timeout(stage_timeout, stream.write_all(data))
        .await
        .map_err(|_| RouteIngressError::new(stage, IngressFailureKind::Timeout))?
        .map_err(|_| RouteIngressError::new(stage, IngressFailureKind::Io))
}

async fn read_exact(
    stream: &mut TcpStream,
    data: &mut [u8],
    stage_timeout: Duration,
    stage: IngressStage,
) -> Result<(), RouteIngressError> {
    timeout(stage_timeout, stream.read_exact(data))
        .await
        .map_err(|_| RouteIngressError::new(stage, IngressFailureKind::Timeout))?
        .map(|_| ())
        .map_err(|_| RouteIngressError::new(stage, IngressFailureKind::Io))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use norishell_ssh_domain::Endpoint;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        task::JoinHandle,
        time::sleep,
    };

    use super::{
        IngressFailureKind, IngressStage, MAX_HTTP_RESPONSE_HEADER_BYTES, ProxyCredentials,
        RouteIngress, Socks5DnsMode, connect_route_stream,
    };

    const TEST_TIMEOUT: Duration = Duration::from_secs(2);

    async fn fake_proxy<F, Fut>(handler: F) -> (Endpoint, JoinHandle<()>)
    where
        F: FnOnce(TcpStream) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
        let address = listener.local_addr().expect("proxy address");
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept client");
            handler(stream).await;
        });
        (
            Endpoint::parse("127.0.0.1", address.port()).expect("proxy endpoint"),
            task,
        )
    }

    async fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(stream.read_u8().await.expect("request byte"));
        }
        request
    }

    async fn read_socks_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut fixed = [0_u8; 4];
        stream.read_exact(&mut fixed).await.expect("SOCKS request");
        let address_length = match fixed[3] {
            0x01 => 4,
            0x04 => 16,
            0x03 => usize::from(stream.read_u8().await.expect("domain length")),
            other => panic!("unexpected address type {other}"),
        };
        let mut rest = vec![0_u8; address_length + 2];
        stream
            .read_exact(&mut rest)
            .await
            .expect("address and port");
        let mut request = fixed.to_vec();
        if fixed[3] == 0x03 {
            request.push(u8::try_from(address_length).expect("bounded length"));
        }
        request.extend(rest);
        request
    }

    #[tokio::test]
    async fn http_connect_success_returns_unconsumed_tunneled_stream() {
        let (proxy, task) = fake_proxy(|mut stream| async move {
            let request = read_http_request(&mut stream).await;
            assert!(request.starts_with(b"CONNECT example.com:22 HTTP/1.1\r\n"));
            stream
                .write_all(
                    b"HTTP/1.1 200 Connection Established\r\nX-Test: yes\r\n\r\nSSH-2.0-fake\r\n",
                )
                .await
                .expect("proxy response");
        })
        .await;
        let target = Endpoint::parse("example.com", 22).expect("target");
        let mut stream = connect_route_stream(
            &target,
            &RouteIngress::HttpConnect {
                proxy,
                credentials: None,
            },
            TEST_TIMEOUT,
        )
        .await
        .expect("CONNECT tunnel");
        let mut banner = [0_u8; 14];
        stream.read_exact(&mut banner).await.expect("SSH banner");
        assert_eq!(&banner, b"SSH-2.0-fake\r\n");
        task.await.expect("proxy task");
    }

    #[tokio::test]
    async fn http_connect_basic_auth_is_sent_but_rejection_error_is_redacted() {
        let (proxy, task) = fake_proxy(|mut stream| async move {
            let request = read_http_request(&mut stream).await;
            let request = String::from_utf8(request).expect("ASCII request");
            assert!(request.contains("Proxy-Authorization: Basic dXNlcjpzZWNyZXQ=\r\n"));
            stream
                .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                .await
                .expect("rejection");
        })
        .await;
        let credentials = ProxyCredentials::new("user", b"secret".to_vec()).expect("credentials");
        assert!(!format!("{credentials:?}").contains("secret"));
        let error = connect_route_stream(
            &Endpoint::parse("example.com", 22).expect("target"),
            &RouteIngress::HttpConnect {
                proxy,
                credentials: Some(credentials),
            },
            TEST_TIMEOUT,
        )
        .await
        .expect_err("proxy must reject");
        assert_eq!(error.stage, IngressStage::HttpStatus);
        assert_eq!(error.kind, IngressFailureKind::Rejected);
        assert!(!error.to_string().contains("secret"));
        task.await.expect("proxy task");
    }

    #[tokio::test]
    async fn http_connect_response_is_bounded() {
        let (proxy, task) = fake_proxy(|mut stream| async move {
            let _request = read_http_request(&mut stream).await;
            stream
                .write_all(&vec![b'X'; MAX_HTTP_RESPONSE_HEADER_BYTES])
                .await
                .expect("oversized response");
        })
        .await;
        let error = connect_route_stream(
            &Endpoint::parse("example.com", 22).expect("target"),
            &RouteIngress::HttpConnect {
                proxy,
                credentials: None,
            },
            TEST_TIMEOUT,
        )
        .await
        .expect_err("response must be bounded");
        assert_eq!(error.stage, IngressStage::HttpResponse);
        assert_eq!(error.kind, IngressFailureKind::ResponseTooLarge);
        task.await.expect("proxy task");
    }

    #[tokio::test]
    async fn http_connect_response_timeout_has_response_stage() {
        let (proxy, task) = fake_proxy(|mut stream| async move {
            let _request = read_http_request(&mut stream).await;
            sleep(Duration::from_millis(150)).await;
        })
        .await;
        let error = connect_route_stream(
            &Endpoint::parse("example.com", 22).expect("target"),
            &RouteIngress::HttpConnect {
                proxy,
                credentials: None,
            },
            Duration::from_millis(50),
        )
        .await
        .expect_err("response must time out");
        assert_eq!(error.stage, IngressStage::HttpResponse);
        assert_eq!(error.kind, IngressFailureKind::Timeout);
        task.await.expect("proxy task");
    }

    #[tokio::test]
    async fn socks5_proxy_dns_sends_domain_without_local_resolution() {
        let (proxy, task) = fake_proxy(|mut stream| async move {
            let mut greeting = [0_u8; 3];
            stream.read_exact(&mut greeting).await.expect("greeting");
            assert_eq!(greeting, [0x05, 0x01, 0x00]);
            stream.write_all(&[0x05, 0x00]).await.expect("method");
            let request = read_socks_request(&mut stream).await;
            assert_eq!(request[3], 0x03);
            assert_eq!(request[4] as usize, "does-not-resolve.invalid".len());
            assert_eq!(
                &request[5..5 + "does-not-resolve.invalid".len()],
                b"does-not-resolve.invalid"
            );
            stream
                .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 22])
                .await
                .expect("connect reply");
        })
        .await;
        connect_route_stream(
            &Endpoint::parse("does-not-resolve.invalid", 22).expect("target"),
            &RouteIngress::Socks5 {
                proxy,
                dns_mode: Socks5DnsMode::Proxy,
                credentials: None,
            },
            TEST_TIMEOUT,
        )
        .await
        .expect("SOCKS proxy DNS tunnel");
        task.await.expect("proxy task");
    }

    #[tokio::test]
    async fn socks5_local_dns_sends_resolved_ip() {
        let (proxy, task) = fake_proxy(|mut stream| async move {
            let mut greeting = [0_u8; 3];
            stream.read_exact(&mut greeting).await.expect("greeting");
            stream.write_all(&[0x05, 0x00]).await.expect("method");
            let request = read_socks_request(&mut stream).await;
            assert!(matches!(request[3], 0x01 | 0x04));
            stream
                .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 22])
                .await
                .expect("connect reply");
        })
        .await;
        connect_route_stream(
            &Endpoint::parse("localhost", 22).expect("target"),
            &RouteIngress::Socks5 {
                proxy,
                dns_mode: Socks5DnsMode::Local,
                credentials: None,
            },
            TEST_TIMEOUT,
        )
        .await
        .expect("SOCKS local DNS tunnel");
        task.await.expect("proxy task");
    }

    #[tokio::test]
    async fn socks5_username_password_authentication_succeeds() {
        let (proxy, task) = fake_proxy(|mut stream| async move {
            let mut greeting = [0_u8; 3];
            stream.read_exact(&mut greeting).await.expect("greeting");
            assert_eq!(greeting, [0x05, 0x01, 0x02]);
            stream.write_all(&[0x05, 0x02]).await.expect("method");
            let mut auth = [0_u8; 13];
            stream.read_exact(&mut auth).await.expect("auth request");
            assert_eq!(&auth, b"\x01\x04user\x06secret");
            stream.write_all(&[0x01, 0x00]).await.expect("auth reply");
            let _request = read_socks_request(&mut stream).await;
            stream
                .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 22])
                .await
                .expect("connect reply");
        })
        .await;
        connect_route_stream(
            &Endpoint::parse("example.com", 22).expect("target"),
            &RouteIngress::Socks5 {
                proxy,
                dns_mode: Socks5DnsMode::Proxy,
                credentials: Some(
                    ProxyCredentials::new("user", b"secret".to_vec()).expect("credentials"),
                ),
            },
            TEST_TIMEOUT,
        )
        .await
        .expect("authenticated SOCKS tunnel");
        task.await.expect("proxy task");
    }

    #[tokio::test]
    async fn socks5_authentication_rejection_is_staged() {
        let (proxy, task) = fake_proxy(|mut stream| async move {
            let mut greeting = [0_u8; 3];
            stream.read_exact(&mut greeting).await.expect("greeting");
            stream.write_all(&[0x05, 0x02]).await.expect("method");
            let mut auth = [0_u8; 13];
            stream.read_exact(&mut auth).await.expect("auth request");
            stream.write_all(&[0x01, 0x01]).await.expect("auth reply");
        })
        .await;
        let error = connect_route_stream(
            &Endpoint::parse("example.com", 22).expect("target"),
            &RouteIngress::Socks5 {
                proxy,
                dns_mode: Socks5DnsMode::Proxy,
                credentials: Some(
                    ProxyCredentials::new("user", b"secret".to_vec()).expect("credentials"),
                ),
            },
            TEST_TIMEOUT,
        )
        .await
        .expect_err("auth must fail");
        assert_eq!(error.stage, IngressStage::SocksAuthentication);
        assert_eq!(error.kind, IngressFailureKind::Rejected);
        task.await.expect("proxy task");
    }

    #[tokio::test]
    async fn socks5_rejects_malformed_zero_length_reply_address() {
        let (proxy, task) = fake_proxy(|mut stream| async move {
            let mut greeting = [0_u8; 3];
            stream.read_exact(&mut greeting).await.expect("greeting");
            stream.write_all(&[0x05, 0x00]).await.expect("method");
            let _request = read_socks_request(&mut stream).await;
            stream
                .write_all(&[0x05, 0x00, 0x00, 0x03, 0x00])
                .await
                .expect("malformed reply");
        })
        .await;
        let error = connect_route_stream(
            &Endpoint::parse("example.com", 22).expect("target"),
            &RouteIngress::Socks5 {
                proxy,
                dns_mode: Socks5DnsMode::Proxy,
                credentials: None,
            },
            TEST_TIMEOUT,
        )
        .await
        .expect_err("malformed reply must fail");
        assert_eq!(error.stage, IngressStage::SocksConnectReply);
        assert_eq!(error.kind, IngressFailureKind::Protocol);
        task.await.expect("proxy task");
    }
}
