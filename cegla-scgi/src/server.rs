//! Server-side SCGI implementation.

use std::future::Future;

use cegla::{
  server::{convert_cgi_request, convert_from_http_response},
  CgiEnvironment, CgiIncoming,
};
use hashlink::LinkedHashMap;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, BufReader, ReadHalf};
use tokio_util::io::StreamReader;

/// Handles a SCGI request by converting it to an HTTP request, invoking the provided request function,
/// and then converting the HTTP response back to a SCGI response.
pub async fn server_handle_scgi<Io, F, Fut, B, Err>(io: Io, request_fn: F) -> Result<(), std::io::Error>
where
  Io: AsyncRead + AsyncWrite + Send + Unpin + 'static,
  F: FnOnce(http::Request<CgiIncoming<BufReader<ReadHalf<Io>>>>) -> Fut,
  Fut: Future<Output = Result<http::Response<B>, Err>>,
  B: http_body::Body,
  B::Data: AsRef<[u8]> + Send + 'static,
  B::Error: Into<std::io::Error>,
  Err: Into<std::io::Error>,
{
  let (read_half, mut write_half) = tokio::io::split(io);
  let mut read_half = BufReader::new(read_half);

  let mut length_buf = Vec::new();
  read_half.read_until(b':', &mut length_buf).await?;
  length_buf.pop(); // Remove ':'
  let environment_length = String::from_utf8(length_buf)
    .ok()
    .and_then(|s| s.parse::<usize>().ok())
    .ok_or(std::io::Error::new(
      std::io::ErrorKind::InvalidData,
      "Invalid environment length",
    ))?;

  let mut environment = vec![0u8; environment_length];
  read_half.read_exact(&mut environment).await?;
  let _ = read_half.read_u8().await?; // Discard ','

  let mut split_environment = environment.split(|b| *b == 0);
  let mut environment = LinkedHashMap::new();
  while let Some(key) = split_environment.next() {
    if let Some(value) = split_environment.next() {
      environment.insert(
        String::from_utf8(key.to_vec())
          .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid environment key"))?,
        String::from_utf8(value.to_vec())
          .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid environment value"))?,
      );
    }
  }

  let environment = CgiEnvironment::from(environment);
  let request = convert_cgi_request(read_half, environment)?;

  let response = request_fn(request).await.map_err(|err| err.into())?;

  let response_data = convert_from_http_response(response)?;
  let mut response_reader = StreamReader::new(response_data);
  tokio::io::copy(&mut response_reader, &mut write_half).await?;

  Ok(())
}
