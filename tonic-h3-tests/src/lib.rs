use std::{net::SocketAddr, sync::Arc};

use tokio_util::sync::CancellationToken;

#[cfg(test)]
#[cfg(target_os = "windows")]
mod dotnet;

#[cfg(test)]
mod axum;

tonic::include_proto!("helloworld"); // The string specified here must match the proto package name

#[derive(Default)]
pub struct HelloWorldService {}

#[tonic::async_trait]
impl crate::greeter_server::Greeter for HelloWorldService {
    async fn say_hello(
        &self,
        req: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let name = req.into_inner().name;
        tracing::debug!("say_hello: {}", name);
        Ok(tonic::Response::new(HelloReply {
            message: format!("hello {}", name),
        }))
    }
}

fn make_test_cert(subject_alt_names: Vec<String>) -> (rcgen::Certificate, rcgen::KeyPair) {
    use rcgen::{generate_simple_self_signed, CertifiedKey};
    let CertifiedKey { cert, key_pair } = generate_simple_self_signed(subject_alt_names).unwrap();
    (cert, key_pair)
}

pub fn make_test_cert_rustls(
    subject_alt_names: Vec<String>,
) -> (
    rustls::pki_types::CertificateDer<'static>,
    rustls::pki_types::PrivateKeyDer<'static>,
) {
    let (cert, key_pair) = make_test_cert(subject_alt_names);
    let cert = rustls::pki_types::CertificateDer::from(cert);
    use rustls::pki_types::pem::PemObject;
    let key = rustls::pki_types::PrivateKeyDer::from_pem(
        rustls::pki_types::pem::SectionKind::PrivateKey,
        key_pair.serialize_der(),
    )
    .unwrap();
    (cert, key)
}

pub fn try_setup_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
}

pub fn make_quinn_server_endpoint(in_addr: SocketAddr) -> quinn::Endpoint {
    let tls_config = Arc::new(make_rustls_server_config());

    let server_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(tls_config).unwrap(),
    ));
    quinn::Endpoint::server(server_config, in_addr).unwrap()
}

// returns handle and listening addr
pub fn run_test_quinn_hello_server(
    in_addr: SocketAddr,
    token: CancellationToken,
) -> (
    tokio::task::JoinHandle<Result<(), tonic_h3::Error>>,
    SocketAddr,
) {
    let endpoint = make_quinn_server_endpoint(in_addr);

    let listen_addr = endpoint.local_addr().unwrap();
    tracing::debug!("listenaddr : {}", listen_addr);

    let hello_svc = crate::HelloWorldService {};
    let router = tonic::transport::Server::builder()
        .add_service(crate::greeter_server::GreeterServer::new(hello_svc));
    let acceptor = tonic_h3::quinn::H3QuinnAcceptor::new(endpoint.clone());

    // run server in background
    let h_sv = tokio::spawn(async move {
        let res = tonic_h3::server::H3Router::from(router)
            .serve_with_shutdown(acceptor, async move { token.cancelled().await })
            .await;
        endpoint.wait_idle().await;
        res
    });
    (h_sv, listen_addr)
}

pub fn run_test_s2n_server(
    in_addr: SocketAddr,
    token: CancellationToken,
) -> (
    tokio::task::JoinHandle<Result<(), tonic_h3::Error>>,
    SocketAddr,
) {
    let tls = s2n_quic::provider::tls::rustls::server::Server::from(make_rustls_server_config());

    let server = s2n_quic::Server::builder()
        .with_tls(tls)
        .unwrap()
        .with_io(in_addr)
        .unwrap()
        .start()
        .unwrap();

    let listen_addr = server.local_addr().unwrap();

    let hello_svc = crate::HelloWorldService {};
    let router = tonic::transport::Server::builder()
        .add_service(crate::greeter_server::GreeterServer::new(hello_svc));
    let acceptor = tonic_h3_s2n::server::H3S2nAcceptor::new(server);

    // run server in background
    let h_sv = tokio::spawn(async move {
        tonic_h3::server::H3Router::from(router)
            .serve_with_shutdown(acceptor, async move { token.cancelled().await })
            .await
    });
    (h_sv, listen_addr)
}

// copied from https://github.com/rustls/rustls/blob/f98484bdbd57a57bafdd459db594e21c531f1b4a/examples/src/bin/tlsclient-mio.rs#L331
mod danger {
    use rustls::client::danger::HandshakeSignatureValid;
    use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::DigitallySignedStruct;

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

pub fn make_danger_rustls_client_config() -> rustls::ClientConfig {
    let mut tls_config = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .dangerous() // Do not verify server certs
    .with_custom_certificate_verifier(Arc::new(crate::danger::NoCertificateVerification::new(
        rustls::crypto::ring::default_provider(),
    )))
    .with_no_client_auth();

    tls_config.enable_early_data = true;
    tls_config.alpn_protocols = vec![b"h3".to_vec()];
    tls_config
}

pub fn make_rustls_server_config() -> rustls::ServerConfig {
    let (cert, key) = crate::make_test_cert_rustls(vec!["localhost".to_string()]);
    let mut tls_config = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert.clone()], key.clone_key())
    .unwrap();
    tls_config.alpn_protocols = vec![b"h3".to_vec()];
    tls_config.max_early_data_size = u32::MAX;
    tls_config
}

pub fn make_test_quinn_client_endpoint() -> h3_quinn::quinn::Endpoint {
    let tls_config = make_danger_rustls_client_config();
    let mut client_endpoint = h3_quinn::quinn::Endpoint::client("[::]:0".parse().unwrap()).unwrap();
    let client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls_config).unwrap(),
    ));
    client_endpoint.set_default_client_config(client_config);
    client_endpoint
}

pub fn make_test_s2n_client_endpoint() -> s2n_quic::Client {
    let tls_config = make_danger_rustls_client_config();
    let tls = s2n_quic::provider::tls::rustls::Client::from(tls_config);
    s2n_quic::Client::builder()
        .with_tls(tls)
        .unwrap()
        .with_io("0.0.0.0:0")
        .unwrap()
        .start()
        .unwrap()
}

pub fn make_test_msquic_client_parts() -> (
    msquic::core::reg::QRegistration,
    msquic::core::config::QConfiguration,
) {
    let api = msquic::core::api::QApi::default();

    let app_name = std::ffi::CString::new("testapp").unwrap();
    let config = msquic_sys2::RegistrationConfig {
        app_name: app_name.as_ptr(),
        execution_profile: msquic_sys2::EXECUTION_PROFILE_LOW_LATENCY,
    };
    let q_reg = msquic::core::reg::QRegistration::new(&api, &config);

    use msquic::core::buffer::{QBufferVec, QVecBuffer};
    let args: [QVecBuffer; 1] = ["h3".into()];
    let alpn = QBufferVec::from(args.as_slice());

    // create an client
    // open client
    let mut client_settings = msquic_sys2::Settings::new();
    client_settings.set_idle_timeout_ms(30000);
    let client_config =
        msquic::core::config::QConfiguration::new(&q_reg, alpn.as_buffers(), &client_settings);
    {
        let mut cred_config = msquic_sys2::CredentialConfig::new_client();
        cred_config.cred_type = msquic_sys2::CREDENTIAL_TYPE_NONE;
        cred_config.cred_flags = msquic_sys2::CREDENTIAL_FLAG_CLIENT;
        cred_config.cred_flags |= msquic_sys2::CREDENTIAL_FLAG_NO_CERTIFICATE_VALIDATION;
        client_config.load_cred(&cred_config);
    }
    (q_reg, client_config)
}

#[cfg(test)]
mod tonic_h3_tests {
    use std::{net::SocketAddr, time::Duration};
    use tokio_util::sync::CancellationToken;
    use tonic::transport::Uri;

    #[tokio::test]
    async fn h3_quinn_test() {
        h3_test(crate::run_test_quinn_hello_server).await;
    }

    #[tokio::test]
    async fn h3_s2n_test() {
        h3_test(crate::run_test_s2n_server).await;
    }

    // takes in the fn to start the server and then send request to the server.
    #[allow(clippy::type_complexity)]
    async fn h3_test(
        run_server: fn(
            SocketAddr,
            CancellationToken,
        ) -> (
            tokio::task::JoinHandle<Result<(), tonic_h3::Error>>,
            SocketAddr,
        ),
    ) {
        crate::try_setup_tracing();

        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let token = CancellationToken::new();
        let (h_svr, listen_addr) = run_server(addr, token.clone());
        tracing::debug!("listenaddr : {}", listen_addr);

        // send client request
        tokio::time::sleep(Duration::from_secs(1)).await;

        tracing::debug!("connecting quic client.");

        let uri: Uri = format!("https://{}", listen_addr).parse().unwrap();

        let client_endpoint = crate::make_test_quinn_client_endpoint();
        // quinn client test
        {
            // client drop is required to end connection. drive will end after connection end
            let cc = tonic_h3::quinn::H3QuinnConnector::new(
                uri.clone(),
                "localhost".to_string(),
                client_endpoint.clone(),
            );
            let channel = tonic_h3::H3Channel::new(cc, uri.clone());
            let mut client = crate::greeter_client::GreeterClient::new(channel);

            {
                let request = tonic::Request::new(crate::HelloRequest {
                    name: "Tonic".into(),
                });
                let response = client.say_hello(request).await.unwrap();

                tracing::debug!("RESPONSE={:?}", response);
            }
            {
                let request = tonic::Request::new(crate::HelloRequest {
                    name: "Tonic2".into(),
                });
                let response = client.say_hello(request).await.unwrap();

                tracing::debug!("RESPONSE={:?}", response);
            }
        }
        tracing::debug!("client wait idle");
        client_endpoint.wait_idle().await;

        // test s2n client
        let mut s2n_ep = crate::make_test_s2n_client_endpoint();
        {
            let channel = tonic_h3_s2n::client::new_s2n_h3_channel(uri.clone(), s2n_ep.clone());
            let mut client = crate::greeter_client::GreeterClient::new(channel);
            {
                let request = tonic::Request::new(crate::HelloRequest {
                    name: "Tonic-S2n".into(),
                });
                let response = client.say_hello(request).await.unwrap();

                tracing::debug!("RESPONSE={:?}", response);
            }
        }
        s2n_ep.wait_idle().await.unwrap();

        // test msquic client
        // TODO: msquic async rust wrapper has but and this does not work yet.
        if false {
            let (reg, config) = crate::make_test_msquic_client_parts();
            let channel = tonic_h3::H3Channel::new(
                tonic_h3_msquic::client::H3MsQuicConnector::new(config, reg, uri.clone()),
                uri.clone(),
            );
            let mut client = crate::greeter_client::GreeterClient::new(channel);
            {
                let request = tonic::Request::new(crate::HelloRequest {
                    name: "Tonic-MsQuic".into(),
                });
                let response = client.say_hello(request).await.unwrap();

                tracing::debug!("RESPONSE={:?}", response);
            }
        }

        token.cancel();
        h_svr.await.unwrap().unwrap();
    }
}

/// Code to be used in rust docs
#[cfg(test)]
mod doc_example {
    /// type used in docs
    #[derive(Clone)]
    pub struct GreeterServer {}
    impl tonic::codegen::Service<http::Request<tonic::body::BoxBody>> for GreeterServer {
        type Response = http::Response<tonic::body::BoxBody>;

        type Error = std::convert::Infallible;

        type Future = std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
        >;

        fn poll_ready(
            &mut self,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            todo!()
        }

        fn call(&mut self, _req: http::Request<tonic::body::BoxBody>) -> Self::Future {
            todo!()
        }
    }

    impl tonic::server::NamedService for GreeterServer {
        const NAME: &'static str = "Dummy";
    }

    impl GreeterServer {
        pub fn new(_inner: HelloWorldService) -> Self {
            todo!()
        }
    }

    pub struct HelloWorldService {}

    #[allow(dead_code)]
    async fn run_server(endpoint: h3_quinn::quinn::Endpoint) -> Result<(), tonic_h3::Error> {
        let router = tonic::transport::Server::builder()
            .add_service(GreeterServer::new(HelloWorldService {}));
        let acceptor = tonic_h3::quinn::H3QuinnAcceptor::new(endpoint.clone());
        tonic_h3::server::H3Router::from(router)
            .serve(acceptor)
            .await?;
        endpoint.wait_idle().await;
        Ok(())
    }

    #[allow(dead_code)]
    async fn run_client(
        uri: http::Uri,
        client_endpoint: h3_quinn::quinn::Endpoint,
    ) -> Result<(), tonic_h3::Error> {
        let cc = tonic_h3::quinn::H3QuinnConnector::new(
            uri.clone(),
            "localhost".to_string(),
            client_endpoint.clone(),
        );
        let channel = tonic_h3::H3Channel::new(cc, uri.clone());
        let mut client = crate::greeter_client::GreeterClient::new(channel);
        let request = tonic::Request::new(crate::HelloRequest {
            name: "Tonic".into(),
        });
        let response = client.say_hello(request).await?;
        println!("RESPONSE={:?}", response);
        Ok(())
    }
}
