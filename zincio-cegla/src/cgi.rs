use std::{path::PathBuf, process::Stdio};

use cegla_cgi::CgiEnvironment;

/// `zincio`-based runtime for `cegla-cgi`
pub struct ZincioCgiRuntime;

/// `zincio`-based child process for `cegla-cgi`
pub struct ZincioCgiChild {
  inner: zincio::process::Child,
}

impl cegla_cgi::client::Runtime for ZincioCgiRuntime {
  type Child = ZincioCgiChild;

  fn spawn(&self, future: impl std::future::Future + 'static) {
    zincio::spawn(async move {
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
    let mut command = zincio::process::Command::new(cmd);
    command
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .envs(env)
      .args(args);
    if let Some(cwd) = cwd {
      command.current_dir(cwd);
    }
    Ok(ZincioCgiChild {
      inner: command.spawn()?,
    })
  }
}

impl cegla_cgi::client::Child for ZincioCgiChild {
  type Stdin = zincio::util::AsyncWrap<zincio::process::ChildStdin>;
  type Stdout = zincio::util::AsyncWrap<zincio::process::ChildStdout>;
  type Stderr = zincio::util::AsyncWrap<zincio::process::ChildStderr>;

  fn stdin(&mut self) -> Option<Self::Stdin> {
    self.inner.stdin.take().map(zincio::util::AsyncWrap::new)
  }

  fn stdout(&mut self) -> Option<Self::Stdout> {
    self.inner.stdout.take().map(zincio::util::AsyncWrap::new)
  }

  fn stderr(&mut self) -> Option<Self::Stderr> {
    self.inner.stderr.take().map(zincio::util::AsyncWrap::new)
  }

  fn try_status(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
    self.inner.try_wait()
  }
}
