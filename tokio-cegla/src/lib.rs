#![cfg_attr(docsrs, feature(doc_cfg))]

//! Tokio-based runtime support for `cegla-*` crates.

#[cfg(feature = "cgi-client")]
mod cgi;
#[cfg(feature = "cgi-client")]
pub use cgi::*;

#[cfg(feature = "scgi-client")]
mod scgi;
#[cfg(feature = "scgi-client")]
pub use scgi::*;
