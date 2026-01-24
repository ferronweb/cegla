use std::io::Error;
use std::pin::Pin;
use std::task::{Context, Poll};

use memchr::memchr2;
use smallvec::SmallVec;
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

use crate::CgiIncoming;

/// Constant defining the capacity of the response buffer
const RESPONSE_BUFFER_CAPACITY: usize = 16384;

/// Converts a CGI-like server response stream into an HTTP response
pub async fn convert_to_http_response<R>(
  stream: R,
) -> Result<http::Response<CgiIncoming<CgiResponseInner<R>>>, std::io::Error>
where
  R: AsyncRead + Unpin + 'static,
{
  let mut cgi_response_inner = CgiResponseInner::new(stream);
  let mut headers = [httparse::EMPTY_HEADER; 128];

  let obtained_head = cgi_response_inner.get_head().await?;
  if !obtained_head.is_empty() {
    httparse::parse_headers(obtained_head, &mut headers)
      .map_err(|e| std::io::Error::other(format!("HTTP response error: {e}")))?;
  }

  let mut response_builder = http::Response::builder();
  let mut status_code = 200;
  for header in headers {
    if header == httparse::EMPTY_HEADER {
      break;
    }
    let mut is_status_header = false;
    match &header.name.to_lowercase() as &str {
      "location" => {
        if !(300..=399).contains(&status_code) {
          status_code = 302;
        }
      }
      "status" => {
        is_status_header = true;
        let header_value_cow = String::from_utf8_lossy(header.value);
        let mut split_status = header_value_cow.split(" ");
        let first_part = split_status.next();
        if let Some(first_part) = first_part {
          if first_part.starts_with("HTTP/") {
            let second_part = split_status.next();
            if let Some(second_part) = second_part {
              if let Ok(parsed_status_code) = second_part.parse::<u16>() {
                status_code = parsed_status_code;
              }
            }
          } else if let Ok(parsed_status_code) = first_part.parse::<u16>() {
            status_code = parsed_status_code;
          }
        }
      }
      _ => (),
    }
    if !is_status_header {
      response_builder = response_builder.header(header.name, header.value);
    }
  }

  response_builder = response_builder.status(status_code);

  response_builder
    .body(CgiIncoming::new(cgi_response_inner))
    .map_err(|e| std::io::Error::other(format!("HTTP response error: {e}")))
}

/// Struct representing an inner CGI response
pub struct CgiResponseInner<R>
where
  R: AsyncRead + Unpin,
{
  stream: R,
  response_buf: SmallVec<[u8; RESPONSE_BUFFER_CAPACITY]>,
  response_head_length: Option<usize>,
}

impl<R> CgiResponseInner<R>
where
  R: AsyncRead + Unpin,
{
  /// Constructor to create a new CgiResponseInner instance
  fn new(stream: R) -> Self {
    Self {
      stream,
      response_buf: SmallVec::with_capacity(RESPONSE_BUFFER_CAPACITY),
      response_head_length: None,
    }
  }

  /// Asynchronous method to get the response headers
  async fn get_head(&mut self) -> Result<&[u8], Error> {
    let mut temp_buf = [0u8; RESPONSE_BUFFER_CAPACITY];
    let to_parse_length;

    loop {
      // Read data from the stream into the temporary buffer
      let read_bytes = self.stream.read(&mut temp_buf).await?;

      // If no bytes are read, return an empty response head
      if read_bytes == 0 {
        self.response_head_length = Some(0);
        return Ok::<&[u8], _>(&[0u8; 0]);
      }

      // If the response buffer exceeds the capacity, return an empty response head
      if self.response_buf.len() + read_bytes > RESPONSE_BUFFER_CAPACITY {
        self.response_head_length = Some(0);
        return Ok::<&[u8], _>(&[0u8; 0]);
      }

      // Determine the starting point for searching the "\r\n\r\n" sequence
      let begin_search = self.response_buf.len().saturating_sub(3);
      self.response_buf.extend_from_slice(&temp_buf[..read_bytes]);

      // Search for the "\r\n\r\n" sequence in the response buffer
      if let Some((separator_index, separator_len)) = search_header_body_separator(&self.response_buf[begin_search..]) {
        to_parse_length = begin_search + separator_index + separator_len;
        break;
      }
    }

    // Set the length of the response header
    self.response_head_length = Some(to_parse_length);

    // Return the response header as a byte slice
    Ok(&self.response_buf[..to_parse_length])
  }
}

// Implementation of AsyncRead for the CgiResponseInner struct
impl<R> AsyncRead for CgiResponseInner<R>
where
  R: AsyncRead + Unpin,
{
  #[inline]
  fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
    // If the response header length is known and the buffer contains more data than the header length
    if let Some(response_head_length) = self.response_head_length {
      if self.response_buf.len() > response_head_length {
        let remaining_data = &self.response_buf[response_head_length..];
        let to_read = remaining_data.len().min(buf.remaining());
        buf.put_slice(&remaining_data[..to_read]);
        self.response_head_length = Some(response_head_length + to_read);
        return Poll::Ready(Ok(()));
      }
    }

    // Create a temporary buffer to hold the data to be consumed
    let stream = Pin::new(&mut self.stream);
    match stream.poll_read(cx, buf) {
      Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
      other => other,
    }
  }
}

/// Searches for the header/body separator in a given slice.
/// Returns the index of the separator and the length of the separator.
#[inline]
fn search_header_body_separator(slice: &[u8]) -> Option<(usize, usize)> {
  if slice.len() < 2 {
    // Slice too short
    return None;
  }
  let mut last_chars: SmallVec<[u8; 4]> = SmallVec::with_capacity(4);
  let mut index = 0;
  while let Some(found_index) = memchr2(b'\r', b'\n', &slice[index..]) {
    if found_index > 0 {
      // Not "\n\n", "\r\n\r\n", "\r\r", nor "\n\n"...
      last_chars.clear();
    }
    let ch = slice[index + found_index];
    if last_chars.get(last_chars.len().saturating_sub(1)) == Some(&ch) {
      // "\n\n" or "\r\r"
      return Some((index + found_index - 1, 2));
    } else {
      last_chars.push(ch);
    }
    if last_chars.len() == 4 {
      // "\r\n\r\n" or "\n\r\n\r"
      return Some((index + found_index - 3, 4));
    }
    index += found_index + 1;
    if index >= slice.len() {
      break;
    }
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;
  use http_body_util::BodyDataStream;
  use tokio::io::AsyncReadExt;
  use tokio_test::io::Builder;
  use tokio_util::io::StreamReader;

  #[tokio::test]
  async fn test_get_head() {
    let data = b"Content-Type: text/plain\r\n\r\n";
    let stream = Builder::new().read(data).build();
    let response = convert_to_http_response(stream).await.unwrap();

    assert_eq!(
      response.headers().get(http::header::CONTENT_TYPE).unwrap().as_bytes(),
      b"text/plain"
    );
  }

  #[tokio::test]
  async fn test_get_head_and_body() {
    let data = b"Content-Type: text/plain\r\n\r\nHello, world!";
    let stream = Builder::new().read(data).build();
    let response = convert_to_http_response(stream).await.unwrap();
    let (parts, body) = response.into_parts();

    assert_eq!(
      parts.headers.get(http::header::CONTENT_TYPE).unwrap().as_bytes(),
      b"text/plain"
    );

    let mut buf = Vec::new();
    StreamReader::new(BodyDataStream::new(body))
      .read_to_end(&mut buf)
      .await
      .unwrap();
    assert_eq!(&buf, b"Hello, world!");
  }

  #[tokio::test]
  async fn test_inner_get_head() {
    let data = b"Content-Type: text/plain\r\n\r\n";
    let mut stream = Builder::new().read(data).build();
    let mut response = CgiResponseInner::new(&mut stream);

    let head = response.get_head().await.unwrap();
    assert_eq!(head, b"Content-Type: text/plain\r\n\r\n");
  }

  #[tokio::test]
  async fn test_inner_get_head_nn() {
    let data = b"Content-Type: text/plain\n\n";
    let mut stream = Builder::new().read(data).build();
    let mut response = CgiResponseInner::new(&mut stream);

    let head = response.get_head().await.unwrap();
    assert_eq!(head, b"Content-Type: text/plain\n\n");
  }

  #[tokio::test]
  async fn test_inner_get_head_empty() {
    let data = b"\r\n\r\n";
    let mut stream = Builder::new().read(data).build();
    let mut response = CgiResponseInner::new(&mut stream);

    let head = response.get_head().await.unwrap();
    assert_eq!(head, b"\r\n\r\n");
  }

  #[tokio::test]
  async fn test_inner_get_head_empty_nn() {
    let data = b"\n\n";
    let mut stream = Builder::new().read(data).build();
    let mut response = CgiResponseInner::new(&mut stream);

    let head = response.get_head().await.unwrap();
    assert_eq!(head, b"\n\n");
  }

  #[tokio::test]
  async fn test_inner_get_head_large_headers() {
    let data = b"Content-Type: text/plain\r\n";
    let large_header = vec![b'A'; RESPONSE_BUFFER_CAPACITY + 10]
      .into_iter()
      .collect::<Vec<u8>>();
    let mut stream = Builder::new().read(data).read(&large_header).build();
    let mut response = CgiResponseInner::new(&mut stream);

    let result = response.get_head().await;
    assert_eq!(result.unwrap().len(), 0);

    // Consume the remaining data to avoid panicking
    let mut remaining_data = vec![0u8; RESPONSE_BUFFER_CAPACITY + 10];
    let _ = response.stream.read(&mut remaining_data).await;
  }

  #[tokio::test]
  async fn test_inner_get_head_premature_eof() {
    let data = b"Content-Type: text/plain\r\n";
    let mut stream = Builder::new().read(data).build();
    let mut response = CgiResponseInner::new(&mut stream);

    let result = response.get_head().await;
    assert_eq!(result.unwrap().len(), 0);
  }

  #[tokio::test]
  async fn test_inner_poll_read() {
    let data = b"Content-Type: text/plain\r\n\r\nHello, world!";
    let mut stream = Builder::new().read(data).build();
    let mut response = CgiResponseInner::new(&mut stream);

    let head = response.get_head().await.unwrap();
    assert_eq!(head, b"Content-Type: text/plain\r\n\r\n");

    let mut buf = vec![0u8; 13];
    let n = response.read(&mut buf).await.unwrap();
    assert_eq!(n, 13);
    assert_eq!(&buf[..n], b"Hello, world!");
  }
}
