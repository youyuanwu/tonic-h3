# tonic-h3
[![build](https://github.com/youyuanwu/msquic-h3/actions/workflows/build.yaml/badge.svg)](https://github.com/youyuanwu/msquic-h3/actions/workflows/build.yaml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://raw.githubusercontent.com/youyuanwu/tonic-h3/main/LICENSE)
[![Crates.io](https://img.shields.io/crates/v/tonic-h3)](https://crates.io/crates/tonic-h3)
[![Documentation](https://docs.rs/tonic-h3/badge.svg)](https://docs.rs/tonic-h3)
[![codecov](https://codecov.io/github/youyuanwu/tonic-h3/graph/badge.svg?token=JUVKWRHNCO)](https://codecov.io/github/youyuanwu/tonic-h3)

Experimental implementation of running [tonic](https://github.com/hyperium/tonic) grpc on [h3](https://github.com/hyperium/h3), following the proposal: [G2-http3-protocol](https://github.com/grpc/proposal/blob/master/G2-http3-protocol.md)

tonic-h3 is targeted to support all quic transport implementations that integrates with h3. Currently the following are incoporated and tested:
* [Quinn](https://github.com/quinn-rs/quinn) ([h3-quinn](https://github.com/hyperium/h3/h3-quinn/))
* [s2n-quic](https://github.com/aws/s2n-quic) ([s2n-quic-h3](https://github.com/aws/s2n-quic/tree/main/quic/s2n-quic-h3))
* [msquic](https://github.com/microsoft/msquic) ([msquic-h3](https://github.com/youyuanwu/msquic-h3)).

See [examples](./tonic-h3-tests/examples/) and [tests](./tonic-h3-tests/src/lib.rs) for getting started.

Compatibility with grpc-dotnet with http3 is tested [here](./dotnet/).

## Supporting crates
* [axum-h3](./axum-h3/) Use [axum](https://github.com/tokio-rs/axum) with [h3](https://github.com/hyperium/h3), used by `tonic-h3` server.
* [h3-util](./h3-util/) Http3 server and client utilities used by `axum-h3` and `tonic-h3`.
* Other quic wrapping crates to support alternative quic implementations.

## Get started
Add deps to your cargo.toml
```toml
tonic-h3 = { version="*" , default-features = false, features = ["quinn"] }
```

## tonic examples
Server:
```rs
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
```
Client:
```rs
  async fn run_client(
      uri: http::Uri,
      client_endpoint: h3_quinn::quinn::Endpoint,
  ) -> Result<(), tonic_h3::Error> {
      let channel = tonic_h3::quinn::new_quinn_h3_channel(uri.clone(), client_endpoint.clone());
      let mut client = crate::greeter_client::GreeterClient::new(channel);
      let request = tonic::Request::new(crate::HelloRequest {
          name: "Tonic".into(),
      });
      let response = client.say_hello(request).await?;
      println!("RESPONSE={:?}", response);
      Ok(())
  }
```

## License

MIT license. See [LICENSE](LICENSE).