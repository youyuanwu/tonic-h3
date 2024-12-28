mod client;
pub mod quinn;
pub mod server;
pub use client::{dns_resolve, H3Channel, H3Connector};
use server_body::H3IncomingServer;

mod client_body;
mod connection;
mod server_body;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
