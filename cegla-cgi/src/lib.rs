//! A high-level CGI implementation for Rust.

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "server")]
pub mod server;

pub use cegla::CgiEnvironment;
