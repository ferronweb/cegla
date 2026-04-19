#![cfg_attr(docsrs, feature(doc_cfg))]

//! A high-level FastCGI implementation for Rust.

pub mod protocol;

// TODO: FastCGI client
//#[cfg(feature = "client")]
//pub mod client;
#[cfg(feature = "server")]
pub mod server;

pub use cegla::CgiEnvironment;
