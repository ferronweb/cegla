use bytes::Bytes;
use cegla_scgi::client::{client_handle_scgi_send, CgiBuilder};
use http_body_util::BodyExt;
use tokio::io::AsyncWriteExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let socket = tokio::net::TcpStream::connect("127.0.0.1:4000").await?;

  let request = http::Request::builder()
    .uri("http://example.com")
    .body(http_body_util::Empty::<Bytes>::new().map_err(std::io::Error::other))?;

  let response = client_handle_scgi_send(request, tokio_cegla::TokioScgiRuntime, socket, CgiBuilder::new()).await?;

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
