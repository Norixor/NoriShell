//! Owner-scoped outbound network resources.
//!
//! The protected broker resolves and approves an endpoint exactly once. This module never accepts
//! an approval, plugin identity, host scope, or a second URL from a guest.

use std::{
    collections::BTreeSet,
    future::Future,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::{SinkExt as _, StreamExt as _};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiResourceEventKind, PluginHttpMethod, PluginNetworkEndpointRequest,
    PluginNetworkErrorCode, PluginNetworkEvent, PluginNetworkHeader, PluginNetworkOperation,
    PluginNetworkProtocol, PluginNetworkScheme, PluginNetworkSendRequest,
    PluginNetworkStartRequest, PreparedNetworkEndpoint,
};
use reqwest::{Client, Method, redirect};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    net::{TcpStream, UdpSocket},
    sync::{mpsc, watch},
    time::timeout,
};
use tokio_rustls::{TlsConnector, client::TlsStream, rustls};
use tokio_tungstenite::{
    WebSocketStream, client_async,
    tungstenite::client::IntoClientRequest,
    tungstenite::{Bytes, ClientRequestBuilder, Message, http::Uri},
};

use crate::plugin_credential_service::CredentialLease;

use super::{
    ResourceCommand, ResourceCommandReceiver, ResourceEventWriter, ResourceFence, ResourceOwner,
    ResourceRegistry,
};

const MAX_NETWORK_ADDRESSES: usize = 16;
const MAX_NETWORK_HEADERS: usize = 32;
const MAX_NETWORK_HEADER_BYTES: usize = 4 * 1024;
const MAX_NETWORK_BODY_BYTES: usize = 64 * 1024;
// The generic resource event has a 16 KiB serialized cap. Eight KiB raw bytes fit after base64
// encoding and event framing, while remaining a bounded chunk below the advertised 16 KiB limit.
const MAX_NETWORK_DATA_BYTES: usize = 8 * 1024;
const MIN_TIMEOUT_MILLISECONDS: u32 = 100;
const MAX_TIMEOUT_MILLISECONDS: u32 = 120_000;

/// Resolves one URL once. The returned value, including all addresses, is what Core binds into its
/// secure approval; `NetworkDriver::start` must receive this exact frozen value later.
pub(crate) async fn prepare_endpoint(
    request: &PluginNetworkEndpointRequest,
) -> Result<PreparedNetworkEndpoint, PluginApiErrorCode> {
    let url =
        reqwest::Url::parse(&request.endpoint).map_err(|_| PluginApiErrorCode::InvalidRequest)?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    let scheme = match url.scheme() {
        "http" => PluginNetworkScheme::Http,
        "https" => PluginNetworkScheme::Https,
        "ws" => PluginNetworkScheme::Ws,
        "wss" => PluginNetworkScheme::Wss,
        "tcp" => PluginNetworkScheme::Tcp,
        "udp" => PluginNetworkScheme::Udp,
        "tls" => PluginNetworkScheme::Tls,
        _ => return Err(PluginApiErrorCode::InvalidRequest),
    };
    let host = url
        .host_str()
        .map(|host| host.trim_matches(['[', ']']).to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .ok_or(PluginApiErrorCode::InvalidRequest)?;
    let port = url
        .port_or_known_default()
        .ok_or(PluginApiErrorCode::InvalidRequest)?;
    if port == 0 {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    let path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    let path_and_query = match url.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    };
    if !path_and_query.starts_with('/')
        || path_and_query
            .bytes()
            .any(|byte| byte == b'\\' || byte.is_ascii_control())
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    if matches!(
        scheme,
        PluginNetworkScheme::Tcp | PluginNetworkScheme::Udp | PluginNetworkScheme::Tls
    ) && path_and_query != "/"
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    let addresses = timeout(
        Duration::from_secs(3),
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await
    .map_err(|_| PluginApiErrorCode::TimedOut)?
    .map_err(|_| PluginApiErrorCode::Unavailable)?
    .map(|address| address.ip())
    .collect::<BTreeSet<_>>();
    if addresses.is_empty() || addresses.len() > MAX_NETWORK_ADDRESSES {
        return Err(PluginApiErrorCode::Unavailable);
    }
    let resolved_ips = addresses
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    Ok(PreparedNetworkEndpoint {
        canonical_url: url.to_string(),
        scheme,
        host,
        port,
        path_and_query,
        resolved_ips,
    })
}

pub(crate) struct NetworkDriver;

impl NetworkDriver {
    /// Registers a resource only. The guest learns that a connection is usable from its later
    /// `opened` event, never from this handle alone.
    #[cfg(test)]
    pub(crate) fn start(
        resources: &ResourceRegistry,
        owner: ResourceOwner,
        endpoint: PreparedNetworkEndpoint,
        request: PluginNetworkStartRequest,
        fence: ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        Self::start_with_credential(resources, owner, endpoint, request, fence, None)
    }

    pub(crate) fn start_with_credential(
        resources: &ResourceRegistry,
        owner: ResourceOwner,
        endpoint: PreparedNetworkEndpoint,
        request: PluginNetworkStartRequest,
        fence: ResourceFence,
        credential: Option<CredentialLease>,
    ) -> Result<String, PluginApiErrorCode> {
        validate_start(&endpoint, &request)?;
        if request.credential.is_some() != credential.is_some() {
            return Err(PluginApiErrorCode::PermissionDenied);
        }
        let fence: ResourceFence = if let Some(lease) = credential.as_ref() {
            let headers = match &request.operation {
                PluginNetworkOperation::Http { headers, .. }
                | PluginNetworkOperation::WebSocket { headers } => headers,
                _ => return Err(PluginApiErrorCode::InvalidRequest),
            };
            if headers
                .iter()
                .any(|header| header.name.eq_ignore_ascii_case(&lease.header_name))
            {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            let _ = sensitive_credential_header(lease)?;
            let credential_fence = lease.fence.clone();
            Arc::new(move || fence() && credential_fence())
        } else {
            fence
        };
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        match request.operation.clone() {
            PluginNetworkOperation::Http {
                method,
                headers,
                body_base64,
            } => {
                let body = decode_bounded(&body_base64, MAX_NETWORK_BODY_BYTES)?;
                resources.spawn(owner, "network-http", move |cancel, events| async move {
                    drive_http(
                        cancel,
                        events,
                        endpoint,
                        request.timeout_ms,
                        method,
                        headers,
                        body,
                        fence,
                        credential,
                    )
                    .await;
                    Ok(())
                })
            }
            PluginNetworkOperation::WebSocket { headers } => resources.spawn_with_commands(
                owner,
                "network-websocket",
                move |cancel, events, commands| async move {
                    drive_websocket(
                        cancel,
                        events,
                        commands,
                        endpoint,
                        request.timeout_ms,
                        headers,
                        fence,
                        credential,
                    )
                    .await;
                    Ok(())
                },
            ),
            PluginNetworkOperation::Tcp {} => resources.spawn_with_commands(
                owner,
                "network-tcp",
                move |cancel, events, commands| async move {
                    drive_tcp(
                        cancel,
                        events,
                        commands,
                        endpoint,
                        request.timeout_ms,
                        fence,
                    )
                    .await;
                    Ok(())
                },
            ),
            PluginNetworkOperation::Udp {} => resources.spawn_with_commands(
                owner,
                "network-udp",
                move |cancel, events, commands| async move {
                    drive_udp(
                        cancel,
                        events,
                        commands,
                        endpoint,
                        request.timeout_ms,
                        fence,
                    )
                    .await;
                    Ok(())
                },
            ),
            PluginNetworkOperation::Tls {} => resources.spawn_with_commands(
                owner,
                "network-tls",
                move |cancel, events, commands| async move {
                    drive_tls(
                        cancel,
                        events,
                        commands,
                        endpoint,
                        request.timeout_ms,
                        fence,
                    )
                    .await;
                    Ok(())
                },
            ),
        }
    }

    /// The resource registry binds the handle to its exact owner. The active capability/lifetime
    /// fence is checked here and again by the driver immediately before the socket write.
    pub(crate) async fn send(
        resources: &ResourceRegistry,
        owner: &ResourceOwner,
        request: PluginNetworkSendRequest,
        fence: &ResourceFence,
    ) -> Result<(), PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let data = decode_bounded(&request.data_base64, MAX_NETWORK_DATA_BYTES)?;
        if data.is_empty() {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        resources.send(owner, &request.handle, data, &**fence).await
    }
}

pub(crate) fn validate_start(
    endpoint: &PreparedNetworkEndpoint,
    request: &PluginNetworkStartRequest,
) -> Result<(), PluginApiErrorCode> {
    if !(MIN_TIMEOUT_MILLISECONDS..=MAX_TIMEOUT_MILLISECONDS).contains(&request.timeout_ms)
        || endpoint.resolved_ips.is_empty()
        || endpoint.resolved_ips.len() > MAX_NETWORK_ADDRESSES
        || endpoint.path_and_query.is_empty()
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    let scheme_matches = matches!(
        (&endpoint.scheme, &request.operation),
        (
            PluginNetworkScheme::Http | PluginNetworkScheme::Https,
            PluginNetworkOperation::Http { .. }
        ) | (
            PluginNetworkScheme::Ws | PluginNetworkScheme::Wss,
            PluginNetworkOperation::WebSocket { .. }
        ) | (PluginNetworkScheme::Tcp, PluginNetworkOperation::Tcp {})
            | (PluginNetworkScheme::Udp, PluginNetworkOperation::Udp {})
            | (PluginNetworkScheme::Tls, PluginNetworkOperation::Tls {})
    );
    if !scheme_matches {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    if request.credential.is_some()
        && !matches!(
            endpoint.scheme,
            PluginNetworkScheme::Https | PluginNetworkScheme::Wss
        )
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    match &request.operation {
        PluginNetworkOperation::Http {
            headers,
            body_base64,
            ..
        } => {
            validate_headers(headers, false)?;
            let _ = decode_bounded(body_base64, MAX_NETWORK_BODY_BYTES)?;
        }
        PluginNetworkOperation::WebSocket { headers } => validate_headers(headers, true)?,
        PluginNetworkOperation::Tcp {}
        | PluginNetworkOperation::Udp {}
        | PluginNetworkOperation::Tls {} => {}
    }
    Ok(())
}

fn decode_bounded(value: &str, limit: usize) -> Result<Vec<u8>, PluginApiErrorCode> {
    if value.len() > limit.saturating_mul(2) {
        return Err(PluginApiErrorCode::QuotaExceeded);
    }
    let decoded = BASE64
        .decode(value)
        .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
    if decoded.len() > limit {
        return Err(PluginApiErrorCode::QuotaExceeded);
    }
    Ok(decoded)
}

fn validate_headers(
    headers: &[PluginNetworkHeader],
    websocket: bool,
) -> Result<(), PluginApiErrorCode> {
    if headers.len() > MAX_NETWORK_HEADERS
        || headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>()
            > MAX_NETWORK_HEADER_BYTES
    {
        return Err(PluginApiErrorCode::QuotaExceeded);
    }
    for header in headers {
        let name = reqwest::header::HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        reqwest::header::HeaderValue::from_str(&header.value)
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let protected = matches!(
            name.as_str(),
            "host" | "connection" | "upgrade" | "content-length" | "transfer-encoding"
        ) || name.as_str().starts_with("proxy-")
            || (websocket && name.as_str().starts_with("sec-websocket-"));
        if protected {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
    }
    Ok(())
}

fn socket_addresses(
    endpoint: &PreparedNetworkEndpoint,
) -> Result<Vec<SocketAddr>, PluginApiErrorCode> {
    endpoint
        .resolved_ips
        .iter()
        .map(|value| {
            value
                .parse::<IpAddr>()
                .map(|ip| SocketAddr::new(ip, endpoint.port))
                .map_err(|_| PluginApiErrorCode::InvalidRequest)
        })
        .collect()
}

async fn connect_tcp(
    endpoint: &PreparedNetworkEndpoint,
    timeout_ms: u32,
    cancel: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
) -> Result<TcpStream, PluginNetworkErrorCode> {
    let addresses =
        socket_addresses(endpoint).map_err(|_| PluginNetworkErrorCode::InvalidEndpoint)?;
    match guarded(
        TcpStream::connect(addresses.as_slice()),
        cancel,
        fence,
        timeout_ms,
    )
    .await
    {
        Guarded::Completed(Ok(stream)) => Ok(stream),
        Guarded::Completed(Err(_)) => Err(PluginNetworkErrorCode::ConnectFailed),
        Guarded::Cancelled => Err(PluginNetworkErrorCode::Cancelled),
        Guarded::Revoked => Err(PluginNetworkErrorCode::Revoked),
        Guarded::TimedOut => Err(PluginNetworkErrorCode::TimedOut),
    }
}

async fn tls_connect(
    endpoint: &PreparedNetworkEndpoint,
    timeout_ms: u32,
    cancel: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
) -> Result<TlsStream<TcpStream>, PluginNetworkErrorCode> {
    let connector = tls_connector().map_err(|_| PluginNetworkErrorCode::TlsFailed)?;
    tls_connect_with_connector(endpoint, timeout_ms, cancel, fence, connector).await
}

async fn tls_connect_with_connector(
    endpoint: &PreparedNetworkEndpoint,
    timeout_ms: u32,
    cancel: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
    connector: TlsConnector,
) -> Result<TlsStream<TcpStream>, PluginNetworkErrorCode> {
    let stream = connect_tcp(endpoint, timeout_ms, cancel, fence).await?;
    let server_name = rustls::pki_types::ServerName::try_from(endpoint.host.clone())
        .map_err(|_| PluginNetworkErrorCode::InvalidEndpoint)?;
    match guarded(
        connector.connect(server_name, stream),
        cancel,
        fence,
        timeout_ms,
    )
    .await
    {
        Guarded::Completed(Ok(stream)) => Ok(stream),
        Guarded::Completed(Err(_)) => Err(PluginNetworkErrorCode::TlsFailed),
        Guarded::Cancelled => Err(PluginNetworkErrorCode::Cancelled),
        Guarded::Revoked => Err(PluginNetworkErrorCode::Revoked),
        Guarded::TimedOut => Err(PluginNetworkErrorCode::TimedOut),
    }
}

enum Guarded<T, E> {
    Completed(Result<T, E>),
    Cancelled,
    Revoked,
    TimedOut,
}

/// Drops an in-flight connect, handshake, read or write as soon as its resource is cancelled or
/// revoked. Polling the lifetime fence every 100ms also closes the creation/disable race.
async fn guarded<T, E, F>(
    operation: F,
    cancel: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
    timeout_ms: u32,
) -> Guarded<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    tokio::pin!(operation);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(u64::from(timeout_ms));
    loop {
        if !fence() {
            return Guarded::Revoked;
        }
        if *cancel.borrow_and_update() {
            return Guarded::Cancelled;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Guarded::TimedOut;
        }
        tokio::select! {
            result = &mut operation => return Guarded::Completed(result),
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow_and_update() {
                    return Guarded::Cancelled;
                }
            }
            _ = tokio::time::sleep(remaining.min(Duration::from_millis(100))) => {}
        }
    }
}

/// Awaits one socket operation while retaining the resource lifecycle checks for long-lived read
/// loops. Unlike `guarded`, reads have no arbitrary idle timeout; they still stop within the
/// fence polling interval when the resource is cancelled, revoked, or its peer half stops.
async fn lifecycle_io<T, E, F>(
    operation: F,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
) -> Option<Result<T, E>>
where
    F: Future<Output = Result<T, E>>,
{
    tokio::pin!(operation);
    loop {
        if !fence() || *cancel.borrow_and_update() || *stop.borrow_and_update() {
            return None;
        }
        tokio::select! {
            result = &mut operation => return Some(result),
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow_and_update() {
                    return None;
                }
            }
            changed = stop.changed() => {
                let _ = changed;
                return None;
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

/// A peer reader may stop while a write is in flight. The writer must then acknowledge that
/// payload as outcome-unknown rather than let the command's acknowledgement sender disappear.
async fn guarded_with_stop<T, E, F>(
    operation: F,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
    timeout_ms: u32,
) -> Guarded<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    if *stop.borrow_and_update() {
        return Guarded::Cancelled;
    }
    tokio::select! {
        result = guarded(operation, cancel, fence, timeout_ms) => result,
        changed = stop.changed() => {
            let _ = changed;
            Guarded::Cancelled
        }
    }
}

async fn emit_while_active(
    events: &ResourceEventWriter,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
    event: PluginNetworkEvent,
) -> Result<(), PluginApiErrorCode> {
    if *stop.borrow_and_update() {
        return Err(PluginApiErrorCode::Cancelled);
    }
    tokio::select! {
        result = emit(events, cancel, fence, event) => result,
        changed = stop.changed() => {
            let _ = changed;
            Err(PluginApiErrorCode::Cancelled)
        }
    }
}

async fn next_command(
    commands: &mut ResourceCommandReceiver,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
) -> Option<ResourceCommand> {
    loop {
        if !fence() || *cancel.borrow_and_update() || *stop.borrow_and_update() {
            return None;
        }
        tokio::select! {
            command = commands.recv() => return command,
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow_and_update() {
                    return None;
                }
            }
            changed = stop.changed() => {
                let _ = changed;
                return None;
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

fn tls_connector() -> Result<TlsConnector, ()> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for certificate in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(certificate);
    }
    tls_connector_with_roots(roots)
}

fn tls_connector_with_roots(roots: rustls::RootCertStore) -> Result<TlsConnector, ()> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| ())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

async fn emit(
    events: &ResourceEventWriter,
    cancel: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
    event: PluginNetworkEvent,
) -> Result<(), PluginApiErrorCode> {
    events
        .emit_backpressured(
            PluginApiResourceEventKind::Network { event },
            cancel,
            &**fence,
        )
        .await
}

async fn emit_error(
    events: &ResourceEventWriter,
    cancel: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
    code: PluginNetworkErrorCode,
) {
    let _ = emit(events, cancel, fence, PluginNetworkEvent::Error { code }).await;
}

#[allow(clippy::too_many_arguments)]
async fn drive_http(
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    endpoint: PreparedNetworkEndpoint,
    timeout_ms: u32,
    method: PluginHttpMethod,
    headers: Vec<PluginNetworkHeader>,
    body: Vec<u8>,
    fence: ResourceFence,
    credential: Option<CredentialLease>,
) {
    if !fence() {
        return;
    }
    let addresses = match socket_addresses(&endpoint) {
        Ok(value) => value,
        Err(_) => {
            emit_error(
                &events,
                &mut cancel,
                &fence,
                PluginNetworkErrorCode::InvalidEndpoint,
            )
            .await;
            return;
        }
    };
    let client = match Client::builder()
        .no_proxy()
        .no_hickory_dns()
        .redirect(redirect::Policy::none())
        .resolve_to_addrs(&endpoint.host, &addresses)
        .timeout(Duration::from_millis(u64::from(timeout_ms)))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            emit_error(
                &events,
                &mut cancel,
                &fence,
                PluginNetworkErrorCode::Unavailable,
            )
            .await;
            return;
        }
    };
    let method = match method {
        PluginHttpMethod::Get => Method::GET,
        PluginHttpMethod::Head => Method::HEAD,
        PluginHttpMethod::Post => Method::POST,
        PluginHttpMethod::Put => Method::PUT,
        PluginHttpMethod::Patch => Method::PATCH,
        PluginHttpMethod::Delete => Method::DELETE,
    };
    let mut request = client.request(method, &endpoint.canonical_url).body(body);
    for header in headers {
        let Ok(name) = reqwest::header::HeaderName::from_bytes(header.name.as_bytes()) else {
            emit_error(
                &events,
                &mut cancel,
                &fence,
                PluginNetworkErrorCode::ProtocolFailed,
            )
            .await;
            return;
        };
        let Ok(value) = reqwest::header::HeaderValue::from_str(&header.value) else {
            emit_error(
                &events,
                &mut cancel,
                &fence,
                PluginNetworkErrorCode::ProtocolFailed,
            )
            .await;
            return;
        };
        request = request.header(name, value);
    }
    if let Some(credential) = credential {
        let Ok((name, value)) = sensitive_credential_header(&credential) else {
            return;
        };
        request = request.header(name, value);
    }
    let mut response = match guarded(request.send(), &mut cancel, &fence, timeout_ms).await {
        Guarded::Completed(Ok(response)) => response,
        Guarded::Completed(Err(error)) => {
            let code = if error.is_timeout() {
                PluginNetworkErrorCode::TimedOut
            } else {
                PluginNetworkErrorCode::HttpFailed
            };
            emit_error(&events, &mut cancel, &fence, code).await;
            return;
        }
        Guarded::Cancelled | Guarded::Revoked => return,
        Guarded::TimedOut => {
            emit_error(
                &events,
                &mut cancel,
                &fence,
                PluginNetworkErrorCode::TimedOut,
            )
            .await;
            return;
        }
    };
    if emit(
        &events,
        &mut cancel,
        &fence,
        PluginNetworkEvent::Opened {
            protocol: PluginNetworkProtocol::Http,
            peer_address: None,
        },
    )
    .await
    .is_err()
    {
        return;
    }
    let headers = response_headers(response.headers());
    if emit(
        &events,
        &mut cancel,
        &fence,
        PluginNetworkEvent::HttpResponse {
            status: response.status().as_u16(),
            headers,
        },
    )
    .await
    .is_err()
    {
        return;
    }
    loop {
        match guarded(response.chunk(), &mut cancel, &fence, timeout_ms).await {
            Guarded::Completed(Ok(Some(chunk))) => {
                for part in chunk.chunks(MAX_NETWORK_DATA_BYTES) {
                    if emit(
                        &events,
                        &mut cancel,
                        &fence,
                        PluginNetworkEvent::Data {
                            data_base64: BASE64.encode(part),
                        },
                    )
                    .await
                    .is_err()
                    {
                        return;
                    }
                }
            }
            Guarded::Completed(Ok(None)) => {
                let _ = emit(&events, &mut cancel, &fence, PluginNetworkEvent::Closed {}).await;
                return;
            }
            Guarded::Completed(Err(error)) => {
                let code = if error.is_timeout() {
                    PluginNetworkErrorCode::TimedOut
                } else {
                    PluginNetworkErrorCode::HttpFailed
                };
                emit_error(&events, &mut cancel, &fence, code).await;
                return;
            }
            Guarded::Cancelled | Guarded::Revoked => return,
            Guarded::TimedOut => {
                emit_error(
                    &events,
                    &mut cancel,
                    &fence,
                    PluginNetworkErrorCode::TimedOut,
                )
                .await;
                return;
            }
        }
    }
}

fn response_headers(headers: &reqwest::header::HeaderMap) -> Vec<PluginNetworkHeader> {
    let mut bytes = 0usize;
    headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str();
            if matches!(
                name,
                "set-cookie" | "www-authenticate" | "proxy-authenticate"
            ) {
                return None;
            }
            let value = value.to_str().ok()?;
            let entry_bytes = name.len() + value.len();
            if entry_bytes > 512 || bytes + entry_bytes > MAX_NETWORK_HEADER_BYTES {
                return None;
            }
            bytes += entry_bytes;
            Some(PluginNetworkHeader {
                name: name.to_owned(),
                value: value.to_owned(),
            })
        })
        .take(MAX_NETWORK_HEADERS)
        .collect()
}

async fn drive_tcp(
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    commands: ResourceCommandReceiver,
    endpoint: PreparedNetworkEndpoint,
    timeout_ms: u32,
    fence: ResourceFence,
) {
    match connect_tcp(&endpoint, timeout_ms, &mut cancel, &fence).await {
        Ok(stream) => {
            let peer_address = stream.peer_addr().ok().map(|address| address.to_string());
            drive_stream(
                stream,
                &mut cancel,
                events,
                commands,
                fence,
                PluginNetworkProtocol::Tcp,
                peer_address,
            )
            .await
        }
        Err(code) => emit_error(&events, &mut cancel, &fence, code).await,
    }
}

async fn drive_tls(
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    commands: ResourceCommandReceiver,
    endpoint: PreparedNetworkEndpoint,
    timeout_ms: u32,
    fence: ResourceFence,
) {
    match tls_connect(&endpoint, timeout_ms, &mut cancel, &fence).await {
        Ok(stream) => {
            let peer_address = stream
                .get_ref()
                .0
                .peer_addr()
                .ok()
                .map(|address| address.to_string());
            drive_stream(
                stream,
                &mut cancel,
                events,
                commands,
                fence,
                PluginNetworkProtocol::Tls,
                peer_address,
            )
            .await
        }
        Err(code) => emit_error(&events, &mut cancel, &fence, code).await,
    }
}

async fn drive_stream<S>(
    stream: S,
    cancel: &mut watch::Receiver<bool>,
    events: ResourceEventWriter,
    mut commands: ResourceCommandReceiver,
    fence: ResourceFence,
    protocol: PluginNetworkProtocol,
    peer_address: Option<String>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    if emit(
        &events,
        cancel,
        &fence,
        PluginNetworkEvent::Opened {
            protocol,
            peer_address,
        },
    )
    .await
    .is_err()
    {
        return;
    }
    let (stop_sender, stop) = watch::channel(false);
    let mut reader_cancel = cancel.clone();
    let mut writer_cancel = cancel.clone();
    let mut reader_stop = stop.clone();
    let mut writer_stop = stop;
    let reader_stop_sender = stop_sender.clone();
    let reader_fence = fence.clone();
    let writer_fence = fence.clone();
    let reader = async {
        drive_stream_reader(
            &mut reader,
            &mut reader_cancel,
            &mut reader_stop,
            &events,
            &reader_fence,
            &reader_stop_sender,
        )
        .await;
        reader_stop_sender.send_replace(true);
    };
    let writer = async {
        drive_stream_writer(
            &mut writer,
            &mut commands,
            &mut writer_cancel,
            &mut writer_stop,
            &events,
            &writer_fence,
            &stop_sender,
        )
        .await;
        stop_sender.send_replace(true);
    };
    tokio::join!(reader, writer);
}

async fn drive_stream_reader<R>(
    reader: &mut R,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    events: &ResourceEventWriter,
    fence: &ResourceFence,
    stop_sender: &watch::Sender<bool>,
) where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0u8; MAX_NETWORK_DATA_BYTES];
    loop {
        match lifecycle_io(reader.read(&mut buffer), cancel, stop, fence).await {
            Some(Ok(0)) => {
                stop_sender.send_replace(true);
                let _ = emit(events, cancel, fence, PluginNetworkEvent::Closed {}).await;
                return;
            }
            Some(Ok(count)) => {
                if emit_while_active(
                    events,
                    cancel,
                    stop,
                    fence,
                    PluginNetworkEvent::Data {
                        data_base64: BASE64.encode(&buffer[..count]),
                    },
                )
                .await
                .is_err()
                {
                    return;
                }
            }
            Some(Err(_)) => {
                stop_sender.send_replace(true);
                emit_error(
                    events,
                    cancel,
                    fence,
                    PluginNetworkErrorCode::ProtocolFailed,
                )
                .await;
                return;
            }
            None => return,
        }
    }
}

async fn drive_stream_writer<W>(
    writer: &mut W,
    commands: &mut ResourceCommandReceiver,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    events: &ResourceEventWriter,
    fence: &ResourceFence,
    stop_sender: &watch::Sender<bool>,
) where
    W: AsyncWrite + Unpin,
{
    while let Some(command) = next_command(commands, cancel, stop, fence).await {
        if !fence() {
            command.finish(Err(PluginApiErrorCode::Revoked));
            return;
        }
        match guarded_with_stop(
            writer.write_all(command.payload()),
            cancel,
            stop,
            fence,
            10_000,
        )
        .await
        {
            Guarded::Completed(Ok(())) => command.finish(Ok(())),
            Guarded::Completed(Err(_)) => {
                command.finish(Err(PluginApiErrorCode::Unavailable));
                stop_sender.send_replace(true);
                emit_error(
                    events,
                    cancel,
                    fence,
                    PluginNetworkErrorCode::ProtocolFailed,
                )
                .await;
                return;
            }
            Guarded::Cancelled | Guarded::Revoked | Guarded::TimedOut => {
                command.finish(Err(PluginApiErrorCode::OutcomeUnknown));
                return;
            }
        }
    }
}

async fn drive_udp_socket(
    socket: UdpSocket,
    cancel: &mut watch::Receiver<bool>,
    events: ResourceEventWriter,
    mut commands: ResourceCommandReceiver,
    fence: ResourceFence,
) {
    let (stop_sender, stop) = watch::channel(false);
    let mut reader_cancel = cancel.clone();
    let mut writer_cancel = cancel.clone();
    let mut reader_stop = stop.clone();
    let mut writer_stop = stop;
    let reader_stop_sender = stop_sender.clone();
    let reader_fence = fence.clone();
    let writer_fence = fence.clone();
    let reader = async {
        drive_udp_reader(
            &socket,
            &mut reader_cancel,
            &mut reader_stop,
            &events,
            &reader_fence,
            &reader_stop_sender,
        )
        .await;
        reader_stop_sender.send_replace(true);
    };
    let writer = async {
        drive_udp_writer(
            &socket,
            &mut commands,
            &mut writer_cancel,
            &mut writer_stop,
            &events,
            &writer_fence,
            &stop_sender,
        )
        .await;
        stop_sender.send_replace(true);
    };
    tokio::join!(reader, writer);
}

async fn drive_udp_reader(
    socket: &UdpSocket,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    events: &ResourceEventWriter,
    fence: &ResourceFence,
    stop_sender: &watch::Sender<bool>,
) {
    let mut buffer = [0u8; 65_535];
    loop {
        match lifecycle_io(socket.recv(&mut buffer), cancel, stop, fence).await {
            Some(Ok(count)) if count <= MAX_NETWORK_DATA_BYTES => {
                if emit_while_active(
                    events,
                    cancel,
                    stop,
                    fence,
                    PluginNetworkEvent::Datagram {
                        data_base64: BASE64.encode(&buffer[..count]),
                    },
                )
                .await
                .is_err()
                {
                    return;
                }
            }
            Some(Ok(_)) | Some(Err(_)) => {
                stop_sender.send_replace(true);
                emit_error(
                    events,
                    cancel,
                    fence,
                    PluginNetworkErrorCode::ProtocolFailed,
                )
                .await;
                return;
            }
            None => return,
        }
    }
}

async fn drive_udp_writer(
    socket: &UdpSocket,
    commands: &mut ResourceCommandReceiver,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    events: &ResourceEventWriter,
    fence: &ResourceFence,
    stop_sender: &watch::Sender<bool>,
) {
    while let Some(command) = next_command(commands, cancel, stop, fence).await {
        if !fence() {
            command.finish(Err(PluginApiErrorCode::Revoked));
            return;
        }
        match guarded_with_stop(socket.send(command.payload()), cancel, stop, fence, 10_000).await {
            Guarded::Completed(Ok(_)) => command.finish(Ok(())),
            Guarded::Completed(Err(_)) => {
                command.finish(Err(PluginApiErrorCode::Unavailable));
                stop_sender.send_replace(true);
                emit_error(
                    events,
                    cancel,
                    fence,
                    PluginNetworkErrorCode::ProtocolFailed,
                )
                .await;
                return;
            }
            Guarded::Cancelled | Guarded::Revoked | Guarded::TimedOut => {
                command.finish(Err(PluginApiErrorCode::OutcomeUnknown));
                return;
            }
        }
    }
}

async fn drive_udp(
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    commands: ResourceCommandReceiver,
    endpoint: PreparedNetworkEndpoint,
    timeout_ms: u32,
    fence: ResourceFence,
) {
    let addresses = match socket_addresses(&endpoint) {
        Ok(value) => value,
        Err(_) => {
            emit_error(
                &events,
                &mut cancel,
                &fence,
                PluginNetworkErrorCode::InvalidEndpoint,
            )
            .await;
            return;
        }
    };
    let target = addresses[0];
    let bind = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = match guarded(UdpSocket::bind(bind), &mut cancel, &fence, timeout_ms).await {
        Guarded::Completed(Ok(socket)) => socket,
        Guarded::Completed(Err(_)) => {
            emit_error(
                &events,
                &mut cancel,
                &fence,
                PluginNetworkErrorCode::ConnectFailed,
            )
            .await;
            return;
        }
        Guarded::TimedOut => {
            emit_error(
                &events,
                &mut cancel,
                &fence,
                PluginNetworkErrorCode::TimedOut,
            )
            .await;
            return;
        }
        Guarded::Cancelled | Guarded::Revoked => return,
    };
    match guarded(socket.connect(target), &mut cancel, &fence, timeout_ms).await {
        Guarded::Completed(Ok(())) => {}
        Guarded::Completed(Err(_)) => {
            emit_error(
                &events,
                &mut cancel,
                &fence,
                PluginNetworkErrorCode::ConnectFailed,
            )
            .await;
            return;
        }
        Guarded::TimedOut => {
            emit_error(
                &events,
                &mut cancel,
                &fence,
                PluginNetworkErrorCode::TimedOut,
            )
            .await;
            return;
        }
        Guarded::Cancelled | Guarded::Revoked => return,
    }
    if emit(
        &events,
        &mut cancel,
        &fence,
        PluginNetworkEvent::Opened {
            protocol: PluginNetworkProtocol::Udp,
            peer_address: Some(target.to_string()),
        },
    )
    .await
    .is_err()
    {
        return;
    }
    drive_udp_socket(socket, &mut cancel, events, commands, fence).await;
}

// The resource task owns each independently fenced WebSocket input explicitly.
#[allow(clippy::too_many_arguments)]
async fn drive_websocket(
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    commands: ResourceCommandReceiver,
    endpoint: PreparedNetworkEndpoint,
    timeout_ms: u32,
    headers: Vec<PluginNetworkHeader>,
    fence: ResourceFence,
    credential: Option<CredentialLease>,
) {
    let request = match websocket_request(&endpoint, headers, credential.as_ref()) {
        Ok(request) => request,
        Err(_) => {
            emit_error(
                &events,
                &mut cancel,
                &fence,
                PluginNetworkErrorCode::InvalidEndpoint,
            )
            .await;
            return;
        }
    };
    drop(credential);
    if matches!(endpoint.scheme, PluginNetworkScheme::Ws) {
        let stream = match connect_tcp(&endpoint, timeout_ms, &mut cancel, &fence).await {
            Ok(stream) => stream,
            Err(code) => {
                emit_error(&events, &mut cancel, &fence, code).await;
                return;
            }
        };
        let peer = stream.peer_addr().ok().map(|address| address.to_string());
        match guarded(
            client_async(request, stream),
            &mut cancel,
            &fence,
            timeout_ms,
        )
        .await
        {
            Guarded::Completed(Ok((socket, _))) => {
                drive_websocket_stream(socket, &mut cancel, events, commands, fence, peer).await
            }
            Guarded::Completed(Err(_)) => {
                emit_error(
                    &events,
                    &mut cancel,
                    &fence,
                    PluginNetworkErrorCode::ProtocolFailed,
                )
                .await
            }
            Guarded::TimedOut => {
                emit_error(
                    &events,
                    &mut cancel,
                    &fence,
                    PluginNetworkErrorCode::TimedOut,
                )
                .await
            }
            Guarded::Cancelled | Guarded::Revoked => {}
        }
    } else {
        let stream = match tls_connect(&endpoint, timeout_ms, &mut cancel, &fence).await {
            Ok(stream) => stream,
            Err(code) => {
                emit_error(&events, &mut cancel, &fence, code).await;
                return;
            }
        };
        let peer = stream
            .get_ref()
            .0
            .peer_addr()
            .ok()
            .map(|address| address.to_string());
        match guarded(
            client_async(request, stream),
            &mut cancel,
            &fence,
            timeout_ms,
        )
        .await
        {
            Guarded::Completed(Ok((socket, _))) => {
                drive_websocket_stream(socket, &mut cancel, events, commands, fence, peer).await
            }
            Guarded::Completed(Err(_)) => {
                emit_error(
                    &events,
                    &mut cancel,
                    &fence,
                    PluginNetworkErrorCode::ProtocolFailed,
                )
                .await
            }
            Guarded::TimedOut => {
                emit_error(
                    &events,
                    &mut cancel,
                    &fence,
                    PluginNetworkErrorCode::TimedOut,
                )
                .await
            }
            Guarded::Cancelled | Guarded::Revoked => {}
        }
    }
}

fn websocket_request(
    endpoint: &PreparedNetworkEndpoint,
    headers: Vec<PluginNetworkHeader>,
    credential: Option<&CredentialLease>,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, PluginApiErrorCode> {
    validate_headers(&headers, true)?;
    let uri = endpoint
        .canonical_url
        .parse::<Uri>()
        .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
    let mut request = ClientRequestBuilder::new(uri);
    for header in headers {
        request = request.with_header(header.name, header.value);
    }
    let mut request = request
        .into_client_request()
        .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
    if let Some(credential) = credential {
        let (name, value) = sensitive_credential_header(credential)?;
        if request.headers().contains_key(&name) {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        request.headers_mut().insert(name, value);
    }
    Ok(request)
}

fn sensitive_credential_header(
    credential: &CredentialLease,
) -> Result<(reqwest::header::HeaderName, reqwest::header::HeaderValue), PluginApiErrorCode> {
    if !(credential.fence)() {
        return Err(PluginApiErrorCode::Revoked);
    }
    let name = reqwest::header::HeaderName::from_bytes(credential.header_name.as_bytes())
        .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
    let mut value = reqwest::header::HeaderValue::from_bytes(&credential.header_value)
        .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
    value.set_sensitive(true);
    Ok((name, value))
}

async fn drive_websocket_stream<S>(
    socket: WebSocketStream<S>,
    cancel: &mut watch::Receiver<bool>,
    events: ResourceEventWriter,
    mut commands: ResourceCommandReceiver,
    fence: ResourceFence,
    peer_address: Option<String>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut writer, mut reader) = socket.split();
    if emit(
        &events,
        cancel,
        &fence,
        PluginNetworkEvent::Opened {
            protocol: PluginNetworkProtocol::WebSocket,
            peer_address,
        },
    )
    .await
    .is_err()
    {
        return;
    }
    let (stop_sender, stop) = watch::channel(false);
    let (control_sender, controls) = mpsc::channel(16);
    let mut reader_cancel = cancel.clone();
    let mut writer_cancel = cancel.clone();
    let mut reader_stop = stop.clone();
    let mut writer_stop = stop;
    let reader_stop_sender = stop_sender.clone();
    let reader_fence = fence.clone();
    let writer_fence = fence.clone();
    let reader_task = async {
        drive_websocket_reader(
            &mut reader,
            &mut reader_cancel,
            &mut reader_stop,
            &events,
            &reader_fence,
            &reader_stop_sender,
            control_sender,
        )
        .await;
        reader_stop_sender.send_replace(true);
    };
    let writer_task = async {
        drive_websocket_writer(
            &mut writer,
            &mut commands,
            controls,
            &mut writer_cancel,
            &mut writer_stop,
            &events,
            &writer_fence,
            &stop_sender,
        )
        .await;
        stop_sender.send_replace(true);
    };
    tokio::join!(reader_task, writer_task);
}

async fn drive_websocket_reader<S>(
    reader: &mut futures_util::stream::SplitStream<WebSocketStream<S>>,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    events: &ResourceEventWriter,
    fence: &ResourceFence,
    stop_sender: &watch::Sender<bool>,
    controls: mpsc::Sender<Message>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let message = lifecycle_io(
            async { Ok::<_, ()>(reader.next().await) },
            cancel,
            stop,
            fence,
        )
        .await;
        match message {
            Some(Ok(Some(Ok(Message::Text(value))))) => {
                let bytes = value.as_bytes();
                if bytes.len() > MAX_NETWORK_DATA_BYTES {
                    stop_sender.send_replace(true);
                    emit_error(
                        events,
                        cancel,
                        fence,
                        PluginNetworkErrorCode::ProtocolFailed,
                    )
                    .await;
                    return;
                }
                if emit_while_active(
                    events,
                    cancel,
                    stop,
                    fence,
                    PluginNetworkEvent::Data {
                        data_base64: BASE64.encode(bytes),
                    },
                )
                .await
                .is_err()
                {
                    return;
                }
            }
            Some(Ok(Some(Ok(Message::Binary(value))))) => {
                if value.len() > MAX_NETWORK_DATA_BYTES {
                    stop_sender.send_replace(true);
                    emit_error(
                        events,
                        cancel,
                        fence,
                        PluginNetworkErrorCode::ProtocolFailed,
                    )
                    .await;
                    return;
                }
                if emit_while_active(
                    events,
                    cancel,
                    stop,
                    fence,
                    PluginNetworkEvent::Data {
                        data_base64: BASE64.encode(value),
                    },
                )
                .await
                .is_err()
                {
                    return;
                }
            }
            Some(Ok(Some(Ok(Message::Ping(value))))) => {
                if !enqueue_websocket_control(&controls, Message::Pong(value), cancel, stop, fence)
                    .await
                {
                    return;
                }
            }
            Some(Ok(Some(Ok(Message::Pong(_))))) => {}
            Some(Ok(Some(Ok(Message::Close(_))))) | Some(Ok(None)) => {
                stop_sender.send_replace(true);
                let _ = emit(events, cancel, fence, PluginNetworkEvent::Closed {}).await;
                return;
            }
            Some(Ok(Some(Ok(Message::Frame(_))))) | Some(Ok(Some(Err(_)))) | Some(Err(_)) => {
                stop_sender.send_replace(true);
                emit_error(
                    events,
                    cancel,
                    fence,
                    PluginNetworkErrorCode::ProtocolFailed,
                )
                .await;
                return;
            }
            None => return,
        }
    }
}

async fn enqueue_websocket_control(
    controls: &mpsc::Sender<Message>,
    message: Message,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
) -> bool {
    loop {
        if !fence() || *cancel.borrow_and_update() || *stop.borrow_and_update() {
            return false;
        }
        tokio::select! {
            permit = controls.reserve() => match permit {
                Ok(permit) => {
                    permit.send(message);
                    return true;
                }
                Err(_) => return false,
            },
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow_and_update() {
                    return false;
                }
            }
            changed = stop.changed() => {
                let _ = changed;
                return false;
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

// Writer ownership keeps the independently cancellable resource channels explicit.
#[allow(clippy::too_many_arguments)]
async fn drive_websocket_writer<S>(
    writer: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    commands: &mut ResourceCommandReceiver,
    mut controls: mpsc::Receiver<Message>,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    events: &ResourceEventWriter,
    fence: &ResourceFence,
    stop_sender: &watch::Sender<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        if !fence() || *cancel.borrow_and_update() || *stop.borrow_and_update() {
            return;
        }
        tokio::select! {
            control = controls.recv() => {
                let Some(control) = control else { return; };
                if !send_websocket_message(writer, control, cancel, stop, events, fence, stop_sender).await {
                    return;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return; };
                if !fence() {
                    command.finish(Err(PluginApiErrorCode::Revoked));
                    return;
                }
                let message = Message::Binary(Bytes::from(command.payload().to_vec()));
                match guarded_with_stop(writer.send(message), cancel, stop, fence, 10_000).await {
                    Guarded::Completed(Ok(())) => command.finish(Ok(())),
                    Guarded::Completed(Err(_)) => {
                        command.finish(Err(PluginApiErrorCode::Unavailable));
                        stop_sender.send_replace(true);
                        emit_error(events, cancel, fence, PluginNetworkErrorCode::ProtocolFailed).await;
                        return;
                    }
                    Guarded::Cancelled | Guarded::Revoked | Guarded::TimedOut => {
                        command.finish(Err(PluginApiErrorCode::OutcomeUnknown));
                        return;
                    }
                }
            }
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow_and_update() {
                    return;
                }
            }
            changed = stop.changed() => {
                let _ = changed;
                return;
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

async fn send_websocket_message<S>(
    writer: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    message: Message,
    cancel: &mut watch::Receiver<bool>,
    stop: &mut watch::Receiver<bool>,
    events: &ResourceEventWriter,
    fence: &ResourceFence,
    stop_sender: &watch::Sender<bool>,
) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match guarded_with_stop(writer.send(message), cancel, stop, fence, 10_000).await {
        Guarded::Completed(Ok(())) => true,
        Guarded::Completed(Err(_)) => {
            stop_sender.send_replace(true);
            emit_error(
                events,
                cancel,
                fence,
                PluginNetworkErrorCode::ProtocolFailed,
            )
            .await;
            false
        }
        Guarded::Cancelled | Guarded::Revoked | Guarded::TimedOut => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_core_api::{
        PluginApiResourceEventKind, PluginId, PluginNetworkCredentialRef, WireSequence,
    };
    use tokio_rustls::TlsAcceptor;

    fn endpoint(scheme: PluginNetworkScheme) -> PreparedNetworkEndpoint {
        PreparedNetworkEndpoint {
            canonical_url: match scheme {
                PluginNetworkScheme::Http => "http://network.test/".to_owned(),
                PluginNetworkScheme::Https => "https://network.test/".to_owned(),
                PluginNetworkScheme::Ws => "ws://network.test/".to_owned(),
                PluginNetworkScheme::Wss => "wss://network.test/".to_owned(),
                PluginNetworkScheme::Tcp => "tcp://network.test:9000".to_owned(),
                PluginNetworkScheme::Udp => "udp://network.test:9000".to_owned(),
                PluginNetworkScheme::Tls => "tls://network.test:9000".to_owned(),
            },
            scheme,
            host: "network.test".to_owned(),
            port: 443,
            path_and_query: "/".to_owned(),
            resolved_ips: vec!["127.0.0.1".to_owned()],
        }
    }

    fn credential_ref() -> PluginNetworkCredentialRef {
        PluginNetworkCredentialRef {
            handle: "credential".to_owned(),
            expected_revision: WireSequence::new(1),
        }
    }

    fn credential_lease(active: bool) -> CredentialLease {
        CredentialLease {
            header_name: "authorization".to_owned(),
            header_value: zeroize::Zeroizing::new(b"Bearer test-token".to_vec()),
            fence: Arc::new(move || active),
        }
    }

    fn tls_endpoint(port: u16, host: &str) -> PreparedNetworkEndpoint {
        PreparedNetworkEndpoint {
            canonical_url: format!("tls://{host}:{port}"),
            scheme: PluginNetworkScheme::Tls,
            host: host.to_owned(),
            port,
            path_and_query: "/".to_owned(),
            resolved_ips: vec!["127.0.0.1".to_owned()],
        }
    }

    fn test_tls_acceptor(
        names: Vec<String>,
    ) -> (TlsAcceptor, rustls::pki_types::CertificateDer<'static>) {
        let key = rcgen::KeyPair::generate().unwrap();
        let certificate = rcgen::CertificateParams::new(names)
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.der().clone()], key.into())
        .unwrap();
        (
            TlsAcceptor::from(Arc::new(config)),
            certificate.der().clone(),
        )
    }

    fn owner() -> ResourceOwner {
        ResourceOwner {
            plugin_id: PluginId::parse("org.norishell.network-test").unwrap(),
            signer: "a".repeat(64),
            package: "b".repeat(64),
            generation: WireSequence::new(1),
        }
    }

    fn http_get() -> PluginNetworkStartRequest {
        PluginNetworkStartRequest {
            credential: None,
            timeout_ms: 2_000,
            operation: PluginNetworkOperation::Http {
                method: PluginHttpMethod::Get,
                headers: vec![],
                body_base64: String::new(),
            },
        }
    }

    async fn events_until(
        resources: &ResourceRegistry,
        owner: &ResourceOwner,
        handle: &str,
        expected: usize,
    ) -> Vec<norishell_core_api::PluginApiResourceEvent> {
        let mut values = Vec::new();
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            values.extend(resources.take_events(owner, handle, 32).unwrap().0);
            if values.len() >= expected {
                return values;
            }
        }
        values
    }

    #[tokio::test]
    async fn endpoint_resolution_is_frozen_and_rejects_ambiguous_urls() {
        let endpoint = prepare_endpoint(&PluginNetworkEndpointRequest {
            endpoint: "tcp://127.0.0.1:7000".to_owned(),
        })
        .await
        .expect("loopback endpoint");
        assert_eq!(endpoint.resolved_ips, ["127.0.0.1"]);
        assert_eq!(endpoint.path_and_query, "/");
        for endpoint in [
            "https://user@example.test/",
            "https://example.test/#fragment",
            "tcp://example.test:443/path",
            "file:///tmp/nope",
        ] {
            assert!(
                prepare_endpoint(&PluginNetworkEndpointRequest {
                    endpoint: endpoint.into()
                })
                .await
                .is_err()
            );
        }
    }

    #[test]
    fn headers_and_payloads_are_bounded_without_control_characters() {
        assert!(
            validate_headers(
                &[PluginNetworkHeader {
                    name: "Host".into(),
                    value: "other.test".into()
                }],
                false
            )
            .is_err()
        );
        assert!(decode_bounded("!not-base64", 8).is_err());
        assert!(decode_bounded(&BASE64.encode(vec![0; 9]), 8).is_err());
    }

    #[test]
    fn credential_network_requests_are_limited_to_https_and_wss() {
        let http = PluginNetworkStartRequest {
            credential: Some(credential_ref()),
            timeout_ms: 2_000,
            operation: PluginNetworkOperation::Http {
                method: PluginHttpMethod::Get,
                headers: vec![],
                body_base64: String::new(),
            },
        };
        let websocket = PluginNetworkStartRequest {
            credential: Some(credential_ref()),
            timeout_ms: 2_000,
            operation: PluginNetworkOperation::WebSocket { headers: vec![] },
        };
        assert!(validate_start(&endpoint(PluginNetworkScheme::Https), &http).is_ok());
        assert!(validate_start(&endpoint(PluginNetworkScheme::Wss), &websocket).is_ok());
        assert!(validate_start(&endpoint(PluginNetworkScheme::Http), &http).is_err());
        assert!(validate_start(&endpoint(PluginNetworkScheme::Ws), &websocket).is_err());
    }

    #[test]
    fn credential_header_is_sensitive_and_cannot_conflict_with_guest_headers() {
        let (_, value) = sensitive_credential_header(&credential_lease(true)).unwrap();
        assert!(value.is_sensitive());
        assert_eq!(value.as_bytes(), b"Bearer test-token");
        assert!(matches!(
            sensitive_credential_header(&credential_lease(false)),
            Err(PluginApiErrorCode::Revoked)
        ));

        let request = PluginNetworkStartRequest {
            credential: Some(credential_ref()),
            timeout_ms: 2_000,
            operation: PluginNetworkOperation::Http {
                method: PluginHttpMethod::Get,
                headers: vec![PluginNetworkHeader {
                    name: "Authorization".to_owned(),
                    value: "guest-value".to_owned(),
                }],
                body_base64: String::new(),
            },
        };
        assert_eq!(
            NetworkDriver::start_with_credential(
                &ResourceRegistry::default(),
                owner(),
                endpoint(PluginNetworkScheme::Https),
                request,
                Arc::new(|| true),
                Some(credential_lease(true)),
            ),
            Err(PluginApiErrorCode::InvalidRequest)
        );

        assert!(
            websocket_request(
                &endpoint(PluginNetworkScheme::Wss),
                vec![PluginNetworkHeader {
                    name: "authorization".to_owned(),
                    value: "guest-value".to_owned(),
                }],
                Some(&credential_lease(true)),
            )
            .is_err()
        );

        let websocket = websocket_request(
            &endpoint(PluginNetworkScheme::Wss),
            vec![],
            Some(&credential_lease(true)),
        )
        .unwrap();
        assert!(websocket.headers()["authorization"].is_sensitive());

        let request = PluginNetworkStartRequest {
            credential: Some(credential_ref()),
            timeout_ms: 2_000,
            operation: PluginNetworkOperation::Http {
                method: PluginHttpMethod::Get,
                headers: vec![],
                body_base64: String::new(),
            },
        };
        assert!(matches!(
            NetworkDriver::start_with_credential(
                &ResourceRegistry::default(),
                owner(),
                endpoint(PluginNetworkScheme::Https),
                request,
                Arc::new(|| true),
                Some(credential_lease(false)),
            ),
            Err(PluginApiErrorCode::Revoked)
        ));
    }

    #[tokio::test]
    async fn loopback_http_streams_a_bounded_response_without_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                )
                .await
                .unwrap();
        });
        let endpoint = prepare_endpoint(&PluginNetworkEndpointRequest {
            endpoint: format!("http://127.0.0.1:{port}/fixed?x=1"),
        })
        .await
        .unwrap();
        let resources = ResourceRegistry::default();
        let owner = owner();
        let fence: ResourceFence = Arc::new(|| true);
        let handle =
            NetworkDriver::start(&resources, owner.clone(), endpoint, http_get(), fence).unwrap();
        let events = events_until(&resources, &owner, &handle, 3).await;
        assert!(events.iter().any(|event| matches!(
            event.kind,
            PluginApiResourceEventKind::Network {
                event: PluginNetworkEvent::HttpResponse { status: 200, .. }
            }
        )));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            PluginApiResourceEventKind::Network { event: PluginNetworkEvent::Data { data_base64 } }
                if BASE64.decode(data_base64).unwrap() == b"hello"
        )));
        resources.close(&owner, &handle).await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn loopback_tcp_and_udp_keep_writes_owner_scoped() {
        let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp_port = tcp_listener.local_addr().unwrap().port();
        let tcp_server = tokio::spawn(async move {
            let (mut stream, _) = tcp_listener.accept().await.unwrap();
            let mut bytes = [0u8; 4];
            stream.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes, b"ping");
            stream.write_all(b"pong").await.unwrap();
        });
        let resources = ResourceRegistry::default();
        let owner = owner();
        let fence: ResourceFence = Arc::new(|| true);
        let endpoint = prepare_endpoint(&PluginNetworkEndpointRequest {
            endpoint: format!("tcp://127.0.0.1:{tcp_port}"),
        })
        .await
        .unwrap();
        let handle = NetworkDriver::start(
            &resources,
            owner.clone(),
            endpoint,
            PluginNetworkStartRequest {
                credential: None,
                timeout_ms: 2_000,
                operation: PluginNetworkOperation::Tcp {},
            },
            fence.clone(),
        )
        .unwrap();
        NetworkDriver::send(
            &resources,
            &owner,
            PluginNetworkSendRequest {
                handle: handle.clone(),
                data_base64: BASE64.encode(b"ping"),
            },
            &fence,
        )
        .await
        .unwrap();
        let events = events_until(&resources, &owner, &handle, 2).await;
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            PluginApiResourceEventKind::Network { event: PluginNetworkEvent::Data { data_base64 } }
                if BASE64.decode(data_base64).unwrap() == b"pong"
        )));
        resources.close(&owner, &handle).await.unwrap();
        tcp_server.await.unwrap();

        let udp_server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let udp_port = udp_server.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut bytes = [0u8; 32];
            let (count, peer) = udp_server.recv_from(&mut bytes).await.unwrap();
            assert_eq!(&bytes[..count], b"ping");
            udp_server.send_to(b"pong", peer).await.unwrap();
        });
        let endpoint = prepare_endpoint(&PluginNetworkEndpointRequest {
            endpoint: format!("udp://127.0.0.1:{udp_port}"),
        })
        .await
        .unwrap();
        let udp_handle = NetworkDriver::start(
            &resources,
            owner.clone(),
            endpoint,
            PluginNetworkStartRequest {
                credential: None,
                timeout_ms: 2_000,
                operation: PluginNetworkOperation::Udp {},
            },
            fence.clone(),
        )
        .unwrap();
        NetworkDriver::send(
            &resources,
            &owner,
            PluginNetworkSendRequest {
                handle: udp_handle.clone(),
                data_base64: BASE64.encode(b"ping"),
            },
            &fence,
        )
        .await
        .unwrap();
        let events = events_until(&resources, &owner, &udp_handle, 2).await;
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            PluginApiResourceEventKind::Network { event: PluginNetworkEvent::Datagram { data_base64 } }
                if BASE64.decode(data_base64).unwrap() == b"pong"
        )));
        resources.close(&owner, &udp_handle).await.unwrap();
        server.await.unwrap();

        let websocket_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let websocket_port = websocket_listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = websocket_listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let message = socket.next().await.unwrap().unwrap();
            assert!(matches!(message, Message::Binary(value) if value.as_ref() == b"ping"));
            socket
                .send(Message::Binary(Bytes::from_static(b"pong")))
                .await
                .unwrap();
        });
        let endpoint = prepare_endpoint(&PluginNetworkEndpointRequest {
            endpoint: format!("ws://127.0.0.1:{websocket_port}/fixed"),
        })
        .await
        .unwrap();
        let websocket_handle = NetworkDriver::start(
            &resources,
            owner.clone(),
            endpoint,
            PluginNetworkStartRequest {
                credential: None,
                timeout_ms: 2_000,
                operation: PluginNetworkOperation::WebSocket { headers: vec![] },
            },
            fence.clone(),
        )
        .unwrap();
        NetworkDriver::send(
            &resources,
            &owner,
            PluginNetworkSendRequest {
                handle: websocket_handle.clone(),
                data_base64: BASE64.encode(b"ping"),
            },
            &fence,
        )
        .await
        .unwrap();
        let events = events_until(&resources, &owner, &websocket_handle, 2).await;
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            PluginApiResourceEventKind::Network { event: PluginNetworkEvent::Data { data_base64 } }
                if BASE64.decode(data_base64).unwrap() == b"pong"
        )));
        resources.close(&owner, &websocket_handle).await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn duplex_writes_progress_while_tcp_events_are_backpressured() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sent, sent_receiver) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let payload = vec![b'x'; MAX_NETWORK_DATA_BYTES];
            for _ in 0..48 {
                stream.write_all(&payload).await.unwrap();
            }
            let _ = sent.send(());
            let mut command = [0u8; 4];
            stream.read_exact(&mut command).await.unwrap();
            assert_eq!(&command, b"ping");
        });
        let resources = ResourceRegistry::default();
        let owner = owner();
        let fence: ResourceFence = Arc::new(|| true);
        let handle = NetworkDriver::start(
            &resources,
            owner.clone(),
            prepare_endpoint(&PluginNetworkEndpointRequest {
                endpoint: format!("tcp://127.0.0.1:{port}"),
            })
            .await
            .unwrap(),
            PluginNetworkStartRequest {
                credential: None,
                timeout_ms: 2_000,
                operation: PluginNetworkOperation::Tcp {},
            },
            fence.clone(),
        )
        .unwrap();
        sent_receiver.await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        tokio::time::timeout(
            Duration::from_secs(2),
            NetworkDriver::send(
                &resources,
                &owner,
                PluginNetworkSendRequest {
                    handle: handle.clone(),
                    data_base64: BASE64.encode(b"ping"),
                },
                &fence,
            ),
        )
        .await
        .expect("event backpressure must not block the TCP writer")
        .unwrap();
        resources.close(&owner, &handle).await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn duplex_writes_progress_while_udp_events_are_backpressured() {
        let server_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = server_socket.local_addr().unwrap().port();
        let (sent, sent_receiver) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut buffer = [0u8; MAX_NETWORK_DATA_BYTES];
            let (count, peer) = server_socket.recv_from(&mut buffer).await.unwrap();
            assert_eq!(&buffer[..count], b"ready");
            let payload = vec![b'x'; MAX_NETWORK_DATA_BYTES];
            for _ in 0..48 {
                server_socket.send_to(&payload, peer).await.unwrap();
            }
            let _ = sent.send(());
            let (count, peer_after) = server_socket.recv_from(&mut buffer).await.unwrap();
            assert_eq!(peer_after, peer);
            assert_eq!(&buffer[..count], b"ping");
        });
        let resources = ResourceRegistry::default();
        let owner = owner();
        let fence: ResourceFence = Arc::new(|| true);
        let handle = NetworkDriver::start(
            &resources,
            owner.clone(),
            prepare_endpoint(&PluginNetworkEndpointRequest {
                endpoint: format!("udp://127.0.0.1:{port}"),
            })
            .await
            .unwrap(),
            PluginNetworkStartRequest {
                credential: None,
                timeout_ms: 2_000,
                operation: PluginNetworkOperation::Udp {},
            },
            fence.clone(),
        )
        .unwrap();
        NetworkDriver::send(
            &resources,
            &owner,
            PluginNetworkSendRequest {
                handle: handle.clone(),
                data_base64: BASE64.encode(b"ready"),
            },
            &fence,
        )
        .await
        .unwrap();
        sent_receiver.await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        tokio::time::timeout(
            Duration::from_secs(2),
            NetworkDriver::send(
                &resources,
                &owner,
                PluginNetworkSendRequest {
                    handle: handle.clone(),
                    data_base64: BASE64.encode(b"ping"),
                },
                &fence,
            ),
        )
        .await
        .expect("event backpressure must not block the UDP writer")
        .unwrap();
        resources.close(&owner, &handle).await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn websocket_writes_progress_while_events_are_backpressured() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sent, sent_receiver) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let payload = Bytes::from(vec![b'x'; MAX_NETWORK_DATA_BYTES]);
            for _ in 0..48 {
                socket.send(Message::Binary(payload.clone())).await.unwrap();
            }
            let _ = sent.send(());
            let command = socket.next().await.unwrap().unwrap();
            assert!(matches!(command, Message::Binary(value) if value.as_ref() == b"ping"));
        });
        let resources = ResourceRegistry::default();
        let owner = owner();
        let fence: ResourceFence = Arc::new(|| true);
        let handle = NetworkDriver::start(
            &resources,
            owner.clone(),
            prepare_endpoint(&PluginNetworkEndpointRequest {
                endpoint: format!("ws://127.0.0.1:{port}/backpressure"),
            })
            .await
            .unwrap(),
            PluginNetworkStartRequest {
                credential: None,
                timeout_ms: 2_000,
                operation: PluginNetworkOperation::WebSocket { headers: vec![] },
            },
            fence.clone(),
        )
        .unwrap();
        sent_receiver.await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        tokio::time::timeout(
            Duration::from_secs(2),
            NetworkDriver::send(
                &resources,
                &owner,
                PluginNetworkSendRequest {
                    handle: handle.clone(),
                    data_base64: BASE64.encode(b"ping"),
                },
                &fence,
            ),
        )
        .await
        .expect("event backpressure must not block the WebSocket writer")
        .unwrap();
        resources.close(&owner, &handle).await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn tls_uses_the_production_verifier_and_test_root_only_for_valid_fixture() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (acceptor, certificate) = test_tls_acceptor(vec!["localhost".to_owned()]);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(stream).await.unwrap();
            let mut bytes = [0u8; 4];
            stream.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes, b"ping");
            stream.write_all(b"pong").await.unwrap();
        });
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certificate).unwrap();
        let connector = tls_connector_with_roots(roots).unwrap();
        let fence: ResourceFence = Arc::new(|| true);
        let (_cancel_sender, mut cancel) = watch::channel(false);
        let mut stream = tls_connect_with_connector(
            &tls_endpoint(port, "localhost"),
            2_000,
            &mut cancel,
            &fence,
            connector,
        )
        .await
        .unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut bytes = [0u8; 4];
        stream.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"pong");
        server.await.unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (acceptor, certificate) = test_tls_acceptor(vec!["network.test".to_owned()]);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            assert!(acceptor.accept(stream).await.is_err());
        });
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certificate).unwrap();
        let connector = tls_connector_with_roots(roots).unwrap();
        let (_cancel_sender, mut cancel) = watch::channel(false);
        assert!(matches!(
            tls_connect_with_connector(
                &tls_endpoint(port, "localhost"),
                2_000,
                &mut cancel,
                &fence,
                connector,
            )
            .await,
            Err(PluginNetworkErrorCode::TlsFailed)
        ));
        server.await.unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (acceptor, _) = test_tls_acceptor(vec!["localhost".to_owned()]);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            assert!(acceptor.accept(stream).await.is_err());
        });
        let (_cancel_sender, mut cancel) = watch::channel(false);
        assert!(matches!(
            tls_connect(&tls_endpoint(port, "localhost"), 2_000, &mut cancel, &fence).await,
            Err(PluginNetworkErrorCode::TlsFailed)
        ));
        server.await.unwrap();
    }
}
