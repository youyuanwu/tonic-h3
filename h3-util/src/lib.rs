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

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub use std::error::Error as StdError;
