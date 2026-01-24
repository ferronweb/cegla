//! CGI implementation for web applications.

use std::future::Future;

use cegla::{
  server::{convert_cgi_request, convert_from_http_response},
  CgiEnvironment, CgiIncoming,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::io::StreamReader;

/// Handles a CGI request by converting it to an HTTP request, invoking the provided request function,
/// and then converting the HTTP response back to a CGI response.
pub async fn handle_request<I, O, E, F, Fut, B, Err>(
  stdin: I,
  mut stdout: O,
  stderr: E,
  request_fn: F,
) -> Result<(), std::io::Error>
where
  I: AsyncRead + Unpin + 'static,
  O: AsyncWrite + Unpin,
  E: AsyncWrite + Unpin,
  F: FnOnce(http::Request<CgiIncoming<I>>, E) -> Fut,
  Fut: Future<Output = Result<http::Response<B>, Err>>,
  B: http_body::Body,
  B::Data: AsRef<[u8]> + Send + 'static,
  B::Error: Into<std::io::Error>,
  Err: Into<std::io::Error>,
{
  let request = convert_cgi_request(stdin, CgiEnvironment::from_iter(std::env::vars()))?;

  let response = request_fn(request, stderr).await.map_err(|err| err.into())?;

  let response_data = convert_from_http_response(response)?;
  let mut response_reader = StreamReader::new(response_data);
  tokio::io::copy(&mut response_reader, &mut stdout).await?;

  Ok(())
}
