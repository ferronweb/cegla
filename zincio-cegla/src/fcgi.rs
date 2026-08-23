/// `zincio`-based runtime for `cegla-fcgi`
pub struct ZincioFcgiRuntime;

impl cegla_fcgi::server::Runtime for ZincioFcgiRuntime {
  fn spawn(&self, future: impl std::future::Future + Send + 'static) {
    zincio::spawn(async move {
      future.await;
    });
  }
}
