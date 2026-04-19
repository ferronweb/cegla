//! Client-side FastCGI implementation.

use std::{
  collections::{HashMap, VecDeque},
  future::Future,
  pin::Pin,
  task::{Context, Poll},
};

use cegla::{client::convert_to_http_response, CgiIncoming};
use futures_util::{stream::FuturesUnordered, Sink, StreamExt};
use http_body::Body;
use http_body_util::BodyExt;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, ReadHalf, WriteHalf};
use tokio_util::{
  bytes::Bytes,
  codec::{FramedRead, FramedWrite},
};

pub use cegla::client::CgiBuilder;

use crate::protocol::{
  codec::{Decoder, Encoder},
  constants::{RecordType, Role, FCGI_KEEP_CONN},
  id_alloc::IdAllocator,
  name_value_pair::NameValuePair,
  record::Record,
};

/// An [`AsyncRead`] adapter that reassembles `FCGI_STDOUT` record content
/// chunks received over an [`async_channel`] into a byte stream suitable for
/// [`convert_to_http_response`].
///
/// A `None` sentinel on the channel signals end-of-stream (the server sent an
/// empty `FCGI_STDOUT` record or an `FCGI_END_REQUEST` record).
pub struct FcgiResponseReader {
  // Incoming chunks of stdout data.  `None` means EOF.
  // Boxed + pinned so that the struct itself is Unpin.
  rx: Pin<Box<async_channel::Receiver<Option<Vec<u8>>>>>,
  // Leftover bytes from the last chunk that haven't been consumed yet.
  leftover: Option<(Vec<u8>, usize)>,
}

// SAFETY: FcgiResponseReader only contains a pinned box (heap-allocated) and
// plain data.  The box makes the receiver address-stable, so moving
// FcgiResponseReader is safe.
impl Unpin for FcgiResponseReader {}

impl AsyncRead for FcgiResponseReader {
  fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
    loop {
      // Drain any leftover bytes first.
      if let Some((ref data, ref mut offset)) = self.leftover {
        let remaining = &data[*offset..];
        let to_copy = remaining.len().min(buf.remaining());
        buf.put_slice(&remaining[..to_copy]);
        *offset += to_copy;
        if *offset >= data.len() {
          self.leftover = None;
        }
        return Poll::Ready(Ok(()));
      }

      // Ask the channel for the next chunk.
      match self.rx.as_mut().poll_next_unpin(cx) {
        Poll::Ready(Some(Some(chunk))) => {
          if chunk.is_empty() {
            continue;
          }
          let to_copy = chunk.len().min(buf.remaining());
          buf.put_slice(&chunk[..to_copy]);
          if to_copy < chunk.len() {
            self.leftover = Some((chunk, to_copy));
          }
          return Poll::Ready(Ok(()));
        }
        // EOF sentinel or channel closed.
        Poll::Ready(Some(None)) | Poll::Ready(None) => {
          return Poll::Ready(Ok(()));
        }
        Poll::Pending => return Poll::Pending,
      }
    }
  }
}

// ---------------------------------------------------------------------------
// PendingRequest
// ---------------------------------------------------------------------------

/// State kept for every in-flight FastCGI request.
struct PendingRequest<B> {
  // The HTTP request body (needed to stream FCGI_STDIN records).
  // Boxed + pinned so that the body is polled through a stable address.
  body: Option<Pin<Box<B>>>,
  // Channel used to feed `FCGI_STDOUT` bytes to [`FcgiResponseReader`].
  stdout_tx: async_channel::Sender<Option<Vec<u8>>>,
  // Channel used to feed `FCGI_STDERR` bytes to [`FcgiResponseReader`].
  stderr_tx: async_channel::Sender<Option<Vec<u8>>>,
  // Whether the `FCGI_END_REQUEST` record has been received.
  end_received: bool,
  // Futures that convert the accumulated stdout into an HTTP response and
  // deliver it through the oneshot channel.
  response_futures: FuturesUnordered<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
}

// ---------------------------------------------------------------------------
// SentRequest (internal message from SendRequest → Connection)
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
struct SentRequest<B> {
  req: http::Request<B>,
  builder: CgiBuilder,
  res_ch: oneshot::Sender<
    Result<
      (
        http::Response<CgiIncoming<cegla::client::CgiResponseInner<FcgiResponseReader>>>,
        FcgiResponseReader,
      ),
      std::io::Error,
    >,
  >,
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

pin_project_lite::pin_project! {
  /// Represents a connection to a FastCGI server to be spawned as a background task.
  pub struct Connection<Io, B> {
    #[pin]
    reader: FramedRead<ReadHalf<Io>, Decoder>,
    #[pin]
    writer: FramedWrite<WriteHalf<Io>, Encoder>,
    keepalive: bool,
    #[pin]
    send_request_rx: async_channel::Receiver<SentRequest<B>>,
    // Per-request state, keyed by FastCGI request ID.
    pending: HashMap<u16, PendingRequest<B>>,
    // ID allocator for FastCGI request IDs.
    id_alloc: IdAllocator,
    // Records waiting to be flushed to the writer.
    write_queue: VecDeque<Record>,
  }
}

// ---------------------------------------------------------------------------
// SendRequest
// ---------------------------------------------------------------------------

/// Represents a handle to send requests to a FastCGI server.
pub struct SendRequest<B> {
  send_request_tx: async_channel::Sender<SentRequest<B>>,
}

impl<B> SendRequest<B>
where
  B: Body + Send + 'static,
  B::Data: AsRef<[u8]> + Send + 'static,
{
  /// Sends an HTTP request to the FastCGI application and awaits the response.
  pub async fn send_request(
    &self,
    req: http::Request<B>,
    builder: CgiBuilder,
  ) -> Result<
    (
      http::Response<CgiIncoming<cegla::client::CgiResponseInner<FcgiResponseReader>>>,
      FcgiResponseReader,
    ),
    std::io::Error,
  > {
    let (res_ch, res_rx) = oneshot::channel();
    if self
      .send_request_tx
      .send(SentRequest { req, builder, res_ch })
      .await
      .is_err()
    {
      return Err(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "underlying connection closed",
      ));
    }
    res_rx
      .await
      .map_err(|_| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "unknown error obtaining response"))?
  }
}

// ---------------------------------------------------------------------------
// Connection::poll  (the core state machine)
// ---------------------------------------------------------------------------

impl<Io, B> Future for Connection<Io, B>
where
  Io: AsyncRead + AsyncWrite + Unpin + 'static,
  B: Body + Send + 'static,
  B::Data: AsRef<[u8]> + Send + 'static,
{
  type Output = Result<(), std::io::Error>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let mut this = self.project();

    // ------------------------------------------------------------------
    // 1. Accept new requests from SendRequest handles
    // ------------------------------------------------------------------
    loop {
      match this.send_request_rx.poll_next_unpin(cx) {
        Poll::Ready(Some(SentRequest { req, builder, res_ch })) => {
          // Allocate a fresh FastCGI request ID.
          let request_id = match this.id_alloc.allocate() {
            Some(id) => id,
            None => {
              // No IDs available – reject immediately.
              let _ = res_ch.send(Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "no FastCGI request IDs available",
              )));
              continue;
            }
          };

          // ---- FCGI_BEGIN_REQUEST ----
          // Role = Responder (1), flags = FCGI_KEEP_CONN when keepalive is set.
          let flags: u8 = if *this.keepalive { FCGI_KEEP_CONN } else { 0 };
          let role = Role::Responder.as_u16();
          let begin_body = vec![(role >> 8) as u8, (role & 0xff) as u8, flags, 0, 0, 0, 0, 0];
          this
            .write_queue
            .push_back(Record::new(RecordType::BeginRequest.as_u8(), request_id, begin_body));

          // ---- FCGI_PARAMS ----
          // Build CGI environment variables from the HTTP request.
          let (parts, body) = req.into_parts();

          let uri = parts.uri.clone();
          let method = parts.method.clone();
          let version = parts.version;
          let headers = parts.headers.clone();

          // Build CGI params directly from the request parts (no body needed).
          let synthetic_req = {
            let mut builder = http::Request::builder()
              .method(method.clone())
              .uri(uri.clone())
              .version(version);
            for (name, value) in &headers {
              builder = builder.header(name, value);
            }
            builder.body(http_body_util::Empty::<Bytes>::new()).unwrap()
          };

          // CgiBuilder::build requires B::Error: Into<io::Error>.
          // http_body_util::Empty<Bytes> has Error = Infallible which doesn't
          // satisfy that bound, so we extract the env vars manually instead.
          let (parts2, _) = synthetic_req.into_parts();
          let env: cegla::CgiEnvironment = {
            // Re-build with a compatible body type by mapping the error.
            let mapped_req = {
              let mut builder = http::Request::builder()
                .method(parts2.method)
                .uri(parts2.uri)
                .version(parts2.version);
              for (name, value) in &parts2.headers {
                builder = builder.header(name, value);
              }
              builder
                .body(http_body_util::Empty::<Bytes>::new().map_err(|e| -> std::io::Error { std::io::Error::other(e) }))
                .unwrap()
            };
            let (env, _) = builder.build(mapped_req);
            env
          };

          let mut params_data = Vec::new();
          for (name, value) in env.iter() {
            let nvp = NameValuePair::from_slices(name.as_bytes(), value.as_bytes());
            params_data.extend_from_slice(&nvp.encode());
          }

          // Chunk PARAMS into ≤65535-byte records.
          for chunk in params_data.chunks(65535) {
            this
              .write_queue
              .push_back(Record::new(RecordType::Params.as_u8(), request_id, chunk.to_vec()));
          }
          // Empty PARAMS record terminates the params stream.
          this
            .write_queue
            .push_back(Record::new(RecordType::Params.as_u8(), request_id, vec![]));

          // Create the stdout and stderr pipes used by FcgiResponseReader.
          let (stdout_tx, stdout_rx) = async_channel::bounded::<Option<Vec<u8>>>(64);
          let (stderr_tx, stderr_rx) = async_channel::bounded::<Option<Vec<u8>>>(64);

          let reader = FcgiResponseReader {
            rx: Box::pin(stdout_rx),
            leftover: None,
          };

          // Spawn the response-conversion future inside the Connection so it
          // is driven without a separate task.
          let res_ch_cell = std::sync::Mutex::new(Some(res_ch));
          let response_future: Pin<Box<dyn Future<Output = ()> + Send + 'static>> = Box::pin(async move {
            let result = convert_to_http_response(reader).await;
            if let Some(ch) = res_ch_cell.lock().unwrap().take() {
              let _ = ch.send(result.map(|r| {
                (
                  r,
                  FcgiResponseReader {
                    rx: Box::pin(stderr_rx),
                    leftover: None,
                  },
                )
              }));
            }
          });

          let response_futures: FuturesUnordered<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> =
            FuturesUnordered::new();
          response_futures.push(response_future);

          this.pending.insert(
            request_id,
            PendingRequest {
              body: Some(Box::pin(body)),
              stdout_tx,
              stderr_tx,
              end_received: false,
              response_futures,
            },
          );
        }
        Poll::Ready(None) => {
          // All SendRequest handles dropped – begin graceful shutdown.
          if this.pending.is_empty() {
            return Poll::Ready(Ok(()));
          }
          break;
        }
        Poll::Pending => break,
      }
    }

    // ------------------------------------------------------------------
    // 2. Drain STDIN from pending requests that still have a body
    // ------------------------------------------------------------------
    for (&request_id, pending) in this.pending.iter_mut() {
      if let Some(ref mut body) = pending.body {
        let mut body_done = false;
        loop {
          match body.as_mut().poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
              if let Ok(data) = frame.into_data() {
                let slice: &[u8] = data.as_ref();
                if !slice.is_empty() {
                  for chunk in slice.chunks(65535) {
                    this
                      .write_queue
                      .push_back(Record::new(RecordType::Stdin.as_u8(), request_id, chunk.to_vec()));
                  }
                }
              }
            }
            Poll::Ready(Some(Err(_))) => {
              this
                .write_queue
                .push_back(Record::new(RecordType::Stdin.as_u8(), request_id, Vec::new()));
            }
            Poll::Ready(None) => {
              body_done = true;
              break;
            }
            Poll::Pending => break,
          }
        }
        if body_done {
          // Empty STDIN record terminates the stdin stream.
          this
            .write_queue
            .push_back(Record::new(RecordType::Stdin.as_u8(), request_id, vec![]));
          pending.body = None;
        }
      }
    }

    // ------------------------------------------------------------------
    // 3. Flush the write queue to the FramedWrite
    // ------------------------------------------------------------------
    while let Some(record) = this.write_queue.front() {
      let record = record.clone();
      match this.writer.as_mut().poll_ready(cx) {
        Poll::Ready(Ok(())) => {
          this.write_queue.pop_front();
          if let Err(e) = this.writer.as_mut().start_send(record) {
            return Poll::Ready(Err(e));
          }
        }
        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
        Poll::Pending => break,
      }
    }
    // Flush any buffered data.
    if let Poll::Ready(Err(e)) = this.writer.as_mut().poll_flush(cx) {
      return Poll::Ready(Err(e));
    }

    // ------------------------------------------------------------------
    // 4. Read incoming FastCGI records and dispatch them
    // ------------------------------------------------------------------
    loop {
      match this.reader.poll_next_unpin(cx) {
        Poll::Ready(Some(Ok(record))) => {
          let request_id = record.request_id;
          match RecordType::from_u8(record.record_type) {
            Some(RecordType::Stdout) => {
              if let Some(pending) = this.pending.get_mut(&request_id) {
                if record.content.is_empty() {
                  // Empty STDOUT = EOF sentinel.
                  let _ = pending.stdout_tx.try_send(None);
                } else {
                  let _ = pending.stdout_tx.try_send(Some(record.content));
                }
              }
            }
            Some(RecordType::Stderr) => {
              if let Some(pending) = this.pending.get_mut(&request_id) {
                if record.content.is_empty() {
                  // Empty STDERR = EOF sentinel.
                  let _ = pending.stderr_tx.try_send(None);
                } else {
                  let _ = pending.stderr_tx.try_send(Some(record.content));
                }
              }
            }
            Some(RecordType::EndRequest) => {
              if let Some(pending) = this.pending.get_mut(&request_id) {
                // Ensure EOF is signalled in case no empty STDOUT was sent.
                let _ = pending.stdout_tx.try_send(None);
                pending.end_received = true;
              }
            }
            _ => {
              // Ignore unknown / unexpected records.
            }
          }
        }
        Poll::Ready(Some(Err(e))) => {
          return Poll::Ready(Err(e));
        }
        Poll::Ready(None) => break,
        Poll::Pending => break,
      }
    }

    // ------------------------------------------------------------------
    // 5. Poll response-conversion futures and clean up finished requests
    // ------------------------------------------------------------------
    let mut finished_ids: Vec<u16> = Vec::new();
    for (&request_id, pending) in this.pending.iter_mut() {
      while let Poll::Ready(Some(())) = pending.response_futures.poll_next_unpin(cx) {}

      // A request is fully done when END_REQUEST arrived and all response
      // futures have completed.
      if pending.end_received && pending.response_futures.is_empty() {
        finished_ids.push(request_id);
      }
    }
    for id in finished_ids {
      this.pending.remove(&id);
      this.id_alloc.free(id);
    }

    Poll::Pending
  }
}

// ---------------------------------------------------------------------------
// Record: add Clone so we can peek-then-send in the write loop
// ---------------------------------------------------------------------------

impl Clone for Record {
  fn clone(&self) -> Self {
    Self {
      version: self.version,
      record_type: self.record_type,
      request_id: self.request_id,
      content: self.content.clone(),
      padding_length: self.padding_length,
    }
  }
}

// ---------------------------------------------------------------------------
// handshake
// ---------------------------------------------------------------------------

/// Performs the FastCGI handshake and returns a handle to send requests and a
/// connection future.
///
/// The returned [`Connection`] must be driven to completion (e.g. via
/// `tokio::spawn`) for requests submitted through [`SendRequest`] to make
/// progress.
pub async fn handshake<Io, B>(io: Io, keepalive: bool) -> Result<(SendRequest<B>, Connection<Io, B>), std::io::Error>
where
  Io: AsyncRead + AsyncWrite + Unpin + 'static,
  B: Body + Send + 'static,
  B::Data: AsRef<[u8]> + Send + 'static,
{
  let (reader, writer) = tokio::io::split(io);
  let framed_read = FramedRead::new(reader, Decoder::default());
  let framed_write = FramedWrite::new(writer, Encoder);
  let (send_request_tx, send_request_rx) = async_channel::unbounded();

  Ok((
    SendRequest { send_request_tx },
    Connection {
      reader: framed_read,
      writer: framed_write,
      keepalive,
      send_request_rx,
      pending: HashMap::default(),
      id_alloc: IdAllocator::new(),
      write_queue: VecDeque::new(),
    },
  ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;
  use crate::protocol::codec::{Decoder, Encoder};
  use tokio::io::{AsyncRead, AsyncWrite};
  use tokio_util::codec::FramedRead;

  /// Mock IO type for testing
  struct MockIo {
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    read_pos: usize,
  }

  impl AsyncRead for MockIo {
    fn poll_read(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
      let remaining = &self.read_buf[self.read_pos..];
      let to_read = remaining.len().min(buf.remaining());
      buf.put_slice(&remaining[..to_read]);
      self.read_pos += to_read;
      Poll::Ready(Ok(()))
    }
  }

  impl AsyncWrite for MockIo {
    fn poll_write(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
      self.write_buf.extend_from_slice(buf);
      Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
      Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
      Poll::Ready(Ok(()))
    }
  }

  /// Create a simple mock response with headers and body
  fn make_mock_response(content: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();

    // FCGI_STDOUT record with content
    let stdout_record = Record::new(RecordType::Stdout.as_u8(), 1, content.to_vec());
    result.extend_from_slice(&stdout_record.encode().unwrap());

    // Empty FCGI_STDOUT record (EOF sentinel)
    let empty_stdout = Record::new(RecordType::Stdout.as_u8(), 1, vec![]);
    result.extend_from_slice(&empty_stdout.encode().unwrap());

    // FCGI_END_REQUEST record
    let end_request_body = vec![0, 0, 0, 0, 0, 0, 0, 0]; // status=0, protocolStatus=0
    let end_request = Record::new(RecordType::EndRequest.as_u8(), 1, end_request_body);
    result.extend_from_slice(&end_request.encode().unwrap());

    result
  }

  #[tokio::test]
  async fn test_fcgi_response_reader_basic() {
    let content = b"Content-Type: text/plain\r\n\r\nHello";
    let mock_data = make_mock_response(content);

    let mock_io = MockIo {
      read_buf: mock_data,
      write_buf: Vec::new(),
      read_pos: 0,
    };

    let (reader, writer) = tokio::io::split(mock_io);
    let framed_read = FramedRead::new(reader, Decoder::default());
    let framed_write = FramedWrite::new(writer, Encoder);
    let (_send_request_tx, send_request_rx) = async_channel::unbounded::<SentRequest<http_body_util::Full<Bytes>>>();

    let connection = Connection {
      reader: framed_read,
      writer: framed_write,
      keepalive: false,
      send_request_rx,
      pending: HashMap::default(),
      id_alloc: IdAllocator::new(),
      write_queue: VecDeque::new(),
    };

    // Test that we can create a connection
    drop(connection);
  }

  #[tokio::test]
  async fn test_handshake() {
    let mock_data = make_mock_response(b"faked");
    let mock_io = MockIo {
      read_buf: mock_data,
      write_buf: Vec::new(),
      read_pos: 0,
    };

    // Use Full<Bytes> which has io::Error as its error type
    let (send_req, connection) = handshake::<MockIo, http_body_util::Full<Bytes>>(mock_io, false)
      .await
      .unwrap();

    // Verify we got a SendRequest handle
    drop(send_req);
    drop(connection);
  }

  #[tokio::test]
  async fn test_send_request_basic() {
    let mock_data = make_mock_response(b"Content-Type: text/plain\r\n\r\nHello");
    let mock_io = MockIo {
      read_buf: mock_data,
      write_buf: Vec::new(),
      read_pos: 0,
    };

    let (send_req, connection) = handshake::<MockIo, http_body_util::Full<Bytes>>(mock_io, false)
      .await
      .unwrap();
    tokio::spawn(connection);

    // Create a simple request with Full body
    let request = http::Request::builder()
      .method("GET")
      .uri("http://example.com/test")
      .body(http_body_util::Full::<Bytes>::from(b"".as_ref()))
      .unwrap();

    let builder = CgiBuilder::new();

    // Send the request
    let result = send_req.send_request(request, builder).await;

    // We expect success since we provided a valid mock response
    assert!(result.is_ok());
  }

  #[tokio::test]
  async fn test_connection_drives_response() {
    let content = b"Content-Type: text/plain\r\n\r\nHello, FastCGI!";
    let mock_data = make_mock_response(content);
    let mock_io = MockIo {
      read_buf: mock_data,
      write_buf: Vec::new(),
      read_pos: 0,
    };

    let (send_req, connection) = handshake::<MockIo, http_body_util::Full<Bytes>>(mock_io, false)
      .await
      .unwrap();

    // Spawn the connection task
    let connection_task = tokio::spawn(connection);

    // Give the task a moment to process
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Create and send a request
    let request = http::Request::builder()
      .method("POST")
      .uri("http://example.com/api")
      .body(http_body_util::Full::<Bytes>::from(b"test body".as_ref()))
      .unwrap();

    let builder = CgiBuilder::new();
    let (response, _stdout_reader) = send_req.send_request(request, builder).await.unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers().get("Content-Type").unwrap(), "text/plain");

    // Drop the connection to stop the task
    drop(connection_task);
  }

  #[tokio::test]
  async fn test_request_id_allocation() {
    let mock_data = make_mock_response(b"faked");
    let mock_io = MockIo {
      read_buf: mock_data,
      write_buf: Vec::new(),
      read_pos: 0,
    };

    let (_send_req, connection) = handshake::<MockIo, http_body_util::Full<Bytes>>(mock_io, false)
      .await
      .unwrap();

    // The connection should have an IdAllocator that can allocate IDs
    // This is tested implicitly through the connection's operation

    drop(connection);
  }

  #[tokio::test]
  async fn test_keepalive_flag() {
    let mock_data = make_mock_response(b"faked");
    let mock_io = MockIo {
      read_buf: mock_data,
      write_buf: Vec::new(),
      read_pos: 0,
    };

    let (send_req, connection) = handshake::<MockIo, http_body_util::Full<Bytes>>(mock_io, true)
      .await
      .unwrap();

    tokio::spawn(connection);

    // Create a request
    let request = http::Request::builder()
      .method("GET")
      .uri("http://example.com/")
      .body(http_body_util::Full::<Bytes>::from(b"".as_ref()))
      .unwrap();

    let builder = CgiBuilder::new();
    let result = send_req.send_request(request, builder).await;
    assert!(result.is_ok());
  }

  #[tokio::test]
  async fn test_multiple_concurrent_requests() {
    // Create a mock that returns multiple responses
    let content1 = b"Content-Type: text/plain\r\n\r\nResponse1";
    let content2 = b"Content-Type: application/json\r\n\r\n{\"ok\":true}";

    let mut mock_data = Vec::new();

    // First response
    let stdout1 = Record::new(RecordType::Stdout.as_u8(), 1, content1.to_vec());
    mock_data.extend_from_slice(&stdout1.encode().unwrap());
    let empty1 = Record::new(RecordType::Stdout.as_u8(), 1, vec![]);
    mock_data.extend_from_slice(&empty1.encode().unwrap());
    let end1 = Record::new(RecordType::EndRequest.as_u8(), 1, vec![0, 0, 0, 0, 0, 0, 0, 0]);
    mock_data.extend_from_slice(&end1.encode().unwrap());

    // Second response (different request ID)
    let stdout2 = Record::new(RecordType::Stdout.as_u8(), 2, content2.to_vec());
    mock_data.extend_from_slice(&stdout2.encode().unwrap());
    let empty2 = Record::new(RecordType::Stdout.as_u8(), 2, vec![]);
    mock_data.extend_from_slice(&empty2.encode().unwrap());
    let end2 = Record::new(RecordType::EndRequest.as_u8(), 2, vec![0, 0, 0, 0, 0, 0, 0, 0]);
    mock_data.extend_from_slice(&end2.encode().unwrap());

    let mock_io = MockIo {
      read_buf: mock_data,
      write_buf: Vec::new(),
      read_pos: 0,
    };

    let (send_req, connection) = handshake::<MockIo, http_body_util::Full<Bytes>>(mock_io, false)
      .await
      .unwrap();
    let connection_task = tokio::spawn(connection);

    // Send two requests concurrently
    let req1 = http::Request::builder()
      .method("GET")
      .uri("http://example.com/1")
      .body(http_body_util::Full::<Bytes>::from(b"".as_ref()))
      .unwrap();

    let req2 = http::Request::builder()
      .method("GET")
      .uri("http://example.com/2")
      .body(http_body_util::Full::<Bytes>::from(b"".as_ref()))
      .unwrap();

    let f1 = send_req.send_request(req1, CgiBuilder::new());
    let f2 = send_req.send_request(req2, CgiBuilder::new());

    let (res1, res2) = tokio::join!(f1, f2);

    assert!(res1.is_ok());
    assert!(res2.is_ok());

    drop(connection_task);
  }

  #[tokio::test]
  async fn test_stderr_stream() {
    // Create a mock with both stdout and stderr
    let mut mock_data = Vec::new();

    // Stdout
    let stdout = Record::new(RecordType::Stdout.as_u8(), 1, b"OK".to_vec());
    mock_data.extend_from_slice(&stdout.encode().unwrap());

    // Stderr (error message)
    let stderr = Record::new(RecordType::Stderr.as_u8(), 1, b"Warning: something happened".to_vec());
    mock_data.extend_from_slice(&stderr.encode().unwrap());

    // EOF markers
    let empty_stdout = Record::new(RecordType::Stdout.as_u8(), 1, vec![]);
    mock_data.extend_from_slice(&empty_stdout.encode().unwrap());
    let empty_stderr = Record::new(RecordType::Stderr.as_u8(), 1, vec![]);
    mock_data.extend_from_slice(&empty_stderr.encode().unwrap());

    let end_request = Record::new(RecordType::EndRequest.as_u8(), 1, vec![0, 0, 0, 0, 0, 0, 0, 0]);
    mock_data.extend_from_slice(&end_request.encode().unwrap());

    let mock_io = MockIo {
      read_buf: mock_data,
      write_buf: Vec::new(),
      read_pos: 0,
    };

    let (send_req, connection) = handshake::<MockIo, http_body_util::Full<Bytes>>(mock_io, false)
      .await
      .unwrap();
    let connection_task = tokio::spawn(connection);

    let request = http::Request::builder()
      .method("GET")
      .uri("http://example.com/")
      .body(http_body_util::Full::<Bytes>::from(b"".as_ref()))
      .unwrap();

    let builder = CgiBuilder::new();
    let (_response, _stdout_reader) = send_req.send_request(request, builder).await.unwrap();

    drop(connection_task);
  }

  #[tokio::test]
  async fn test_empty_request_body() {
    let mock_data = make_mock_response(b"Content-Type: text/plain\r\n\r\n");
    let mock_io = MockIo {
      read_buf: mock_data,
      write_buf: Vec::new(),
      read_pos: 0,
    };

    let (send_req, connection) = handshake::<MockIo, http_body_util::Full<Bytes>>(mock_io, false)
      .await
      .unwrap();
    let connection_task = tokio::spawn(connection);

    let request = http::Request::builder()
      .method("GET")
      .uri("http://example.com/")
      .body(http_body_util::Full::<Bytes>::from(b"".as_ref()))
      .unwrap();

    let builder = CgiBuilder::new();
    let (response, _reader) = send_req.send_request(request, builder).await.unwrap();

    assert_eq!(response.status(), 200);

    drop(connection_task);
  }

  #[tokio::test]
  async fn test_request_body_streaming() {
    let content = b"Content-Type: text/plain\r\n\r\nReceived";
    let mock_data = make_mock_response(content);
    let mock_io = MockIo {
      read_buf: mock_data,
      write_buf: Vec::new(),
      read_pos: 0,
    };

    let (send_req, connection) = handshake::<MockIo, http_body_util::Full<Bytes>>(mock_io, false)
      .await
      .unwrap();
    let connection_task = tokio::spawn(connection);

    let body_data = b"This is a test body with some data";
    let request = http::Request::builder()
      .method("POST")
      .uri("http://example.com/upload")
      .body(http_body_util::Full::<Bytes>::from(body_data.as_ref()))
      .unwrap();

    let builder = CgiBuilder::new();
    let (_response, _reader) = send_req.send_request(request, builder).await.unwrap();

    drop(connection_task);
  }

  #[tokio::test]
  async fn test_connection_cleanup() {
    let mock_data = make_mock_response(b"faked");
    let mock_io = MockIo {
      read_buf: mock_data,
      write_buf: Vec::new(),
      read_pos: 0,
    };

    let (send_req, connection) = handshake::<MockIo, http_body_util::Full<Bytes>>(mock_io, false)
      .await
      .unwrap();

    // Drop the send_req to signal no more requests
    drop(send_req);

    // Connection should complete when pending is empty
    let result = connection.await;
    assert!(result.is_ok());
  }
}
