use std::{path::PathBuf, process::Stdio};

use cegla_cgi::CgiEnvironment;

/// Tokio-based runtime for `cegla-cgi`
pub struct TokioCgiRuntime;

/// Tokio-based child process for `cegla-cgi`
pub struct TokioCgiChild {
  inner: tokio::process::Child,
}

impl cegla_cgi::client::SendRuntime for TokioCgiRuntime {
  type Child = TokioCgiChild;

  fn spawn(&self, future: impl std::future::Future + Send + 'static) {
    tokio::spawn(async move {
      future.await;
    });
  }

  fn start_child(
    &self,
    cmd: &std::ffi::OsStr,
    args: &[&std::ffi::OsStr],
    env: CgiEnvironment,
    cwd: Option<PathBuf>,
  ) -> Result<Self::Child, std::io::Error> {
    let mut command = tokio::process::Command::new(cmd);
    command
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .envs(env)
      .args(args);
    if let Some(cwd) = cwd {
      command.current_dir(cwd);
    }
    Ok(TokioCgiChild {
      inner: command.spawn()?,
    })
  }
}

impl cegla_cgi::client::SendChild for TokioCgiChild {
  type Stdin = tokio::process::ChildStdin;
  type Stdout = tokio::process::ChildStdout;
  type Stderr = tokio::process::ChildStderr;

  fn stdin(&mut self) -> Option<Self::Stdin> {
    self.inner.stdin.take()
  }

  fn stdout(&mut self) -> Option<Self::Stdout> {
    self.inner.stdout.take()
  }

  fn stderr(&mut self) -> Option<Self::Stderr> {
    self.inner.stderr.take()
  }

  fn try_status(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
    self.inner.try_wait()
  }
}
