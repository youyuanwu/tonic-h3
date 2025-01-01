pub mod client;
pub mod client_body;
mod connection;
pub mod quinn;
pub mod server;
pub mod server_body;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub use std::error::Error as StdError;
