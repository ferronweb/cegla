//! CGI implementation for web servers.

use std::{ffi::OsStr, future::Future, path::PathBuf};

pub use cegla::client::CgiBuilder;

use cegla::{
  client::{convert_to_http_response, CgiResponseInner},
  CgiEnvironment, CgiIncoming,
};
use http_body::Body;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::io::StreamReader;

/// Runtime trait for CGI "client".
pub trait Runtime {
  type Child: Child;

  /// Spawns a new task to execute the given future.
  fn spawn(&self, future: impl Future + 'static);

  /// Starts a child process with the given command and arguments.
  fn start_child(&self, cmd: &OsStr, args: &[&OsStr], env: CgiEnvironment) -> Result<Self::Child, std::io::Error>;
}

/// `Send` runtime trait for CGI "client".
pub trait SendRuntime {
  type Child: SendChild;

  /// Spawns a new task to execute the given future.
  fn spawn(&self, future: impl Future + Send + 'static);

  /// Starts a child process with the given command and arguments.
  fn start_child(
    &self,
    cmd: &OsStr,
    args: &[&OsStr],
    env: CgiEnvironment,
    cwd: Option<PathBuf>,
  ) -> Result<Self::Child, std::io::Error>;
}

/// Runtime trait for CGI child process.
pub trait Child {
  type Stdin: AsyncWrite + Unpin + 'static;
  type Stdout: AsyncRead + Unpin + 'static;
  type Stderr: AsyncRead + Unpin + 'static;

  /// Obtains the standard input stream.
  fn stdin(&mut self) -> Option<Self::Stdin>;

  /// Obtains the standard output stream.
  fn stdout(&mut self) -> Option<Self::Stdout>;

  /// Obtains the standard error stream.
  fn stderr(&mut self) -> Option<Self::Stderr>;

  /// Returns the exit status if the process has exited.
  fn try_status(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;
}

/// `Send` runtime trait for CGI child process.
pub trait SendChild {
  type Stdin: AsyncWrite + Send + Unpin + 'static;
  type Stdout: AsyncRead + Send + Unpin + 'static;
  type Stderr: AsyncRead + Send + Unpin + 'static;

  /// Obtains the standard input stream.
  fn stdin(&mut self) -> Option<Self::Stdin>;

  /// Obtains the standard output stream.
  fn stdout(&mut self) -> Option<Self::Stdout>;

  /// Obtains the standard error stream.
  fn stderr(&mut self) -> Option<Self::Stderr>;

  /// Returns the exit status if the process has exited.
  fn try_status(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;
}

/// Executes a CGI program, returning the response and error streams.
pub async fn execute_cgi<B, R>(
  request: http::Request<B>,
  runtime: R,
  cmd: &OsStr,
  args: &[&OsStr],
  env: CgiBuilder,
) -> Result<
  (
    http::Response<CgiIncoming<CgiResponseInner<<R::Child as Child>::Stdout>>>,
    Option<<R::Child as Child>::Stderr>,
  ),
  std::io::Error,
>
where
  B: Body + 'static,
  B::Data: AsRef<[u8]> + Send + 'static,
  B::Error: Into<std::io::Error>,
  R: Runtime,
{
  let (cgi_environment, cgi_data) = env.build(request);
  let mut child = runtime.start_child(cmd, args, cgi_environment)?;

  let mut stdin = child.stdin().ok_or(std::io::Error::other("Failed to take stdin"))?;
  let stdout = child.stdout().ok_or(std::io::Error::other("Failed to take stdout"))?;
  let stderr = child.stderr();

  let mut cgi_data_reader = StreamReader::new(cgi_data);
  runtime.spawn(async move {
    let _ = tokio::io::copy(&mut cgi_data_reader, &mut stdin).await;
  });

  Ok((convert_to_http_response(stdout).await?, stderr))
}

/// Executes a CGI program, on a `Send` runtime, returning the response and error streams.
pub async fn execute_cgi_send<B, R>(
  request: http::Request<B>,
  runtime: R,
  cmd: &OsStr,
  args: &[&OsStr],
  env: CgiBuilder,
  cwd: Option<PathBuf>,
) -> Result<
  (
    http::Response<CgiIncoming<CgiResponseInner<<R::Child as SendChild>::Stdout>>>,
    Option<<R::Child as SendChild>::Stderr>,
  ),
  std::io::Error,
>
where
  B: Body + Send + 'static,
  B::Data: AsRef<[u8]> + Send + 'static,
  B::Error: Into<std::io::Error>,
  R: SendRuntime,
{
  let (cgi_environment, cgi_data) = env.build(request);
  let mut child = runtime.start_child(cmd, args, cgi_environment, cwd)?;

  let mut stdin = child.stdin().ok_or(std::io::Error::other("Failed to take stdin"))?;
  let stdout = child.stdout().ok_or(std::io::Error::other("Failed to take stdout"))?;
  let stderr = child.stderr();

  let mut cgi_data_reader = StreamReader::new(cgi_data);
  runtime.spawn(async move {
    let _ = tokio::io::copy(&mut cgi_data_reader, &mut stdin).await;
  });

  Ok((convert_to_http_response(stdout).await?, stderr))
}
