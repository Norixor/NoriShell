//! Reliable RDP-UDP sideband, bound to the peer and certificate of the approved TCP connection.
use ironrdp::pdu::rdp::multitransport::{
    MultitransportRequestPdu, MultitransportResponsePdu, RequestedProtocol,
};
use ironrdp_rdpeudp_tokio::{MultitransportBootstrap, UdpTlsConfig, UdpTransport};
use ironrdp_tls::CertificateValidation;
use norishell_desktop_protocol::{EngineControl, EngineError, Result};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio_rustls::rustls::pki_types::UnixTime;
use x509_cert::{Certificate, der::Decode};

const MAX_UDP_PAYLOAD: usize = u16::MAX as usize;

fn approved_leaf_may_override_error(approved_leaf: &[u8], leaf: &[u8], reason: &str) -> bool {
    if leaf != approved_leaf {
        return false;
    }
    // The upstream callback receives the rustls error's Display text, not its
    // variant. Only trust and name errors can be covered by the TCP decision.
    let Some(certificate_error) = reason.strip_prefix("invalid peer certificate: ") else {
        return false;
    };
    if certificate_error != "UnknownIssuer"
        && certificate_error != "NotValidForName"
        && !certificate_error.starts_with("certificate not valid for name ")
    {
        return false;
    }
    // The certificate can expire between the TCP and UDP handshakes. Never use
    // the prior approval to override current validity or malformed DER.
    let Ok(cert) = Certificate::from_der(leaf) else {
        return false;
    };
    let validity = cert.tbs_certificate.validity;
    let now = UnixTime::now().as_secs();
    validity.not_before.to_unix_duration().as_secs() <= now
        && now <= validity.not_after.to_unix_duration().as_secs()
}

pub(super) struct UdpSession {
    peer: Option<SocketAddr>,
    server_name: String,
    approved_leaf: Vec<u8>,
    attempted: Vec<RequestedProtocol>,
    transport: Option<UdpTransport>,
}

impl UdpSession {
    pub fn new(peer: Option<SocketAddr>, server_name: &str, approved_leaf: Vec<u8>) -> Self {
        Self {
            peer,
            server_name: server_name.to_owned(),
            approved_leaf,
            attempted: Vec::with_capacity(2),
            transport: None,
        }
    }

    pub fn established(&self) -> bool {
        self.transport.is_some()
    }

    pub fn available(&self) -> bool {
        self.transport.as_ref().is_some_and(UdpTransport::is_alive)
    }

    pub fn discard(&mut self) {
        self.transport = None;
    }

    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        self.transport.as_mut()?.recv().await
    }

    pub async fn send(&self, payload: Vec<u8>) -> Result<()> {
        if payload.is_empty() || payload.len() > MAX_UDP_PAYLOAD {
            return Err(EngineError::ResourceLimit);
        }
        let transport = self.transport.as_ref().ok_or(EngineError::ConnectionLost)?;
        if !transport.is_alive() {
            return Err(EngineError::ConnectionLost);
        }
        tokio::time::timeout(Duration::from_secs(10), transport.send(payload))
            .await
            .map_err(|_| EngineError::Timeout)?
            .map_err(|_| EngineError::ConnectionLost)
    }

    pub async fn send_fenced(
        &self,
        payload: Vec<u8>,
        control: &EngineControl,
        epoch: u64,
    ) -> Result<()> {
        let mut focus = control.focus_epoch.clone();
        if *control.stop.borrow() || *focus.borrow_and_update() != epoch {
            return Err(EngineError::StaleInput);
        }
        tokio::select! { biased;
            _ = focus.changed() => Err(EngineError::ConnectionLost),
            result = self.send(payload) => result,
        }
    }

    /// A failed bootstrap is a TCP fallback. Once Soft-Sync migrates a channel, its
    /// transport failure is handled by the active loop as a disconnected session.
    pub async fn bootstrap(&mut self, request: MultitransportRequestPdu, soft_sync: bool) -> bool {
        let protocol = request.requested_protocol;
        if self.attempted.contains(&protocol) {
            return false;
        }
        self.attempted.push(protocol);
        let Some(peer) = self.peer else {
            return false;
        };
        if !soft_sync || protocol != RequestedProtocol::UdpFecR || self.transport.is_some() {
            return false;
        }

        let tls = self.tls_config();
        let mut bootstrap = MultitransportBootstrap::new(request);
        if let Err(error) = bootstrap
            .connect(
                peer,
                self.server_name.clone(),
                ironrdp_rdpeudp::ConnectionConfig::default(),
                tls,
            )
            .await
        {
            super::record_protocol_failure("udp.bootstrap", error.report());
            return false;
        }
        self.transport = bootstrap.take_transport();
        super::record_protocol_failure("udp.bootstrap", "established");
        self.transport.is_some()
    }

    fn tls_config(&self) -> UdpTlsConfig {
        let expected_for_callback = self.approved_leaf.clone();
        UdpTlsConfig {
            certificate_validation: CertificateValidation::Strict,
            // TCP already verified the signature and approved this leaf. Only
            // current-validity trust/name failures may reuse that decision;
            // the pin also covers paths where this callback is not invoked.
            certificate_validation_callback: Some(Arc::new(move |leaf, _, reason| {
                approved_leaf_may_override_error(&expected_for_callback, leaf, reason)
            })),
            certificate_validation_endpoint: self.server_name.clone(),
            expected_leaf_der: Some(self.approved_leaf.clone()),
        }
    }
}

pub(super) fn response(
    request_id: u32,
    success: bool,
    soft_sync: bool,
) -> Option<MultitransportResponsePdu> {
    use ironrdp::connector::MultitransportResult;
    let result = if success {
        MultitransportResult::Success
    } else {
        MultitransportResult::Failure(MultitransportResponsePdu::E_ABORT)
    };
    result.response_pdu(request_id, soft_sync)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_rustls::rustls::{self, pki_types::ServerName};

    #[test]
    fn gateway_and_missing_peer_never_advertise_udp() {
        let gateway = UdpSession::new(None, "rdp.test", vec![1, 2, 3]);
        assert!(gateway.peer.is_none());
        assert!(!gateway.established());
    }

    #[test]
    fn udp_tls_uses_strict_validation_and_exact_tcp_leaf() {
        let cert = test_certificate(2020, 2090);
        let session = UdpSession::new(
            Some("127.0.0.1:3389".parse().unwrap()),
            "rdp.test",
            cert.clone(),
        );
        let tls = session.tls_config();
        assert_eq!(tls.certificate_validation, CertificateValidation::Strict);
        assert_eq!(tls.expected_leaf_der, Some(cert.clone()));
        let callback = tls.certificate_validation_callback.unwrap();
        let unknown_issuer =
            rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer).to_string();
        let wrong_name =
            rustls::Error::InvalidCertificate(rustls::CertificateError::NotValidForNameContext {
                expected: ServerName::try_from("rdp.test").unwrap().to_owned(),
                presented: vec!["another.test".to_owned()],
            })
            .to_string();
        assert!(callback(&cert, "rdp.test", &unknown_issuer));
        assert!(callback(&cert, "rdp.test", &wrong_name));
        assert!(!callback(&[1, 2, 4], "rdp.test", &unknown_issuer));
    }

    fn test_certificate(not_before: i32, not_after: i32) -> Vec<u8> {
        let mut params = rcgen::CertificateParams::new(vec!["rdp.test".into()]).unwrap();
        params.not_before = rcgen::date_time_ymd(not_before, 1, 1);
        params.not_after = rcgen::date_time_ymd(not_after, 1, 1);
        let key = rcgen::KeyPair::generate().unwrap();
        params.self_signed(&key).unwrap().der().as_ref().to_vec()
    }

    #[test]
    fn udp_approval_cannot_override_expired_or_not_yet_valid_leaf() {
        for cert in [test_certificate(2000, 2001), test_certificate(2099, 2100)] {
            assert!(!approved_leaf_may_override_error(
                &cert,
                &cert,
                "invalid peer certificate: UnknownIssuer"
            ));
        }
    }

    #[test]
    fn udp_approval_cannot_override_other_certificate_failures() {
        let cert = test_certificate(2020, 2090);
        for reason in [
            "invalid peer certificate: Expired",
            "invalid peer certificate: BadEncoding",
            "invalid peer certificate: Revoked",
            "invalid peer certificate: BadSignature",
            "invalid peer certificate: InvalidPurpose",
            "invalid peer certificate: UnknownRevocationStatus",
            "invalid peer certificate: certificate expired: verification time 1 (UNIX)",
            "invalid peer certificate: Other(UnknownIssuer)",
            "unknown issuer",
        ] {
            assert!(
                !approved_leaf_may_override_error(&cert, &cert, reason),
                "{reason}"
            );
        }
        assert!(!approved_leaf_may_override_error(
            &[1, 2, 3],
            &[1, 2, 3],
            "invalid peer certificate: UnknownIssuer"
        ));
    }

    #[tokio::test]
    async fn gateway_declines_multitransport_without_opening_a_socket() {
        use ironrdp::pdu::rdp::headers::{BasicSecurityHeader, BasicSecurityHeaderFlags};
        let mut gateway = UdpSession::new(None, "rdp.test", vec![1, 2, 3]);
        let request = MultitransportRequestPdu {
            security_header: BasicSecurityHeader {
                flags: BasicSecurityHeaderFlags::TRANSPORT_REQ,
            },
            request_id: 7,
            requested_protocol: RequestedProtocol::UdpFecR,
            security_cookie: [0; 16],
        };
        assert!(!gateway.bootstrap(request.clone(), true).await);
        assert!(!gateway.bootstrap(request, true).await);
        assert!(!gateway.established());
        assert_eq!(gateway.attempted, vec![RequestedProtocol::UdpFecR]);
    }

    #[tokio::test]
    async fn stale_focus_cannot_enqueue_udp_input() {
        let session = UdpSession::new(None, "rdp.test", vec![1]);
        let (_stop_tx, stop) = tokio::sync::watch::channel(false);
        let (_focus_tx, focus_epoch) = tokio::sync::watch::channel(2);
        let control = EngineControl { stop, focus_epoch };
        assert_eq!(
            session.send_fenced(vec![1], &control, 1).await,
            Err(EngineError::StaleInput)
        );
    }

    #[test]
    fn failed_bootstrap_response_is_explicit_abort() {
        assert_eq!(
            response(7, false, true).unwrap().hr_response,
            MultitransportResponsePdu::E_ABORT
        );
    }
}
