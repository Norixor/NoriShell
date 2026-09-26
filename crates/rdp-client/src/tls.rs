use norishell_desktop_protocol::{BoxedDesktopIo, EngineError, Result};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio_rustls::{
    TlsConnector,
    rustls::{
        self, DigitallySignedStruct, SignatureScheme,
        client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        pki_types::{CertificateDer, ServerName, UnixTime},
    },
};
use x509_cert::der::Decode;

#[derive(Debug, Clone)]
pub struct CertificateChallenge {
    pub server_name: String,
    pub sha256_fingerprint: String,
}
/// Core must display the target and fingerprint in a protected window; approval is connection-scoped, never global trust.
pub type CertificateApproval =
    Arc<dyn Fn(CertificateChallenge) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

#[derive(Debug)]
struct Verifier {
    normal: Arc<rustls::client::WebPkiServerVerifier>,
    pending: Mutex<Option<Vec<u8>>>,
}
impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        leaf: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        match self.normal.verify_server_cert(leaf, chain, name, ocsp, now) {
            Ok(valid) => Ok(valid),
            Err(rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer)) => {
                // Temporarily complete the handshake only to verify proof of key possession; send no NLA or login material before approval.
                // An unknown CA, including Windows' default CN-only certificates, requires explicit approval of this exact
                // fingerprint. Do not treat the CN as a verified target name; validity and proof of key possession remain mandatory.
                let cert = x509_cert::Certificate::from_der(leaf.as_ref()).map_err(|_| {
                    rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
                })?;
                let validity = cert.tbs_certificate.validity;
                if now.as_secs() < validity.not_before.to_unix_duration().as_secs()
                    || now.as_secs() > validity.not_after.to_unix_duration().as_secs()
                {
                    return Err(rustls::Error::InvalidCertificate(
                        rustls::CertificateError::Expired,
                    ));
                }
                *self.pending.lock().map_err(|_| {
                    rustls::Error::General("certificate verification unavailable".into())
                })? = Some(leaf.to_vec());
                Ok(ServerCertVerified::assertion())
            }
            Err(error) => Err(error),
        }
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.normal.verify_tls12_signature(message, cert, signature)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.normal.verify_tls13_signature(message, cert, signature)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.normal.supported_verify_schemes()
    }
}

pub(crate) async fn upgrade(
    stream: BoxedDesktopIo,
    name: &str,
    approve: Option<&CertificateApproval>,
) -> Result<(BoxedDesktopIo, Vec<u8>, Vec<u8>)> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    let normal = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .map_err(|_| EngineError::CertificateRejected)?;
    let verifier = Arc::new(Verifier {
        normal,
        pending: Mutex::new(None),
    });
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| EngineError::CertificateRejected)?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    config.resumption = rustls::client::Resumption::disabled();
    config.key_log = Arc::new(rustls::NoKeyLog);
    let name_tls =
        ServerName::try_from(name.to_owned()).map_err(|_| EngineError::InvalidConfiguration)?;
    let tls = TlsConnector::from(Arc::new(config))
        .connect(name_tls, stream)
        .await
        .map_err(|_| EngineError::CertificateRejected)?;
    let pending = verifier
        .pending
        .lock()
        .map_err(|_| EngineError::CertificateRejected)?
        .take();
    if let Some(cert) = pending {
        let callback = approve.ok_or(EngineError::CertificateRejected)?;
        let challenge = CertificateChallenge {
            server_name: name.to_owned(),
            sha256_fingerprint: Sha256::digest(cert)
                .iter()
                .map(|v| format!("{v:02X}"))
                .collect::<Vec<_>>()
                .join(":"),
        };
        if !callback(challenge).await {
            return Err(EngineError::CertificateRejected);
        }
    }
    let cert = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .ok_or(EngineError::CertificateRejected)?;
    let leaf_der = cert.as_ref().to_vec();
    let cert = x509_cert::Certificate::from_der(&leaf_der)
        .map_err(|_| EngineError::CertificateRejected)?;
    let public_key = cert
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
        .ok_or(EngineError::CertificateRejected)?
        .to_vec();
    Ok((Box::new(tls), public_key, leaf_der))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::PrivatePkcs8KeyDer;
    use tokio_rustls::TlsAcceptor;
    fn server() -> (TlsAcceptor, String) {
        server_from_params(rcgen::CertificateParams::new(vec!["rdp.test".into()]).unwrap())
    }
    fn server_from_params(params: rcgen::CertificateParams) -> (TlsAcceptor, String) {
        let signing_key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&signing_key).unwrap();
        let fingerprint = Sha256::digest(cert.der())
            .iter()
            .map(|v| format!("{v:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        let key = PrivatePkcs8KeyDer::from(signing_key.serialize_der());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], key.into())
        .unwrap();
        (TlsAcceptor::from(Arc::new(config)), fingerprint)
    }
    #[tokio::test]
    async fn unknown_certificate_requires_explicit_approval() {
        let (acceptor, _) = server();
        let (client, peer) = tokio::io::duplex(16384);
        let task = tokio::spawn(async move { acceptor.accept(peer).await });
        assert!(matches!(
            upgrade(Box::new(client), "rdp.test", None).await,
            Err(EngineError::CertificateRejected)
        ));
        let _ = task.await;
    }
    #[tokio::test]
    async fn unknown_issuer_name_mismatch_requires_exact_connection_approval() {
        let (acceptor, fingerprint) = server();
        let (client, peer) = tokio::io::duplex(16384);
        let task = tokio::spawn(async move { acceptor.accept(peer).await });
        let approve: CertificateApproval = Arc::new(move |challenge| {
            assert_eq!(challenge.server_name, "other.test");
            assert_eq!(challenge.sha256_fingerprint, fingerprint);
            Box::pin(async { true })
        });
        assert!(
            upgrade(Box::new(client), "other.test", Some(&approve))
                .await
                .is_ok()
        );
        let _ = task.await;
    }

    #[test]
    fn unknown_issuer_does_not_bypass_validity() {
        let mut params = rcgen::CertificateParams::new(vec!["rdp.test".into()]).unwrap();
        params.not_before = rcgen::date_time_ymd(2000, 1, 1);
        params.not_after = rcgen::date_time_ymd(2001, 1, 1);
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let normal = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .unwrap();
        let verifier = Verifier {
            normal,
            pending: Mutex::new(None),
        };
        let name = ServerName::try_from("rdp.test").unwrap();
        assert!(
            verifier
                .verify_server_cert(cert.der(), &[], &name, &[], UnixTime::now())
                .is_err()
        );
        assert!(verifier.pending.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn approval_receives_exact_target_and_verified_leaf() {
        let (acceptor, fingerprint) = server();
        let (client, peer) = tokio::io::duplex(16384);
        let task = tokio::spawn(async move { acceptor.accept(peer).await });
        let approved: CertificateApproval = Arc::new(move |challenge| {
            assert_eq!(challenge.server_name, "rdp.test");
            assert_eq!(challenge.sha256_fingerprint, fingerprint);
            Box::pin(async { true })
        });
        let (stream, key, leaf) = upgrade(Box::new(client), "rdp.test", Some(&approved))
            .await
            .unwrap();
        assert!(!key.is_empty());
        assert!(!leaf.is_empty());
        task.await.unwrap().unwrap();
        drop(stream);
    }

    fn test_verifier(root: Option<CertificateDer<'static>>) -> Verifier {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        if let Some(root) = root {
            roots.add(root).unwrap();
        }
        Verifier {
            normal: rustls::client::WebPkiServerVerifier::builder_with_provider(
                Arc::new(roots),
                Arc::new(rustls::crypto::ring::default_provider()),
            )
            .build()
            .unwrap(),
            pending: Mutex::new(None),
        }
    }

    #[tokio::test]
    async fn windows_cn_only_certificate_requires_explicit_approval() {
        for approved in [false, true] {
            let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
            params
                .distinguished_name
                .push(rcgen::DnType::CommonName, "VINCENT5468");
            let (acceptor, fingerprint) = server_from_params(params);
            let (client, peer) = tokio::io::duplex(16384);
            let task = tokio::spawn(async move { acceptor.accept(peer).await });
            let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let observed = seen.clone();
            let approval: CertificateApproval = Arc::new(move |challenge| {
                assert_eq!(challenge.server_name, "10.211.55.4");
                assert_eq!(challenge.sha256_fingerprint, fingerprint);
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async move { approved })
            });
            let result = upgrade(Box::new(client), "10.211.55.4", Some(&approval)).await;
            assert!(seen.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(result.is_ok(), approved);
            if !approved {
                assert!(matches!(result, Err(EngineError::CertificateRejected)));
            }
            let _ = task.await;
        }
    }

    #[test]
    fn future_and_malformed_certificates_never_become_pending_approvals() {
        let mut params = rcgen::CertificateParams::new(vec!["rdp.test".into()]).unwrap();
        params.not_before = rcgen::date_time_ymd(2099, 1, 1);
        params.not_after = rcgen::date_time_ymd(2100, 1, 1);
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        for leaf in [cert.der().clone(), CertificateDer::from(vec![0, 1, 2, 3])] {
            let verifier = test_verifier(None);
            assert!(
                verifier
                    .verify_server_cert(
                        &leaf,
                        &[],
                        &ServerName::try_from("rdp.test").unwrap(),
                        &[],
                        UnixTime::now()
                    )
                    .is_err()
            );
            assert!(verifier.pending.lock().unwrap().is_none());
        }
    }

    #[test]
    fn trusted_name_validation_is_not_relaxed() {
        let generated = rcgen::generate_simple_self_signed(vec!["rdp.test".into()]).unwrap();
        let verifier = test_verifier(Some(generated.cert.der().clone()));
        assert!(
            verifier
                .verify_server_cert(
                    generated.cert.der(),
                    &[],
                    &ServerName::try_from("rdp.test").unwrap(),
                    &[],
                    UnixTime::now()
                )
                .is_ok()
        );
        assert!(verifier.pending.lock().unwrap().is_none());
        assert!(
            verifier
                .verify_server_cert(
                    generated.cert.der(),
                    &[],
                    &ServerName::try_from("other.test").unwrap(),
                    &[],
                    UnixTime::now()
                )
                .is_err()
        );
        assert!(verifier.pending.lock().unwrap().is_none());
    }

    async fn pending_approval_sends_no_application_data(cancel: bool) {
        use tokio::io::AsyncReadExt;
        let (acceptor, _) = server();
        let (client, peer) = tokio::io::duplex(16384);
        let accepted = tokio::spawn(async move { acceptor.accept(peer).await.unwrap() });
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (callback_entered, callback_release) = (entered.clone(), release.clone());
        let approval: CertificateApproval = Arc::new(move |_| {
            let (entered, release) = (callback_entered.clone(), callback_release.clone());
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                false
            })
        });
        let connection =
            tokio::spawn(
                async move { upgrade(Box::new(client), "rdp.test", Some(&approval)).await },
            );
        let mut peer = accepted.await.unwrap();
        entered.notified().await;
        let mut data = [0; 16];
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), peer.read(&mut data))
                .await
                .is_err()
        );
        if cancel {
            connection.abort();
            assert!(matches!(connection.await, Err(error) if error.is_cancelled()));
        } else {
            release.notify_one();
            assert!(matches!(
                connection.await.unwrap(),
                Err(EngineError::CertificateRejected)
            ));
        }
        let closed = tokio::time::timeout(std::time::Duration::from_secs(1), peer.read(&mut data))
            .await
            .expect("TLS stream must close after refusal or cancellation");
        assert!(matches!(closed, Ok(0) | Err(_)));
    }

    #[tokio::test]
    async fn refusal_closes_the_same_stream_without_login_data() {
        pending_approval_sends_no_application_data(false).await;
    }

    #[tokio::test]
    async fn cancelled_approval_closes_the_stream_without_login_data() {
        pending_approval_sends_no_application_data(true).await;
    }

    #[derive(Debug)]
    struct InvalidSigner;
    impl rustls::sign::SigningKey for InvalidSigner {
        fn choose_scheme(
            &self,
            offered: &[SignatureScheme],
        ) -> Option<Box<dyn rustls::sign::Signer>> {
            offered
                .contains(&SignatureScheme::ECDSA_NISTP256_SHA256)
                .then(|| Box::new(Self) as Box<dyn rustls::sign::Signer>)
        }
        fn algorithm(&self) -> rustls::SignatureAlgorithm {
            rustls::SignatureAlgorithm::ECDSA
        }
    }
    impl rustls::sign::Signer for InvalidSigner {
        fn sign(&self, _: &[u8]) -> std::result::Result<Vec<u8>, rustls::Error> {
            Ok(vec![0; 64])
        }
        fn scheme(&self) -> SignatureScheme {
            SignatureScheme::ECDSA_NISTP256_SHA256
        }
    }

    #[tokio::test]
    async fn invalid_handshake_signature_never_reaches_approval() {
        let generated = rcgen::generate_simple_self_signed(vec!["rdp.test".into()]).unwrap();
        let key = rustls::sign::CertifiedKey::new(
            vec![generated.cert.der().clone()],
            Arc::new(InvalidSigner),
        );
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(rustls::sign::SingleCertAndKey::from(key)));
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let (client, peer) = tokio::io::duplex(16384);
        let task = tokio::spawn(async move { acceptor.accept(peer).await });
        let approval: CertificateApproval =
            Arc::new(|_| panic!("invalid proof of key possession must not reach approval"));
        assert!(matches!(
            upgrade(Box::new(client), "rdp.test", Some(&approval)).await,
            Err(EngineError::CertificateRejected)
        ));
        let _ = task.await;
    }
}
