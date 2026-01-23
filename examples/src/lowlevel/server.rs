use cegla::{
  server::{convert_cgi_request, convert_from_http_response},
  CgiEnvironment,
};
use http_body_util::BodyExt;
use tokio_util::io::StreamReader;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let request = convert_cgi_request(tokio::io::stdin(), CgiEnvironment::from_iter(std::env::vars()))?;

  let response = http::Response::builder().status(200).body(
    http_body_util::Full::new(bytes::Bytes::from(format!(
      "Hello World! Request path: {}",
      request.uri().path()
    )))
    .map_err(std::io::Error::other),
  )?;

  let response_data = convert_from_http_response(response)?;
  let mut response_reader = StreamReader::new(response_data);
  tokio::io::copy(&mut response_reader, &mut tokio::io::stdout()).await?;

  Ok(())
}
