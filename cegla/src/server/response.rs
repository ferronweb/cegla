use std::pin::Pin;
use std::task::Poll;

use bytes::Bytes;
use futures_util::Stream;
use http::{HeaderMap, HeaderName, HeaderValue};
use http_body::Body;

/// Converts an HTTP response into a CGI-like server response stream
pub fn convert_from_http_response<B>(response: http::Response<B>) -> Result<CgiResponse<B>, std::io::Error>
where
  B: Body,
  B::Data: AsRef<[u8]> + Send + 'static,
  B::Error: Into<std::io::Error>,
{
  let (mut parts, body) = response.into_parts();

  // CGI-specific "Status" header
  parts.headers.insert(
    HeaderName::from_static("status"),
    HeaderValue::from_str(
      &parts
        .status
        .canonical_reason()
        .map_or(parts.status.as_u16().to_string(), |reason| {
          format!("{} {}", parts.status.as_u16(), reason)
        }),
    )
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid header value"))?,
  );

  Ok(CgiResponse {
    body: Box::pin(body),
    headers: Some(parts.headers),
  })
}

/// A CGI-like server response
pub struct CgiResponse<B> {
  body: Pin<Box<B>>,
  headers: Option<HeaderMap>,
}

impl<B> Stream for CgiResponse<B>
where
  B: Body,
  B::Data: AsRef<[u8]> + Send + 'static,
  B::Error: Into<std::io::Error>,
{
  type Item = Result<Bytes, std::io::Error>;

  fn poll_next(
    mut self: std::pin::Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<Option<Self::Item>> {
    if let Some(headers) = self.headers.take() {
      // CGI headers
      let mut data = Vec::new();
      for (header_name, header_value) in headers {
        if let Some(header_name) = header_name {
          let mut header_name_new = String::new();
          let mut separated = true;
          for c in header_name.as_str().chars() {
            if c == '-' {
              header_name_new.push(c);
              separated = true;
            } else if separated {
              header_name_new.push(c.to_ascii_uppercase());
              separated = false;
            } else {
              header_name_new.push(c.to_ascii_lowercase());
            }
          }
          data.extend_from_slice(header_name_new.as_bytes());
          data.extend_from_slice(b": ");
          data.extend_from_slice(header_value.as_bytes());
          data.extend_from_slice(b"\r\n");
        }
      }
      if !data.is_empty() {
        data.extend_from_slice(b"\r\n");
        return Poll::Ready(Some(Ok(Bytes::from_owner(data))));
      }
    }

    // Response body
    match Pin::new(&mut self.body).poll_frame(cx) {
      Poll::Ready(Some(Ok(frame))) => {
        if let Ok(data) = frame.into_data() {
          Poll::Ready(Some(Ok(Bytes::from_owner(data))))
        } else {
          Poll::Ready(None)
        }
      }
      Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err.into()))),
      Poll::Ready(None) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use futures_util::StreamExt;
  use http::{Response, StatusCode};
  use http_body::Frame;
  use std::task::Context;

  /// Mock body for testing
  struct MockBody {
    data: Vec<Result<Bytes, std::io::Error>>,
  }

  impl Body for MockBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
      mut self: Pin<&mut Self>,
      _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
      if !self.data.is_empty() {
        let item = self.data.remove(0);
        Poll::Ready(Some(Ok(Frame::data(item?))))
      } else {
        Poll::Ready(None)
      }
    }
  }

  #[tokio::test]
  async fn test_convert_from_http_response() {
    let body = MockBody {
      data: vec![Ok(Bytes::from("test data"))],
    };
    let response = Response::builder().status(StatusCode::OK).body(body).unwrap();

    let result = super::convert_from_http_response(response);
    assert!(result.is_ok());
    let cgi_response = result.unwrap();

    // Check if headers are set correctly
    assert!(cgi_response.headers.is_some());
    let headers = cgi_response.headers.unwrap();
    assert!(headers.contains_key("status"));
    assert_eq!(headers["status"], HeaderValue::from_static("200 OK"));
  }

  #[tokio::test]
  async fn test_cgi_response_stream() {
    let body = MockBody {
      data: vec![Ok(Bytes::from("test data"))],
    };
    let response = Response::builder().status(StatusCode::OK).body(body).unwrap();

    let cgi_response = super::convert_from_http_response(response).unwrap();
    let stream = Box::pin(cgi_response);

    // Collect all items from the stream
    let items: Vec<_> = stream.collect().await;

    let mut data = Vec::new();
    for bytes in items.into_iter().flatten() {
      data.extend_from_slice(&bytes);
    }

    // Check if the stream contains the expected data
    assert!(memchr::memmem::find(&data, b"test data").is_some());
  }

  #[tokio::test]
  async fn test_cgi_response_stream_with_headers() {
    let body = MockBody {
      data: vec![Ok(Bytes::from("test data"))],
    };
    let response = Response::builder()
      .status(StatusCode::OK)
      .header("content-type", "text/plain")
      .body(body)
      .unwrap();

    let cgi_response = super::convert_from_http_response(response).unwrap();
    let stream = Box::pin(cgi_response);

    // Collect all items from the stream
    let items: Vec<_> = stream.collect().await;
    let mut data = Vec::new();
    for bytes in items.into_iter().flatten() {
      data.extend_from_slice(&bytes);
    }

    // Check if the stream contains the expected data
    assert!(memchr::memmem::find(&data, b"Content-Type: text/plain").is_some());
    assert!(memchr::memmem::find(&data, b"test data").is_some());
  }
}
