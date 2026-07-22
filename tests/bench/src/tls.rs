//! TLS and self-signed certificate helpers for the benchmark harness.
//!
//! Adapted from `tonic-h3-tests/src/lib.rs` and `tonic-h3-tests/src/cert_gen.rs`.
//! These are benchmark-only helpers: the client uses a dangerous "accept any
//! certificate" verifier and the server uses a self-signed localhost cert. This
//! mirrors the existing integration-test harness and is NOT suitable for
//! production use.

use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// Generate a self-signed certificate and key pair for the given SANs.
pub fn make_test_cert(subject_alt_names: Vec<String>) -> (rcgen::Certificate, rcgen::KeyPair) {
    let key_pair = rcgen::generate_simple_self_signed(subject_alt_names).unwrap();
    (key_pair.cert, key_pair.signing_key)
}

/// Create cert/key PEM files on disk (needed by the quiche and msquic backends,
/// which take certificate file paths). Files are written under the OS temp dir.
pub fn make_test_cert_files(name: &str, regen: bool) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::io::Write;

    let temp_dir = std::env::temp_dir().join("tonic_h3_bench_certs").join(name);

    if regen {
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
    std::fs::create_dir_all(&temp_dir).expect("failed to create temp cert dir");

    let cert_path = temp_dir.join("cert.pem");
    let key_path = temp_dir.join("key.pem");
    if !key_path.exists() || !cert_path.exists() {
        let (cert, key) = make_test_cert(vec!["localhost".to_string(), "127.0.0.1".to_string()]);

        let mut cert_f = std::fs::File::create(&cert_path).expect("create cert file");
        cert_f
            .write_all(cert.pem().as_bytes())
            .expect("write cert file");

        let mut key_f = std::fs::File::create(&key_path).expect("create key file");
        key_f
            .write_all(key.serialize_pem().as_bytes())
            .expect("write key file");
    }
    (cert_path, key_path)
}

/// Self-signed localhost cert/key as rustls DER types.
pub fn make_test_cert_rustls() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let (cert, key_pair) = make_test_cert(vec!["localhost".to_string()]);
    let cert = CertificateDer::from(cert);
    use rustls::pki_types::pem::PemObject;
    let key = PrivateKeyDer::from_pem(
        rustls::pki_types::pem::SectionKind::PrivateKey,
        key_pair.serialize_der(),
    )
    .unwrap();
    (cert, key)
}

/// Build a rustls server config with the given ALPN protocols (`b"h3"` for the
/// QUIC backends, `b"h2"` for the TCP+TLS baseline).
pub fn make_server_config(alpn: &[&[u8]]) -> rustls::ServerConfig {
    let (cert, key) = make_test_cert_rustls();
    let mut tls_config = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)
    .unwrap();
    tls_config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    tls_config.max_early_data_size = u32::MAX;
    tls_config
}

/// Build a rustls client config with a dangerous no-verification verifier and
/// the given ALPN protocols. Benchmark-only.
pub fn make_danger_client_config(alpn: &[&[u8]]) -> rustls::ClientConfig {
    let mut tls_config = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(danger::NoCertificateVerification::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    )))
    .with_no_client_auth();
    tls_config.enable_early_data = true;
    tls_config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    tls_config
}

mod danger {
    use rustls::DigitallySignedStruct;
    use rustls::client::danger::HandshakeSignatureValid;
    use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

    #[derive(Debug)]
    pub struct NoCertificateVerification(CryptoProvider);

    impl NoCertificateVerification {
        pub fn new(provider: CryptoProvider) -> Self {
            Self(provider)
        }
    }

    impl rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp: &[u8],
            _now: UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls12_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls13_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }
}
