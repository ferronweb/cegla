use std::ffi::OsStr;

use bytes::Bytes;
use cegla_cgi::client::{execute_cgi_send, CgiBuilder};
use http_body_util::BodyExt;
use tokio::io::AsyncWriteExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let program_to_execute = std::env::args()
    .nth(1)
    .ok_or(std::io::Error::other("No CGI program to execute provided"))?;

  let request = http::Request::builder()
    .uri("http://example.com")
    .body(http_body_util::Empty::<Bytes>::new().map_err(std::io::Error::other))?;

  let (response, stderr, status) = execute_cgi_send(
    request,
    tokio_cegla::TokioCgiRuntime,
    OsStr::new(&program_to_execute),
    &[],
    CgiBuilder::new(),
    None,
  )
  .await?;

  if let Some(status) = status {
    if let Some(mut stderr) = stderr {
      tokio::io::copy(&mut stderr, &mut tokio::io::stderr()).await?;
    }
    if !status.success() {
      return Err(std::io::Error::other(format!("CGI program exited with {}", status)).into());
    }
  }

  let (parts, mut body) = response.into_parts();
  if !parts.status.is_success() {
    return Err(std::io::Error::other(format!("Non-2xx status code: {}", parts.status)).into());
  }

  let mut main_stdout = tokio::io::stdout();
  while let Some(chunk) = body.frame().await {
    if let Ok(data) = chunk?.into_data() {
      main_stdout.write_all(&data).await?;
    }
  }

  Ok(())
}
