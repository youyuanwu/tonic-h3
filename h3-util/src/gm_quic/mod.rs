//! gm-quic backend for h3-util using h3-shim
//!
//! This module provides integration with the gm-quic QUIC implementation
//! through the h3-shim crate.
//!
//! # Example
//!
//! ## Client
//! ```ignore
//! use h3_util::gm_quic::{H3GmQuicConnector, gm_quic::prelude::*};
//! use std::sync::Arc;
//!
//! // Create a gm-quic client
//! let client = QuicClient::builder()
//!     .without_verifier()
//!     .without_cert()
//!     .build();
//!
//! // Connect to a server
//! let connection = client.connect("example.com:443").await?;
//!
//! // Create the H3 connector
//! let connector = H3GmQuicConnector::new(
//!     "https://example.com".parse()?,
//!     "example.com".to_string(),
//!     Arc::new(connection),
//! );
//! ```
//!
//! ## Server
//! ```ignore
//! use h3_util::gm_quic::{H3GmQuicAcceptor, gm_quic::prelude::*};
//! use std::sync::Arc;
//!
//! // Create gm-quic listeners
//! let listeners = QuicListeners::builder()
//!     .with_certificate_file("cert.pem", "key.pem")?
//!     .listen("0.0.0.0:443")?;
//!
//! // Create the H3 acceptor
//! let mut acceptor = H3GmQuicAcceptor::new(Arc::new(listeners));
//!
//! // Accept incoming connections
//! while let Ok(Some(conn)) = acceptor.accept().await {
//!     // Handle connection
//! }
//! ```

pub mod client;
pub mod server;

pub use client::H3GmQuicConnector;
pub use server::H3GmQuicAcceptor;

// Re-export the underlying crates for convenience
pub use gm_quic;
pub use h3_shim;
