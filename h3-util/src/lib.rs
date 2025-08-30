pub mod client;
pub mod client_body;
mod client_conn;
pub mod executor;
#[cfg(feature = "msquic")]
pub mod msquic;
#[cfg(feature = "quinn")]
pub mod quinn;
pub mod server;
pub mod server_body;

/// s2n backend
#[cfg(feature = "s2n-quic")]
pub mod s2n;

/// internal copy of the unpublished s2n-quic-h3 crate.
#[cfg(feature = "s2n-quic")]
pub(crate) mod s2n_quic_h3;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub use std::error::Error as StdError;
