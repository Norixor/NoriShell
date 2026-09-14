//! Shared, platform-independent SSH endpoint and host-key primitives.

pub mod openssh_config;

use std::net::{IpAddr, Ipv6Addr};

use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EndpointError {
    #[error("SSH host address is empty")]
    Empty,
    #[error("SSH host address is not a supported IPv4, IPv6 or ASCII DNS name")]
    Invalid,
    #[error("SSH port must be between 1 and 65535")]
    InvalidPort,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Endpoint {
    normalized_address: String,
    port: u16,
}

impl Endpoint {
    pub fn parse(address: &str, port: u16) -> Result<Self, EndpointError> {
        if port == 0 {
            return Err(EndpointError::InvalidPort);
        }
        Ok(Self {
            normalized_address: normalize_host_address(address)?,
            port,
        })
    }

    #[must_use]
    pub fn normalized_address(&self) -> &str {
        &self.normalized_address
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn trust_key(&self) -> String {
        if self.normalized_address.parse::<Ipv6Addr>().is_ok() {
            format!("[{}]:{}", self.normalized_address, self.port)
        } else {
            format!("{}:{}", self.normalized_address, self.port)
        }
    }
}

pub fn normalize_host_address(input: &str) -> Result<String, EndpointError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(EndpointError::Empty);
    }
    let (address, bracketed) = match input
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        Some(address) => (address, true),
        None => (input, false),
    };
    if let Ok(ip) = address.parse::<IpAddr>() {
        if bracketed && !ip.is_ipv6() {
            return Err(EndpointError::Invalid);
        }
        return Ok(ip.to_string());
    }
    if bracketed || !address.is_ascii() {
        return Err(EndpointError::Invalid);
    }

    let hostname = address
        .strip_suffix('.')
        .unwrap_or(address)
        .to_ascii_lowercase();
    if hostname.is_empty() || hostname.len() > 253 {
        return Err(EndpointError::Invalid);
    }
    let labels = hostname.split('.').collect::<Vec<_>>();
    if labels.len() == 4
        && labels
            .iter()
            .all(|label| label.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(EndpointError::Invalid);
    }
    for label in labels {
        if label.is_empty()
            || label.len() > 63
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || !label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
        {
            return Err(EndpointError::Invalid);
        }
    }
    Ok(hostname)
}

#[must_use]
pub fn ssh_sha256_fingerprint(public_key_blob: &[u8]) -> String {
    format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(public_key_blob))
    )
}

#[cfg(test)]
mod tests {
    use super::{Endpoint, EndpointError, normalize_host_address, ssh_sha256_fingerprint};

    #[test]
    fn normalizes_dns_ipv4_and_ipv6_without_using_resolved_ips_as_identity() {
        assert_eq!(
            normalize_host_address(" EXAMPLE.COM. ").unwrap(),
            "example.com"
        );
        assert_eq!(
            normalize_host_address("192.168.001.1"),
            Err(EndpointError::Invalid)
        );
        assert_eq!(
            normalize_host_address("192.168.1.1").unwrap(),
            "192.168.1.1"
        );
        assert_eq!(
            normalize_host_address("[2001:0db8::1]").unwrap(),
            "2001:db8::1"
        );
        assert_eq!(
            Endpoint::parse("[2001:db8::1]", 2222).unwrap().trust_key(),
            "[2001:db8::1]:2222"
        );
    }

    #[test]
    fn rejects_ambiguous_or_non_ascii_hostnames_until_an_idna_contract_exists() {
        for invalid in ["", "bad..host", "-bad.test", "bad-.test", "例子.测试"] {
            assert!(
                normalize_host_address(invalid).is_err(),
                "accepted {invalid}"
            );
        }
        assert_eq!(
            Endpoint::parse("example.com", 0),
            Err(EndpointError::InvalidPort)
        );
    }

    #[test]
    fn formats_openssh_style_sha256_fingerprints_without_padding() {
        assert_eq!(
            ssh_sha256_fingerprint(b"host key blob"),
            "SHA256:A5ih/F93UXpmQXscb+3yzFThRX+eQsSL07H/uSIBPUw"
        );
    }
}
