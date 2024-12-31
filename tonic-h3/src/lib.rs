//! gRPC over HTTP/3 for Rust.
//! # Examples
//! Server example:
//! ```ignore
//! async fn run_server(endpoint: h3_quinn::quinn::Endpoint) -> Result<(), tonic_h3::Error> {
//!     let router = tonic::transport::Server::builder()
//!         .add_service(GreeterServer::new(HelloWorldService {}));
//!     let acceptor = tonic_h3::quinn::H3QuinnAcceptor::new(endpoint.clone());
//!     tonic_h3::server::H3Router::from(router)
//!         .serve(acceptor)
//!         .await?;
//!     endpoint.wait_idle().await;
//!     Ok(())
//! }
//! ```
//! Client example:
//! ```ignore
//! async fn run_client(
//!   uri: http::Uri,
//!   client_endpoint: quinn::Endpoint,
//! ) -> Result<(), tonic_h3::Error> {
//!   let channel = tonic_h3::quinn::new_quinn_h3_channel(uri.clone(), client_endpoint.clone());
//!   let mut client = crate::greeter_client::GreeterClient::new(channel);
//!   let request = tonic::Request::new(crate::HelloRequest {
//!       name: "Tonic".into(),
//!   });
//!   let response = client.say_hello(request).await?;
//!   println!("RESPONSE={:?}", response);
//!   Ok(())
//! }
//!```

mod client;
pub mod server;
pub use client::H3Channel;

mod client_body;
mod connection;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
