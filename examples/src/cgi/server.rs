use http_body_util::BodyExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  cegla_cgi::server::handle_request(
    tokio::io::stdin(),
    tokio::io::stdout(),
    tokio::io::stderr(),
    |request, _stderr| async move {
      http::Response::builder()
        .status(200)
        .body(
          http_body_util::Full::new(bytes::Bytes::from(format!(
            "Hello World! Request path: {}",
            request.uri().path()
          )))
          .map_err(std::io::Error::other),
        )
        .map_err(std::io::Error::other)
    },
  )
  .await?;
  Ok(())
}
