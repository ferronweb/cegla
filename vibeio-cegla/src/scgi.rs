/// `vibeio`-based runtime for `cegla-scgi`
pub struct VibeioScgiRuntime;

impl cegla_scgi::client::Runtime for VibeioScgiRuntime {
  fn spawn(&self, future: impl std::future::Future + 'static) {
    vibeio::spawn(async move {
      future.await;
    });
  }
}
