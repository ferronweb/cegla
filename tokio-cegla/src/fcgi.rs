/// Tokio-based runtime for `cegla-fcgi`
pub struct TokioFcgiRuntime;

impl cegla_fcgi::server::Runtime for TokioFcgiRuntime {
  fn spawn(&self, future: impl std::future::Future + Send + 'static) {
    tokio::spawn(async move {
      future.await;
    });
  }
}
