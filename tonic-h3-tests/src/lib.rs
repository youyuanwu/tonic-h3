use std::{net::SocketAddr, sync::Arc};

use h3_util::msquic::msquic_h3::msquic;
use h3_util::quinn::h3_quinn::quinn::{self};
use h3_util::{client::H3Connector, s2n::s2n_quic, server::H3Acceptor};
use http::Uri;
use tokio_util::sync::CancellationToken;

// llvm-cov with external exe messes up the build cache.
// So we do not run the cross exe tests.
#[cfg(test)]
#[cfg(target_os = "windows")]
#[cfg(not(feature = "llvm-cov-mode"))]
mod dotnet;

#[cfg(test)]
mod axum;

#[cfg(test)]
mod reconnect;

#[cfg(test)]
mod mix;

#[cfg(test)]
mod quiche;

#[cfg(test)]
mod gm_quic;

pub mod cert_gen;

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
            message: format!("hello {name}"),
        }))
    }
}

pub fn make_test_cert_rustls(
    subject_alt_names: Vec<String>,
) -> (
    rustls::pki_types::CertificateDer<'static>,
    rustls::pki_types::PrivateKeyDer<'static>,
) {
    let (cert, key_pair) = cert_gen::make_test_cert(subject_alt_names);
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
    // Install rustls crypto provider for gm-quic compatibility
    // (gm-quic uses rustls 0.23+ which requires explicit provider setup)
    let _ = rustls::crypto::ring::default_provider().install_default();
    
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

pub fn run_test_server(
    acceptor: impl H3Acceptor + Send + 'static,
    token: CancellationToken,
) -> tokio::task::JoinHandle<Result<(), tonic_h3::Error>> {
    let hello_svc = crate::HelloWorldService {};
    let router = tonic::service::Routes::builder()
        .add_service(crate::greeter_server::GreeterServer::new(hello_svc))
        .clone()
        .routes();

    // run server in background
    tokio::spawn(async move {
        tonic_h3::server::H3Router::new(router)
            .serve_with_shutdown(acceptor, async move { token.cancelled().await })
            .await
    })
}

// returns handle and listening addr
pub fn run_test_quinn_hello_server(
    in_addr: SocketAddr,
    token: CancellationToken,
) -> (tokio::task::JoinHandle<()>, SocketAddr) {
    let endpoint = make_quinn_server_endpoint(in_addr);
    let listen_addr = endpoint.local_addr().unwrap();
    tracing::debug!("listenaddr : {}", listen_addr);
    let acceptor = tonic_h3::quinn::H3QuinnAcceptor::new(endpoint.clone());
    let h_sv = run_test_server(acceptor, token);

    let h = tokio::spawn(async move {
        h_sv.await
            .expect("cannot join")
            .expect("tonic server failed");
        endpoint.close(0_u16.into(), b"svr shutdown");
        endpoint.wait_idle().await;
        tracing::debug!("test server ended")
    });

    (h, listen_addr)
}

pub fn run_test_s2n_server(
    in_addr: SocketAddr,
    token: CancellationToken,
) -> (tokio::task::JoinHandle<()>, SocketAddr) {
    let tls = h3_util::s2n::s2n_quic::provider::tls::rustls::server::Server::from(
        make_rustls_server_config(),
    );
    let server = h3_util::s2n::s2n_quic::Server::builder()
        .with_tls(tls)
        .unwrap()
        .with_io(in_addr)
        .unwrap()
        .start()
        .unwrap();
    let listen_addr = server.local_addr().unwrap();
    let acceptor = h3_util::s2n::server::H3S2nAcceptor::new(server);
    let h_sv = run_test_server(acceptor, token);

    let h = tokio::spawn(async move {
        h_sv.await
            .expect("cannot join")
            .expect("tonic server failed");
        tracing::debug!("test server ended");
        // s2n does not support close so wait a bit to let server release listening port.
        // tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        // This does not work.
    });

    (h, listen_addr)
}

// #[cfg(target_os = "windows")]
pub mod msquic_util {
    use std::net::SocketAddr;

    use h3_util::msquic::msquic_h3::{
        Listener,
        msquic::{
            self, BufferRef, Configuration, Credential, CredentialConfig, CredentialFlags,
            Registration, RegistrationConfig, Settings,
        },
    };
    use h3_util::msquic::server::H3MsQuicAcceptor;
    use http::Uri;
    use tokio_util::sync::CancellationToken;

    /// Use pwsh to get the test cert hash
    #[cfg(target_os = "windows")]
    pub fn get_test_cred() -> Credential {
        use h3_util::msquic::msquic_h3::msquic::CertificateHash;
        fn get_hash() -> Option<String> {
            let get_cert_cmd = "Get-ChildItem Cert:\\CurrentUser\\My | Where-Object -Property FriendlyName -EQ -Value MsQuic-Test | Select-Object -ExpandProperty Thumbprint -First 1";
            let output = std::process::Command::new("pwsh.exe")
                .args(["-Command", get_cert_cmd])
                .output()
                .expect("Failed to execute command");
            assert!(output.status.success());
            let mut s = String::from_utf8(output.stdout).unwrap();
            if s.ends_with('\n') {
                s.pop();
                if s.ends_with('\r') {
                    s.pop();
                }
            };
            if s.is_empty() { None } else { Some(s) }
        }
        fn gen_cert() {
            let gen_cert_cmd = "New-SelfSignedCertificate -DnsName $env:computername,localhost -FriendlyName MsQuic-Test -KeyUsageProperty Sign -KeyUsage DigitalSignature -CertStoreLocation cert:\\CurrentUser\\My -HashAlgorithm SHA256 -Provider \"Microsoft Software Key Storage Provider\" -KeyExportPolicy Exportable";
            let output = std::process::Command::new("pwsh.exe")
                .args(["-Command", gen_cert_cmd])
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .output()
                .expect("Failed to execute command");
            assert!(output.status.success());
        }
        // generate the cert if not exist
        let s = match get_hash() {
            Some(s) => s,
            None => {
                gen_cert();
                get_hash().unwrap()
            }
        };
        Credential::CertificateHash(CertificateHash::from_str(&s).unwrap())
    }

    #[cfg(not(target_os = "windows"))]
    pub fn get_test_cred() -> Credential {
        use msquic::CertificateFile;

        let (cert_path, key_path) = crate::cert_gen::make_test_cert_files("msquic", false);
        Credential::CertificateFile(CertificateFile::new(
            key_path.display().to_string(),
            cert_path.display().to_string(),
        ))
    }

    pub fn run_test_msquic_server(
        in_addr: SocketAddr,
        token: CancellationToken,
    ) -> (tokio::task::JoinHandle<()>, SocketAddr) {
        let cred = get_test_cred();
        let alpn = [BufferRef::from("h3")];
        let settings = Settings::new()
            .set_PeerBidiStreamCount(10)
            .set_PeerUnidiStreamCount(10)
            .set_ServerResumptionLevel(msquic::ServerResumptionLevel::ResumeAndZerortt) // TODO:
            .set_IdleTimeoutMs(1000);

        let app_name = String::from("testapp_server");
        let reg = Registration::new(&RegistrationConfig::default().set_app_name(app_name)).unwrap();
        let config = Configuration::open(&reg, &alpn, Some(&settings)).unwrap();

        let cred_config = CredentialConfig::new()
            .set_credential_flags(CredentialFlags::NO_CERTIFICATE_VALIDATION)
            .set_credential(cred);
        config.load_credential(&cred_config).unwrap();

        let config = std::sync::Arc::new(config);
        let config_cp = config.clone();
        let max_retry = 30;
        let mut i = 0;
        let l = loop {
            match Listener::new(&reg, config.clone(), &alpn, Some(in_addr)) {
                Ok(l) => break l,
                Err(e) => {
                    if i < max_retry
                        && e.try_as_status_code().unwrap()
                            == msquic::StatusCode::QUIC_STATUS_ADDRESS_IN_USE
                    {
                        std::thread::yield_now();
                    } else {
                        panic!("cannot open server {e}")
                    }
                }
            }
            i += 1;
        };
        let local_addr = l.get_ref().get_local_addr().unwrap().as_socket().unwrap();
        let acceptor = H3MsQuicAcceptor::new(l);
        let acceptor_cp = acceptor.clone();

        let h_sv = super::run_test_server(acceptor, token);

        let h = tokio::spawn(async move {
            h_sv.await
                .expect("cannot join")
                .expect("tonic server failed");
            // This is required for msquic to clean up.
            acceptor_cp.shutdown().await;
            // config is closed after listener
            std::mem::drop(acceptor_cp);
            std::mem::drop(config_cp);
            let th = std::thread::spawn(move || {
                std::mem::drop(reg);
            });
            while !th.is_finished() {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            tracing::debug!("test server ended")
        });

        (h, local_addr)
    }

    pub fn run_msquic_client(
        uri: Uri,
        token: CancellationToken,
    ) -> (
        tokio::task::JoinHandle<()>,
        impl h3_util::client::H3Connector,
    ) {
        let (reg, config) = crate::make_test_msquic_client_parts();

        let msquic_waiter = h3_util::msquic::client::H3MsQuicClientWaiter::default();
        let cc = h3_util::msquic::client::H3MsQuicConnector::new(
            config,
            reg.clone(),
            uri.clone(),
            msquic_waiter.clone(),
        );

        let h = tokio::spawn(async move {
            token.cancelled().await;
            tracing::debug!("client cancel received. Shutting down registration.");
            // signify to close all connections on this registration.
            reg.shutdown();
            tracing::debug!("client registration shutdown completed.");
            msquic_waiter.wait_shutdown().await;
            // If connections are not yet closed this will stuck.
            std::mem::drop(reg);
            tracing::debug!("client registration dropped.");
        });
        (h, cc)
    }
}

// copied from https://github.com/rustls/rustls/blob/f98484bdbd57a57bafdd459db594e21c531f1b4a/examples/src/bin/tlsclient-mio.rs#L331
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

pub fn make_test_quinn_client_endpoint() -> quinn::Endpoint {
    let tls_config = make_danger_rustls_client_config();
    let mut client_endpoint = quinn::Endpoint::client("[::]:0".parse().unwrap()).unwrap();
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

pub fn make_test_msquic_client_parts() -> (Arc<msquic::Registration>, Arc<msquic::Configuration>) {
    let app_name = String::from("testapp_client");
    let config = msquic::RegistrationConfig::new().set_app_name(app_name);
    let reg = msquic::Registration::new(&config).unwrap();

    let alpn = msquic::BufferRef::from("h3");
    // create an client
    // open client. Allow peer open streams: h3 server opens stream to send resp back.
    let client_settings = msquic::Settings::new()
        .set_IdleTimeoutMs(1000)
        .set_PeerBidiStreamCount(10)
        .set_PeerUnidiStreamCount(10);
    let client_config = msquic::Configuration::open(&reg, &[alpn], Some(&client_settings)).unwrap();
    {
        let cred_config = msquic::CredentialConfig::new_client()
            .set_credential_flags(msquic::CredentialFlags::NO_CERTIFICATE_VALIDATION);
        client_config.load_credential(&cred_config).unwrap();
    }
    (reg.into(), client_config.into())
}

/// run client in background
/// token cancel will close the endpoint.
pub fn run_quinn_client(
    uri: Uri,
    token: CancellationToken,
) -> (tokio::task::JoinHandle<()>, impl H3Connector) {
    let client_endpoint = crate::make_test_quinn_client_endpoint();
    let cc = tonic_h3::quinn::H3QuinnConnector::new(
        uri,
        "localhost".to_string(),
        client_endpoint.clone(),
    );
    let h = tokio::spawn(async move {
        token.cancelled().await;
        tracing::debug!("client canancl and close");
        client_endpoint.close(0_u16.into(), b"client close");
    });
    (h, cc)
}

pub fn run_s2n_client(
    uri: Uri,
    token: CancellationToken,
) -> (tokio::task::JoinHandle<()>, impl H3Connector) {
    let mut s2n_ep = crate::make_test_s2n_client_endpoint();

    let cc = h3_util::s2n::client::H3S2nConnector::new(
        uri.clone(),
        uri.host().unwrap().to_string(),
        s2n_ep.clone(),
    );

    let h = tokio::spawn(async move {
        token.cancelled().await;
        // s2n does not support close.
        tracing::debug!("client endpoint canancl");
        s2n_ep.wait_idle().await.unwrap();
    });
    (h, cc)
}

pub mod gm_quic_util {
    use std::net::SocketAddr;

    use gm_quic::prelude::{QuicClient, QuicListeners};
    use h3_util::gm_quic::server::H3GmQuicAcceptor;
    use tokio_util::sync::CancellationToken;

    pub fn make_test_gm_quic_client() -> QuicClient {
        // Create a gm-quic client with no certificate verification (for testing)
        // Must set ALPN to "h3" for HTTP/3 protocol negotiation
        QuicClient::builder()
            .without_verifier()
            .without_cert()
            .with_alpns(["h3"])
            .build()
    }

    pub fn run_test_gm_quic_server(
        in_addr: SocketAddr,
        token: CancellationToken,
    ) -> (tokio::task::JoinHandle<()>, SocketAddr) {
        use gm_quic::prelude::QuicIO;

        // Generate test certificates
        let (cert_path, key_path) = crate::cert_gen::make_test_cert_files("gm_quic", false);

        // Create gm-quic server listeners
        // 1. builder() returns Result
        // 2. without_client_cert_verifier() configures no client auth
        // 3. with_alpns() sets the ALPN protocols
        // 4. listen(backlog) creates the listeners (returns Arc<QuicListeners>)
        let listeners = QuicListeners::builder()
            .expect("Failed to create QuicListeners builder")
            .without_client_cert_verifier()
            .with_alpns(["h3"])
            .listen(8); // backlog size

        // Add a server with certificate (virtual host style)
        // BindUri accepts &str, ToCertificate/ToPrivateKey accept &Path
        listeners
            .add_server(
                "localhost",
                cert_path.as_path(),
                key_path.as_path(),
                [in_addr], // SocketAddr implements Into<BindUri>
                None::<Vec<u8>>,
            )
            .expect("Failed to add server for localhost");

        // Get the actual bound address from the server's interface
        let listen_addr = {
            let server = listeners.get_server("localhost").expect("Server not found");
            let bind_interfaces = server.bind_interfaces();
            let (_, bind_iface) = bind_interfaces.iter().next().expect("No bound interface");
            let real_addr = bind_iface
                .borrow()
                .expect("Failed to borrow interface")
                .real_addr()
                .expect("Failed to get real address");
            match real_addr {
                gm_quic::prelude::RealAddr::Internet(addr) => addr,
                _ => panic!("Expected internet address"),
            }
        };

        // listen() already returns Arc<QuicListeners>
        let acceptor = H3GmQuicAcceptor::new(listeners);
        let acceptor_cp = acceptor.clone();

        let h_sv = super::run_test_server(acceptor, token);

        let h = tokio::spawn(async move {
            h_sv.await
                .expect("cannot join")
                .expect("tonic server failed");
            // Shutdown the gm-quic acceptor to release global resources
            acceptor_cp.shutdown().await;
            tracing::debug!("gm-quic test server ended");
        });

        (h, listen_addr)
    }
}

/// Code to be used in rust docs
#[cfg(test)]
mod doc_example {
    use http::Uri;

    /// type used in docs
    #[derive(Clone)]
    pub struct GreeterServer {}
    impl tonic::codegen::Service<http::Request<tonic::body::Body>> for GreeterServer {
        type Response = http::Response<tonic::body::Body>;

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

        fn call(&mut self, _req: http::Request<tonic::body::Body>) -> Self::Future {
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
    async fn run_server(endpoint: quinn::Endpoint) -> Result<(), tonic_h3::Error> {
        let router = tonic::service::Routes::builder()
            .add_service(GreeterServer::new(HelloWorldService {}))
            .clone()
            .routes();
        let acceptor = tonic_h3::quinn::H3QuinnAcceptor::new(endpoint.clone());
        tonic_h3::server::H3Router::new(router)
            .serve(acceptor)
            .await?;
        endpoint.wait_idle().await;
        Ok(())
    }

    #[allow(dead_code)]
    async fn run_client(uri: Uri, client_endpoint: quinn::Endpoint) -> Result<(), tonic_h3::Error> {
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
        println!("RESPONSE={response:?}");
        Ok(())
    }
}
