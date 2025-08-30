pub mod client;
pub mod server;

pub use s2n_quic;

/// internal copy of the unpublished s2n-quic-h3 crate.
#[cfg(feature = "s2n-quic")]
pub mod s2n_quic_h3;
