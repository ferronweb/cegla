use std::process::Stdio;

use bytes::Bytes;
use cegla::client::{convert_to_http_response, CgiBuilder};
use http_body_util::BodyExt;
use tokio::io::AsyncWriteExt;
use tokio_util::io::StreamReader;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let program_to_execute = std::env::args()
    .nth(1)
    .ok_or(std::io::Error::other("No CGI program to execute provided"))?;

  let request = http::Request::builder()
    .uri("http://example.com")
    .body(http_body_util::Empty::<Bytes>::new().map_err(std::io::Error::other))?;

  let (cgi_env, cgi_data) = CgiBuilder::new().request_uri(request.uri()).build(request);

  let mut child = tokio::process::Command::new(program_to_execute)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::inherit())
    .envs(cgi_env)
    .spawn()?;
  let mut stdin = child
    .stdin
    .take()
    .ok_or(std::io::Error::other("Failed to take stdin"))?;
  let stdout = child
    .stdout
    .take()
    .ok_or(std::io::Error::other("Failed to take stdout"))?;

  let mut cgi_data_reader = StreamReader::new(cgi_data);
  tokio::spawn(async move {
    let _ = tokio::io::copy(&mut cgi_data_reader, &mut stdin).await;
  });

  let response = convert_to_http_response(stdout).await?;

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
