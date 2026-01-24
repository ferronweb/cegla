use http_body_util::BodyExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let server = tokio::net::TcpListener::bind("127.0.0.1:4000").await?;

  loop {
    let (socket, _) = server.accept().await?;

    tokio::spawn(async move {
      let _ = cegla_scgi::server::server_handle_scgi(socket, |request| async move {
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
      })
      .await;
    });
  }
}
