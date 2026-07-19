//! HTTP/3 server and client utilities used by [`tonic-h3`] and [`axum-h3`].
//!
//! This crate abstracts QUIC transports behind two traits so that the same
//! HTTP/3 server and client code can run over any supported backend:
//!
//! - [`client::H3Connector`] — establishes client-side QUIC connections.
//! - [`server::H3Acceptor`] — accepts server-side QUIC connections.
//!
//! On the client side, [`client::H3Channel`] wraps an [`client::H3Connector`]
//! into a [`tower::Service`]/`tonic`-compatible channel that transparently
//! (re)connects when the underlying HTTP/3 driver ends.
//!
//! # Backends
//!
//! Each backend is gated behind a cargo feature and exposes its own
//! `H3Connector`/`H3Acceptor` implementations:
//!
//! | Feature | Backend | Module | Support |
//! |---------|---------|--------|---------|
//! | `quinn` | [Quinn](https://github.com/quinn-rs/quinn) | `quinn` | **production** |
//! | `msquic` | [MsQuic](https://github.com/youyuanwu/msquic-h3) | `msquic` | experimental |
//! | `s2n-quic` | [s2n-quic](https://github.com/aws/s2n-quic) | `s2n` | experimental |
//! | `quiche` | [quiche](https://github.com/cloudflare/quiche) | `quiche_h3` | experimental |
//!
//! No backend is enabled by default; enable the one you need, e.g.
//! `features = ["quinn"]`.
//!
//! **Only the `quinn` backend is supported for production use.** The `msquic`,
//! `s2n-quic`, and `quiche` backends are experimental and provided for
//! evaluation only. They have known limitations — for example, they do not
//! release the listening UDP socket promptly on server shutdown.
//!
//! [`tonic-h3`]: https://github.com/youyuanwu/tonic-h3
//! [`axum-h3`]: https://crates.io/crates/axum-h3
//! [`tower::Service`]: https://docs.rs/tower/latest/tower/trait.Service.html

pub mod client;
pub mod client_body;
mod client_conn;
pub mod executor;
#[cfg(feature = "msquic")]
pub mod msquic;
#[cfg(feature = "quinn")]
pub mod quinn;
mod send_guard;
pub mod server;
pub mod server_body;

/// s2n backend
#[cfg(feature = "s2n-quic")]
pub mod s2n;

#[cfg(feature = "quiche")]
pub mod quiche_h3;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub use std::error::Error as StdError;
