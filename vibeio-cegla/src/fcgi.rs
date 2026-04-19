/// `vibeio`-based runtime for `cegla-fcgi`
pub struct VibeioFcgiRuntime;

impl cegla_fcgi::server::Runtime for VibeioFcgiRuntime {
  fn spawn(&self, future: impl std::future::Future + Send + 'static) {
    vibeio::spawn(async move {
      future.await;
    });
  }
}
