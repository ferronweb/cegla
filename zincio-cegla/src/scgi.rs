/// `zincio`-based runtime for `cegla-scgi`
pub struct ZincioScgiRuntime;

impl cegla_scgi::client::Runtime for ZincioScgiRuntime {
  fn spawn(&self, future: impl std::future::Future + 'static) {
    zincio::spawn(async move {
      future.await;
    });
  }
}
