// h3 wrapper for quiche (backed by the local `quiche-h3` bridge crate).

mod client;
pub use client::H3QuicheConnector;
mod server;
pub use server::H3QuicheAcceptor;

// Re-export the non-colliding config types for convenience. The bridge crate's
// own `H3QuicheConnector`/`H3QuicheAcceptor` are intentionally NOT glob
// re-exported here because they share names with our wrappers above; reach them
// via the `quiche_h3` module below if needed.
pub use quiche_h3::{H3QuicheClientConfig, H3QuicheServerConfig};

// Re-export the underlying bridge crate so callers can reach `tokio_quiche` /
// `quiche` and other items through this module, mirroring how the quinn backend
// re-exports `h3_quinn`.
pub use quiche_h3;
