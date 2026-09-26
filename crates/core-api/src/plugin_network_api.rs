//! Typed, non-authoritative requests for plugin-owned outbound network resources.
//!
//! Core resolves an endpoint once and presents the complete frozen result for approval. A guest
//! never supplies the resulting addresses, an approval, an owner, or a resource generation.

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The endpoint to resolve once before an approval is created.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginNetworkEndpointRequest {
    pub endpoint: String,
}

impl fmt::Debug for PluginNetworkEndpointRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginNetworkEndpointRequest")
            .field("endpoint", &"[redacted]")
            .finish()
    }
}

/// The only endpoint a network driver may dial. It is constructed by Core and bound into the
/// protected approval fingerprint; it is not accepted as a guest request.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedNetworkEndpoint {
    pub canonical_url: String,
    pub scheme: PluginNetworkScheme,
    pub host: String,
    pub port: u16,
    /// This path and query come from `endpoint` and remain immutable after approval.
    pub path_and_query: String,
    /// Every address returned by the one DNS resolution. Core shows these before approval and the
    /// driver only dials one member of this frozen set.
    pub resolved_ips: Vec<String>,
}

impl fmt::Debug for PreparedNetworkEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedNetworkEndpoint")
            .field("canonical_url", &"[redacted]")
            .field("scheme", &self.scheme)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("path_and_query", &"[redacted]")
            .field("resolved_ips", &self.resolved_ips)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginNetworkScheme {
    Http,
    Https,
    Ws,
    Wss,
    Tcp,
    Udp,
    Tls,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginNetworkStartRequest {
    /// Includes connect, TLS handshake and first-response limits. It cannot disable the driver's
    /// independent resource and cancellation checks.
    pub timeout_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub credential: Option<crate::PluginNetworkCredentialRef>,
    /// Core resolves this plugin-owned OAuth session for the frozen endpoint origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub oauth_profile_id: Option<String>,
    pub operation: PluginNetworkOperation,
}

impl fmt::Debug for PluginNetworkStartRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginNetworkStartRequest")
            .field("timeout_ms", &self.timeout_ms)
            .field("oauth_profile_id", &self.oauth_profile_id)
            .field("operation", &self.operation)
            .finish()
    }
}

/// The operation is selected after endpoint preparation. It may never contain another URL,
/// authority, proxy, credential reference, owner, or approval token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginNetworkOperation {
    Http {
        method: PluginHttpMethod,
        headers: Vec<PluginNetworkHeader>,
        /// Standard base64 without a data-URL prefix. This avoids unbounded JSON number arrays.
        body_base64: String,
    },
    /// Request and response bodies are Core-owned opaque blobs; no exchange bytes enter Wasm.
    HttpExchange {
        method: PluginHttpMethod,
        headers: Vec<PluginNetworkHeader>,
        profile_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        body_blob_handle: Option<String>,
        max_response_bytes: u32,
    },
    WebSocket {
        headers: Vec<PluginNetworkHeader>,
    },
    Tcp {},
    Udp {},
    Tls {},
}

impl fmt::Debug for PluginNetworkOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http {
                method,
                headers,
                body_base64,
            } => formatter
                .debug_struct("Http")
                .field("method", method)
                .field("header_count", &headers.len())
                .field("body_base64_bytes", &body_base64.len())
                .finish(),
            Self::HttpExchange {
                method,
                headers,
                profile_id,
                body_blob_handle,
                max_response_bytes,
            } => formatter
                .debug_struct("HttpExchange")
                .field("method", method)
                .field("header_count", &headers.len())
                .field("profile_id", profile_id)
                .field("has_body_blob", &body_blob_handle.is_some())
                .field("max_response_bytes", max_response_bytes)
                .finish(),
            Self::WebSocket { headers } => formatter
                .debug_struct("WebSocket")
                .field("header_count", &headers.len())
                .finish(),
            Self::Tcp {} => formatter.write_str("Tcp"),
            Self::Udp {} => formatter.write_str("Udp"),
            Self::Tls {} => formatter.write_str("Tls"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginHttpMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginNetworkHeader {
    pub name: String,
    pub value: String,
}

impl fmt::Debug for PluginNetworkHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginNetworkHeader")
            .field("name", &self.name)
            .field("value", &"[redacted]")
            .finish()
    }
}

/// A write is routed by Core to an exact, owner-scoped network resource. A request has no target
/// address because a resource can only write to its prepared endpoint.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginNetworkSendRequest {
    pub handle: String,
    pub data_base64: String,
}

impl fmt::Debug for PluginNetworkSendRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginNetworkSendRequest")
            .field("handle", &self.handle)
            .field("data_base64_bytes", &self.data_base64.len())
            .finish()
    }
}

/// A bounded network event carried by the generic resource-event queue. Payload values are never
/// copied into Core logs or audit records.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginNetworkEvent {
    Opened {
        protocol: PluginNetworkProtocol,
        peer_address: Option<String>,
    },
    HttpResponse {
        status: u16,
        headers: Vec<PluginNetworkHeader>,
    },
    HttpExchangeCompleted {
        receipt_handle: String,
        status: u16,
        etag: Option<String>,
        body_blob_handle: Option<String>,
        byte_length: u32,
    },
    Data {
        data_base64: String,
    },
    Datagram {
        data_base64: String,
    },
    Closed {},
    Error {
        code: PluginNetworkErrorCode,
    },
}

impl fmt::Debug for PluginNetworkEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Opened {
                protocol,
                peer_address,
            } => formatter
                .debug_struct("Opened")
                .field("protocol", protocol)
                .field("peer_address", peer_address)
                .finish(),
            Self::HttpResponse { status, headers } => formatter
                .debug_struct("HttpResponse")
                .field("status", status)
                .field("header_count", &headers.len())
                .finish(),
            Self::HttpExchangeCompleted {
                status,
                byte_length,
                ..
            } => formatter
                .debug_struct("HttpExchangeCompleted")
                .field("status", status)
                .field("byte_length", byte_length)
                .finish_non_exhaustive(),
            Self::Data { data_base64 } => formatter
                .debug_struct("Data")
                .field("data_base64_bytes", &data_base64.len())
                .finish(),
            Self::Datagram { data_base64 } => formatter
                .debug_struct("Datagram")
                .field("data_base64_bytes", &data_base64.len())
                .finish(),
            Self::Closed {} => formatter.write_str("Closed"),
            Self::Error { code } => formatter.debug_struct("Error").field("code", code).finish(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginNetworkProtocol {
    Http,
    WebSocket,
    Tcp,
    Udp,
    Tls,
}

/// Stable errors deliberately exclude socket, certificate, request and response text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginNetworkErrorCode {
    InvalidEndpoint,
    ResolveFailed,
    ConnectFailed,
    TlsFailed,
    HttpFailed,
    ProtocolFailed,
    TimedOut,
    QuotaExceeded,
    OutcomeUnknown,
    Revoked,
    Cancelled,
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_requests_cannot_supply_authority_or_a_second_target() {
        let accepted = serde_json::json!({
            "timeoutMs": 1_000,
            "operation": {"kind": "http", "method": "get", "headers": [], "bodyBase64": ""}
        });
        assert!(serde_json::from_value::<PluginNetworkStartRequest>(accepted).is_ok());
        for request in [
            serde_json::json!({
                "timeoutMs": 1_000,
            "operation": {"kind": "tcp", "endpoint": "tcp://other.test:80"}
            }),
            serde_json::json!({
                "timeoutMs": 1_000,
                "approved": true,
                "operation": {"kind": "tcp"}
            }),
        ] {
            assert!(serde_json::from_value::<PluginNetworkStartRequest>(request).is_err());
        }
    }
}
