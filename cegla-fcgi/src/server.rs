//! Server-side FastCGI implementation.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::{future::Future, sync::Arc};

use cegla::{
  server::{convert_cgi_request, convert_from_http_response},
  CgiEnvironment, CgiIncoming,
};
use futures_util::{Sink, SinkExt, StreamExt};
use parking_lot::RwLock;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::{
  bytes::Bytes,
  codec::{FramedRead, FramedWrite},
  io::{CopyToBytes, SinkWriter, StreamReader},
};

use crate::protocol::constants::FCGI_KEEP_CONN;
use crate::protocol::{
  codec::{Decoder, Encoder},
  constants::{ProtocolStatus, RecordType, Role},
  name_value_pair::NameValuePair,
  record::Record,
};

/// Runtime trait for FastCGI server.
pub trait Runtime {
  /// Spawns a new task to execute the given future.
  fn spawn(&self, future: impl Future + Send + 'static);
}

/// Handles a FastCGI request by converting it to an HTTP request, invoking the provided request function,
/// and then converting the HTTP response back to a FastCGI response.
pub async fn server_handle_fcgi<Io, F, Fut, B, Err, R>(io: Io, runtime: R, request_fn: F) -> Result<(), std::io::Error>
where
  Io: AsyncRead + AsyncWrite + Unpin + 'static,
  F: Fn(
      http::Request<
        CgiIncoming<StreamReader<std::pin::Pin<Box<async_channel::Receiver<Result<Bytes, std::io::Error>>>>, Bytes>>,
      >,
      SinkWriter<CopyToBytes<std::pin::Pin<Box<dyn Sink<Bytes, Error = std::io::Error> + Send + Sync>>>>,
    ) -> Fut
    + Send
    + Sync
    + 'static,
  Fut: Future<Output = Result<http::Response<B>, Err>> + Send + 'static,
  B: http_body::Body + Send + 'static,
  B::Data: AsRef<[u8]> + Send + 'static,
  B::Error: Into<std::io::Error> + Send + 'static,
  Err: Into<std::io::Error> + Send + 'static,
  R: Runtime,
{
  let (reader, writer) = tokio::io::split(io);
  let mut framed_read = FramedRead::new(reader, Decoder::default());
  let mut framed_write = FramedWrite::new(writer, Encoder);

  let (write_tx, write_rx) = async_channel::bounded::<Record>(1);

  let read_fut = async move {
    let shutdown = tokio_util::sync::CancellationToken::new();
    let request_fn = Arc::new(request_fn);
    let read_channels = Arc::new(RwLock::new(HashMap::new()));
    while let Some(record) = tokio::select! {
      _ = shutdown.cancelled() => {
        return Ok(());
      }
      record = framed_read.next() => {
        record
      }
    } {
      let record = record?;
      let request_id = record.request_id;
      match RecordType::from_u8(record.record_type) {
        Some(RecordType::BeginRequest) => {
          let (tx, rx) = async_channel::unbounded::<Record>();
          match read_channels.write().entry(request_id) {
            std::collections::hash_map::Entry::Occupied(_) => {
              // Ignore duplicate request id
              continue;
            }
            std::collections::hash_map::Entry::Vacant(e) => {
              e.insert(tx.clone());
            }
          }

          if record.content.len() < 8 {
            // Incomplete BEGIN_REQUEST content, ignore
            continue;
          }
          let role = u16::from_be_bytes([record.content[0], record.content[1]]);

          if role != Role::Responder as u16 {
            let end_request = Record::new(
              RecordType::EndRequest as u8,
              request_id,
              vec![0, 0, 0, 0, ProtocolStatus::UnknownRole as u8, 0, 0, 0],
            );
            tx.send(end_request).await.ok();
          }

          let request_fn = Arc::clone(&request_fn);
          let read_channels = Arc::clone(&read_channels);
          let write_tx = write_tx.clone();
          let shutdown = if record.content[2] & FCGI_KEEP_CONN == 0 {
            Some(shutdown.clone())
          } else {
            None
          };
          runtime.spawn(async move {
            let _ = handle_fcgi_request(write_tx, rx, request_id, request_fn).await;
            let _ = read_channels.write().remove(&request_id);
            if let Some(shutdown) = shutdown {
              shutdown.cancel();
            }
            Ok::<(), std::io::Error>(())
          });
        }
        Some(RecordType::GetValues) => {
          let mut names: HashSet<String> = HashSet::new();
          let mut cursor = Cursor::new(record.content);
          while let Some(nvp) = NameValuePair::decode(&mut cursor)? {
            let name = String::from_utf8_lossy(&nvp.name).to_string();
            names.insert(name);
          }

          let new_content = names
            .into_iter()
            .filter_map(|name| {
              let value = match &*name {
                "FCGI_MAX_CONNS" => "65536".as_bytes().to_vec(),
                "FCGI_MAX_REQS" => "65536".as_bytes().to_vec(),
                "FCGI_MPXS_CONNS" => "1".as_bytes().to_vec(),
                _ => return None,
              };
              let nvp = NameValuePair {
                name: name.into_bytes(),
                value,
              };
              Some(nvp.encode())
            })
            .flatten()
            .collect::<Vec<u8>>();

          let record = Record {
            version: 1,
            record_type: RecordType::GetValuesResult as u8,
            request_id,
            content: new_content,
            padding_length: 0,
          };
          let _ = write_tx.send(record).await;
        }
        _ => {
          if let Some(tx) = read_channels.read().get(&request_id) {
            let _ = tx.try_send(record);
          }
        }
      }
    }
    Ok::<(), std::io::Error>(())
  };

  let write_fut = async move {
    while let Ok(record) = write_rx.recv().await {
      framed_write.send(record).await?;
    }
    Ok::<(), std::io::Error>(())
  };

  tokio::try_join!(read_fut, write_fut)?;

  Ok(())
}

async fn handle_fcgi_request<F, Fut, B, Err>(
  tx: async_channel::Sender<Record>,
  rx: async_channel::Receiver<Record>,
  request_id: u16,
  request_fn: Arc<F>,
) -> Result<(), std::io::Error>
where
  F: Fn(
    http::Request<
      CgiIncoming<StreamReader<std::pin::Pin<Box<async_channel::Receiver<Result<Bytes, std::io::Error>>>>, Bytes>>,
    >,
    SinkWriter<CopyToBytes<std::pin::Pin<Box<dyn Sink<Bytes, Error = std::io::Error> + Send + Sync>>>>,
  ) -> Fut,
  Fut: Future<Output = Result<http::Response<B>, Err>>,
  B: http_body::Body,
  B::Data: AsRef<[u8]> + Send + 'static,
  B::Error: Into<std::io::Error>,
  Err: Into<std::io::Error>,
{
  // 1. Accumulate PARAMS
  let mut params_data = Vec::new();
  loop {
    let record = rx
      .recv()
      .await
      .map_err(|_| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Connection closed before PARAMS end"))?;
    if record.request_id != request_id {
      continue;
    }
    if record.record_type == RecordType::Params as u8 {
      if record.content.is_empty() {
        break;
      }
      params_data.extend_from_slice(&record.content);
    } else if record.record_type == RecordType::AbortRequest as u8 {
      let end_request = Record::new(
        RecordType::EndRequest as u8,
        request_id,
        vec![0, 0, 0, 0, ProtocolStatus::RequestComplete as u8, 0, 0, 0],
      );
      tx.send(end_request).await.ok();
      return Ok(());
    }
  }

  let mut env_map = hashlink::LinkedHashMap::new();
  let mut cursor = Cursor::new(params_data);
  while let Some(nvp) = NameValuePair::decode(&mut cursor)? {
    let name = String::from_utf8_lossy(&nvp.name).to_string();
    let value = String::from_utf8_lossy(&nvp.value).to_string();
    env_map.insert(name, value);
  }
  let env = CgiEnvironment::from(env_map);

  // 2. Set up STDIN and STDERR streams
  let (stdin_tx, stdin_rx) = async_channel::bounded::<Result<Bytes, std::io::Error>>(1);
  let stdin_reader = StreamReader::new(Box::pin(stdin_rx));

  let rx_clone = rx.clone();
  let tx_clone = tx.clone();
  let stdin_handler_fut = async move {
    loop {
      let record = match rx_clone.recv().await {
        Ok(r) => r,
        Err(_) => break,
      };
      if record.request_id != request_id {
        continue;
      }
      if record.record_type == RecordType::Stdin as u8 {
        if record.content.is_empty() {
          break;
        }
        if stdin_tx.send(Ok(Bytes::from(record.content))).await.is_err() {
          break;
        }
      } else if record.record_type == RecordType::AbortRequest as u8 {
        let end_request = Record::new(
          RecordType::EndRequest as u8,
          request_id,
          vec![0, 0, 0, 0, ProtocolStatus::RequestComplete as u8, 0, 0, 0],
        );
        tx_clone.send(end_request).await.ok();
        break;
      }
    }
    futures_util::future::pending::<()>().await
  };

  let (stderr_tx, stderr_rx) = async_channel::bounded::<Bytes>(1);
  let tx_clone = tx.clone();
  let stderr_handler_fut = async move {
    while let Ok(chunk) = stderr_rx.recv().await {
      for part in chunk.chunks(65535) {
        let record = Record::new(RecordType::Stderr as u8, request_id, part.to_vec());
        tx_clone.send(record).await.ok();
      }
    }
    futures_util::future::pending::<()>().await
  };
  let stderr_sink: std::pin::Pin<Box<dyn Sink<Bytes, Error = std::io::Error> + Send + Sync>> =
    Box::pin(futures_util::sink::unfold(stderr_tx, |tx, chunk| async move {
      if tx.send(chunk).await.is_err() {
        Err(std::io::Error::other("stderr send error"))
      } else {
        Ok(tx)
      }
    }));
  let stderr_writer = SinkWriter::new(CopyToBytes::new(stderr_sink));

  let mut stdin_handler_fut = std::pin::pin!(stdin_handler_fut);
  let mut stderr_handler_fut = std::pin::pin!(stderr_handler_fut);

  let request = convert_cgi_request(stdin_reader, env)?;
  let response;
  tokio::select! {
      biased;

      _ = &mut stdin_handler_fut => {
          unreachable!()
      }
      _ = &mut stderr_handler_fut =>{
          unreachable!()
      }
      result = request_fn(request, stderr_writer) => {
          response = result.map_err(|e| e.into())?
      }
  };

  let mut cgi_response = convert_from_http_response(response)?;

  let tx_clone = tx.clone();
  let stdout_fut = async move {
    while let Some(Ok(chunk)) = cgi_response.next().await {
      for part in chunk.chunks(65535) {
        let record = Record::new(RecordType::Stdout as u8, request_id, part.to_vec());
        tx_clone.send(record).await.ok();
      }
    }

    // End of STDOUT
    tx_clone
      .send(Record::new(RecordType::Stdout as u8, request_id, vec![]))
      .await
      .ok();
  };

  tokio::select! {
    _ = &mut stdin_handler_fut => {
      unreachable!()
    }
    _ = &mut stderr_handler_fut =>{
      unreachable!()
    }
    _ = stdout_fut => {}
  }

  // END_REQUEST
  let end_request = Record::new(
    RecordType::EndRequest as u8,
    request_id,
    vec![0, 0, 0, 0, ProtocolStatus::RequestComplete as u8, 0, 0, 0],
  );
  tx.send(end_request).await.ok();

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::protocol::constants::{ProtocolStatus, RecordType, Role};
  use crate::protocol::record::Record;
  use http::{Response, StatusCode};
  use tokio_util::bytes::Bytes;

  pub struct TokioRuntime;

  impl Runtime for TokioRuntime {
    fn spawn(&self, future: impl Future + Send + 'static) {
      tokio::spawn(async move {
        future.await;
      });
    }
  }

  #[tokio::test]
  async fn test_server_handle_fcgi_simple() {
    use http_body_util::BodyExt;
    let (client_io, server_io) = tokio::io::duplex(1024);

    let handle = tokio::spawn(async move {
      server_handle_fcgi(server_io, TokioRuntime, |req, _| async move {
        assert_eq!(req.method(), http::Method::GET);
        assert_eq!(req.uri(), "/test");

        Ok::<_, std::io::Error>(
          Response::builder()
            .status(StatusCode::OK)
            .body(
              http_body_util::Full::new(Bytes::from("Hello from FastCGI"))
                .map_err(|e| -> std::io::Error { match e {} }),
            )
            .unwrap(),
        )
      })
      .await
      .unwrap();
    });

    let (client_reader, client_writer) = tokio::io::split(client_io);
    let mut client_read = FramedRead::new(client_reader, Decoder::default());
    let mut client_write = FramedWrite::new(client_writer, Encoder);

    // 1. Send BEGIN_REQUEST
    client_write
      .send(Record::new(
        RecordType::BeginRequest as u8,
        1,
        vec![0, Role::Responder as u8, 0, 0, 0, 0, 0, 0],
      ))
      .await
      .unwrap();

    // 2. Send PARAMS
    let mut params = Vec::new();
    params.extend_from_slice(&NameValuePair::new(b"REQUEST_METHOD".to_vec(), b"GET".to_vec()).encode());
    params.extend_from_slice(&NameValuePair::new(b"REQUEST_URI".to_vec(), b"/test".to_vec()).encode());

    client_write
      .send(Record::new(RecordType::Params as u8, 1, params))
      .await
      .unwrap();
    client_write
      .send(Record::new(RecordType::Params as u8, 1, vec![]))
      .await
      .unwrap();

    // 3. Send empty STDIN
    client_write
      .send(Record::new(RecordType::Stdin as u8, 1, vec![]))
      .await
      .unwrap();

    // 4. Receive STDOUT
    let mut stdout_data = Vec::new();
    loop {
      let record = client_read.next().await.unwrap().unwrap();
      if record.record_type == RecordType::Stdout as u8 {
        if record.content.is_empty() {
          break;
        }
        stdout_data.extend_from_slice(&record.content);
      } else if record.record_type == RecordType::EndRequest as u8 {
        panic!("Received EndRequest before empty Stdout");
      }
    }

    let response_str = String::from_utf8_lossy(&stdout_data);
    assert!(response_str.contains("Status: 200 OK"));
    assert!(response_str.contains("Hello from FastCGI"));

    // 5. Receive END_REQUEST
    let record = client_read.next().await.unwrap().unwrap();
    assert_eq!(record.record_type, RecordType::EndRequest as u8);
    assert_eq!(record.content[4], ProtocolStatus::RequestComplete as u8);

    handle.await.unwrap();
  }
}
