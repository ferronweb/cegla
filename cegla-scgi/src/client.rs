//! Client-side SCGI implementation.

use std::future::Future;

pub use cegla::client::CgiBuilder;

use cegla::{
  client::{convert_to_http_response, CgiResponseInner},
  CgiIncoming,
};
use http_body::Body;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadHalf};
use tokio_util::io::StreamReader;

/// Runtime trait for SCGI client.
pub trait Runtime {
  /// Spawns a new task to execute the given future.
  fn spawn(&self, future: impl Future + 'static);
}

/// `Send` runtime trait for SCGI client.
pub trait SendRuntime {
  /// Spawns a new task to execute the given future.
  fn spawn(&self, future: impl Future + Send + 'static);
}

/// Handles SCGI client-side, returning the response
pub async fn client_handle_scgi<B, R, Io>(
  request: http::Request<B>,
  runtime: R,
  io: Io,
  env: CgiBuilder,
) -> Result<http::Response<CgiIncoming<CgiResponseInner<ReadHalf<Io>>>>, std::io::Error>
where
  B: Body + 'static,
  B::Data: AsRef<[u8]> + Send + 'static,
  B::Error: Into<std::io::Error>,
  R: Runtime,
  Io: AsyncRead + AsyncWrite + Unpin + 'static,
{
  let (cgi_environment, cgi_data) = env
    .var("SCGI".to_string(), "1".to_string())
    .var_noreplace("CONTENT_LENGTH".to_string(), "0".to_string())
    .build(request);
  let (read_half, mut write_half) = tokio::io::split(io);

  // Create environment variable netstring
  let mut environment_variables_to_wrap = Vec::new();
  for (key, value) in cgi_environment {
    let mut environment_variable = Vec::new();
    let is_content_length = key == "CONTENT_LENGTH";
    environment_variable.extend(key.into_bytes());
    environment_variable.push(b'\0');
    environment_variable.extend(value.into_bytes());
    environment_variable.push(b'\0');
    if is_content_length {
      environment_variable.append(&mut environment_variables_to_wrap);
      environment_variables_to_wrap = environment_variable;
    } else {
      environment_variables_to_wrap.append(&mut environment_variable);
    }
  }

  let environment_variables_to_wrap_length = environment_variables_to_wrap.len();
  let mut environment_variables_netstring = Vec::new();
  environment_variables_netstring.extend_from_slice(environment_variables_to_wrap_length.to_string().as_bytes());
  environment_variables_netstring.push(b':');
  environment_variables_netstring.append(&mut environment_variables_to_wrap);
  environment_variables_netstring.push(b',');

  // Write environment variable netstring
  write_half.write_all(&environment_variables_netstring).await?;

  let mut stdin = write_half;
  let stdout = read_half;

  let mut cgi_data_reader = StreamReader::new(cgi_data);
  runtime.spawn(async move {
    let _ = tokio::io::copy(&mut cgi_data_reader, &mut stdin).await;
  });

  convert_to_http_response(stdout).await
}

/// Handles SCGI client-side, on a `Send` runtime, returning the response
pub async fn client_handle_scgi_send<B, R, Io>(
  request: http::Request<B>,
  runtime: R,
  io: Io,
  env: CgiBuilder,
) -> Result<http::Response<CgiIncoming<CgiResponseInner<ReadHalf<Io>>>>, std::io::Error>
where
  B: Body + Send + 'static,
  B::Data: AsRef<[u8]> + Send + 'static,
  B::Error: Into<std::io::Error>,
  R: SendRuntime,
  Io: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
  let (cgi_environment, cgi_data) = env
    .var("SCGI".to_string(), "1".to_string())
    .var_noreplace("CONTENT_LENGTH".to_string(), "0".to_string())
    .build(request);
  let (read_half, mut write_half) = tokio::io::split(io);

  // Create environment variable netstring
  let mut environment_variables_to_wrap = Vec::new();
  for (key, value) in cgi_environment {
    let mut environment_variable = Vec::new();
    let is_content_length = key == "CONTENT_LENGTH";
    environment_variable.extend(key.into_bytes());
    environment_variable.push(b'\0');
    environment_variable.extend(value.into_bytes());
    environment_variable.push(b'\0');
    if is_content_length {
      environment_variable.append(&mut environment_variables_to_wrap);
      environment_variables_to_wrap = environment_variable;
    } else {
      environment_variables_to_wrap.append(&mut environment_variable);
    }
  }

  let environment_variables_to_wrap_length = environment_variables_to_wrap.len();
  let mut environment_variables_netstring = Vec::new();
  environment_variables_netstring.extend_from_slice(environment_variables_to_wrap_length.to_string().as_bytes());
  environment_variables_netstring.push(b':');
  environment_variables_netstring.append(&mut environment_variables_to_wrap);
  environment_variables_netstring.push(b',');

  // Write environment variable netstring
  write_half.write_all(&environment_variables_netstring).await?;

  let mut stdin = write_half;
  let stdout = read_half;

  let mut cgi_data_reader = StreamReader::new(cgi_data);
  runtime.spawn(async move {
    let _ = tokio::io::copy(&mut cgi_data_reader, &mut stdin).await;
  });

  convert_to_http_response(stdout).await
}
