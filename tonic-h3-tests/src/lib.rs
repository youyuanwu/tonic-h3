#[cfg(test)]
mod h3_tests {
    use std::{sync::Arc, time::Duration};

    use hyper_h3::incoming_conn;
    use tokio_util::sync::CancellationToken;

    fn make_test_cert(subject_alt_names: Vec<String>) -> (rcgen::Certificate, rcgen::KeyPair) {
        use rcgen::{generate_simple_self_signed, CertifiedKey};
        let CertifiedKey { cert, key_pair } =
            generate_simple_self_signed(subject_alt_names).unwrap();
        (cert, key_pair)
    }

    fn make_test_cert_rustls(
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

    #[tokio::test]
    async fn h3_test() {
        try_setup_tracing();

        let (cert, key) = make_test_cert_rustls(vec!["localhost".to_string()]);

        //TODO: client auth?? disable??

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
        let tls_config = Arc::new(tls_config);

        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

        let server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(tls_config).unwrap(),
        ));
        let endpoint = quinn::Endpoint::server(server_config, addr).unwrap();

        let listen_addr = endpoint.local_addr().unwrap();
        tracing::debug!("listenaddr : {}", listen_addr);

        let hello_svc = tonic_shared::HelloWorldService {};
        let svc = tonic::service::Routes::new(tonic_shared::greeter_server::GreeterServer::new(
            hello_svc,
        ));
        let token = CancellationToken::new();

        let reqs = hyper_h3::incoming_req(incoming_conn(endpoint.clone()));

        // run server in background
        let token_cp = token.clone();
        let h_sv = tokio::spawn(async move {
            hyper_h3::serve_tonic(svc, reqs, async move { token_cp.cancelled().await }).await
        });

        // send client request
        tokio::time::sleep(Duration::from_secs(1)).await;

        let mut root_store = rustls::RootCertStore::empty();
        root_store.add(cert).unwrap();
        let mut tls_config = rustls::ClientConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(root_store)
        .with_no_client_auth();

        tls_config.enable_early_data = true;
        tls_config.alpn_protocols = vec![b"h3".to_vec()];

        let mut client_endpoint =
            h3_quinn::quinn::Endpoint::client("[::]:0".parse().unwrap()).unwrap();

        let client_config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(tls_config).unwrap(),
        ));
        client_endpoint.set_default_client_config(client_config);

        tracing::debug!("connecting quic client.");
        let conn = client_endpoint
            .connect(listen_addr, "localhost")
            .unwrap()
            .await
            .unwrap();

        tracing::debug!("QUIC connection established");

        let uri = format!("https://{}", listen_addr).parse().unwrap();
        let channel = hyper_h3::channel_h3(conn, uri);

        let mut client = tonic_shared::greeter_client::GreeterClient::new(channel);

        let request = tonic::Request::new(tonic_shared::HelloRequest {
            name: "Tonic".into(),
        });
        let response = client.say_hello(request).await.unwrap();

        tracing::debug!("RESPONSE={:?}", response);

        std::mem::drop(client);

        token.cancel();
        h_sv.await.unwrap().unwrap();
        endpoint.wait_idle().await;
    }
}
