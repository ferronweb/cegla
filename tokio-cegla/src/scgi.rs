/// Tokio-based runtime for `cegla-scgi`
pub struct TokioScgiRuntime;

impl cegla_scgi::client::SendRuntime for TokioScgiRuntime {
  fn spawn(&self, future: impl std::future::Future + Send + 'static) {
    tokio::spawn(async move {
      future.await;
    });
  }
}
