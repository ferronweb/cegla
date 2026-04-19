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
  B::Error: Into<std::io::Error>,
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
  B::Error: Into<std::io::Error>,
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
                .body(http_body_util::Empty::<Bytes>::new().map_err(|e| -> std::io::Error { match e {} }))
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
            Poll::Ready(Some(Err(e))) => {
              return Poll::Ready(Err(e.into()));
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
        Poll::Ready(None) => {
          return Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "unexpected EOF when reading FastCGI record",
          )));
        }
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
  B::Error: Into<std::io::Error>,
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
