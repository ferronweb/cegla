use std::{path::PathBuf, process::Stdio};

use cegla_cgi::CgiEnvironment;

/// `vibeio`-based runtime for `cegla-cgi`
pub struct VibeioCgiRuntime;

/// `vibeio`-based child process for `cegla-cgi`
pub struct VibeioCgiChild {
  inner: vibeio::process::Child,
}

impl cegla_cgi::client::Runtime for VibeioCgiRuntime {
  type Child = VibeioCgiChild;

  fn spawn(&self, future: impl std::future::Future + 'static) {
    vibeio::spawn(async move {
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
    let mut command = vibeio::process::Command::new(cmd);
    command
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .envs(env)
      .args(args);
    if let Some(cwd) = cwd {
      command.current_dir(cwd);
    }
    Ok(VibeioCgiChild {
      inner: command.spawn()?,
    })
  }
}

impl cegla_cgi::client::Child for VibeioCgiChild {
  type Stdin = vibeio::util::AsyncWrap<vibeio::process::ChildStdin>;
  type Stdout = vibeio::util::AsyncWrap<vibeio::process::ChildStdout>;
  type Stderr = vibeio::util::AsyncWrap<vibeio::process::ChildStderr>;

  fn stdin(&mut self) -> Option<Self::Stdin> {
    self.inner.stdin.take().map(vibeio::util::AsyncWrap::new)
  }

  fn stdout(&mut self) -> Option<Self::Stdout> {
    self.inner.stdout.take().map(vibeio::util::AsyncWrap::new)
  }

  fn stderr(&mut self) -> Option<Self::Stderr> {
    self.inner.stderr.take().map(vibeio::util::AsyncWrap::new)
  }

  fn try_status(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
    self.inner.try_wait()
  }
}
