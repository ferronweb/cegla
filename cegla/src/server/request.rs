use tokio::io::AsyncRead;

use crate::{CgiEnvironment, CgiIncoming};

/// Converts a CGI-like server request stream into an HTTP request.
///
/// The CGI environment variables can be accessed through the `CgiIncoming` struct
/// in HTTP response extensions.
pub fn convert_cgi_request<R>(stream: R, env: CgiEnvironment) -> Result<http::Request<CgiIncoming<R>>, std::io::Error>
where
  R: AsyncRead + Unpin + 'static,
{
  let mut builder = http::Request::builder();
  for (key, value) in &env {
    if let Some(header_raw) = key.strip_prefix("HTTP_") {
      // See https://stackoverflow.com/a/1801191
      for value in value.split(if header_raw == "COOKIE" { ";" } else { "," }) {
        builder = builder.header(header_raw.replace('_', "-").to_lowercase(), value.trim());
      }
    } else {
      match key.as_str() {
        "REQUEST_METHOD" => builder = builder.method(value.as_bytes()),
        "REQUEST_URI" => builder = builder.uri(value),
        "CONTENT_LENGTH" => builder = builder.header(http::header::CONTENT_LENGTH, value),
        "CONTENT_TYPE" => builder = builder.header(http::header::CONTENT_TYPE, value),
        _ => {}
      }
    }
  }

  builder
    .extension(env)
    .body(CgiIncoming::new(stream))
    .map_err(|e| std::io::Error::other(format!("HTTP response error: {e}")))
}

#[cfg(test)]
mod tests {
  use super::*;
  use futures_util::StreamExt;
  use http_body_util::BodyDataStream;

  #[tokio::test]
  async fn test_convert_cgi_request() {
    let mut inner = hashlink::LinkedHashMap::new();
    inner.insert("REQUEST_URI".to_string(), "/path?a=b".to_string());
    inner.insert("REQUEST_METHOD".to_string(), "POST".to_string());
    inner.insert("CONTENT_LENGTH".to_string(), "9".to_string());
    inner.insert("HTTP_X_CUSTOM_HEADER".to_string(), "custom_value".to_string());
    let env = CgiEnvironment { inner };
    let stream = tokio_test::io::Builder::new().read(b"test body").build();
    let request = convert_cgi_request(stream, env).unwrap();
    let (parts, body) = request.into_parts();
    assert_eq!(parts.method, http::Method::POST);
    assert_eq!(parts.uri, "/path?a=b");
    assert_eq!(
      parts.headers.get(http::header::CONTENT_LENGTH),
      Some(&http::HeaderValue::from_static("9"))
    );
    assert_eq!(parts.headers.get(http::header::CONTENT_TYPE), None);
    assert_eq!(
      parts.headers.get(http::HeaderName::from_static("x-custom-header")),
      Some(&http::HeaderValue::from_static("custom_value"))
    );

    let items: Vec<_> = BodyDataStream::new(body).collect().await;

    let mut data = Vec::new();
    for bytes in items.into_iter().flatten() {
      data.extend_from_slice(&bytes);
    }
    assert_eq!(data, b"test body");
  }
}
