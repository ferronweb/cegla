//! Tokio-based runtime support for `cegla-*` crates.

#[cfg(feature = "cgi-client")]
mod cgi;
#[cfg(feature = "cgi-client")]
pub use cgi::*;
