# Changelog

## `cegla` 0.2.0 - March 22, 2026

- Added `cwd` argument to `start_child` function of `Runtime` trait in `cegla-cgi`
- The `CONTENT_LENGTH` CGI variable is now overridden if it's present before building a `CgiEnvironment`
- Improved HTTP header environment variable handling consistency between client-side and server-side implementations

## `cegla` 0.1.2 - January 24, 2026

- Added `cegla-scgi` (a high-level SCGI implementation for Rust)
- Disabled `cgi-client` feature of `tokio-cegla` by default

## `cegla` 0.1.1 - January 24, 2026

- Added `cegla-cgi` (a high-level CGI implementation for Rust)
- Added `tokio-cegla` (a Tokio-based runtime support for `cegla-*` crates)

## `cegla` 0.1.0 - January 23, 2026

- First release
