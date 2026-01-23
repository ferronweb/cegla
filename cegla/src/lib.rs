//! A low-level parsing library for CGI (and CGI-like protocols)

use std::{
  collections::HashMap,
  pin::Pin,
  task::{Context, Poll},
};

use bytes::Bytes;
use futures_util::Stream;
use hashlink::LinkedHashMap;
use http_body::Body;
use tokio_util::io::ReaderStream;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "server")]
pub mod server;

/// A CGI-like incoming body
#[derive(Debug)]
pub struct CgiIncoming<R> {
  pub(crate) inner: Pin<Box<ReaderStream<R>>>,
}

#[allow(unused)]
impl<R> CgiIncoming<R>
where
  R: tokio::io::AsyncRead,
{
  /// Creates a new instance of `CgiIncoming`
  pub(crate) fn new(inner: R) -> Self {
    Self {
      inner: Box::pin(ReaderStream::new(inner)),
    }
  }
}

impl<R> Body for CgiIncoming<R>
where
  R: tokio::io::AsyncRead,
{
  type Data = Bytes;
  type Error = std::io::Error;

  fn poll_frame(
    mut self: Pin<&mut Self>,
    cx: &mut Context<'_>,
  ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
    match Pin::new(&mut self.inner).poll_next(cx) {
      Poll::Ready(Some(Ok(data))) => Poll::Ready(Some(Ok(http_body::Frame::data(data)))),
      Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err))),
      Poll::Ready(None) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}

/// A map of CGI environment variables.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CgiEnvironment {
  pub(crate) inner: hashlink::LinkedHashMap<String, String>,
}

impl CgiEnvironment {
  /// Obtains the value of the specified environment variable.
  pub fn get(&self, key: &str) -> Option<&str> {
    let key = key.to_uppercase();
    self.inner.get(&key).map(|value| value.as_str())
  }

  /// Determines whether the specified environment variable exists.
  pub fn contains_key(&self, key: &str) -> bool {
    let key = key.to_uppercase();
    self.inner.contains_key(&key)
  }

  /// Obtains an iterator over the environment variables.
  pub fn iter<'a>(&'a self) -> hashlink::linked_hash_map::Iter<'a, String, String> {
    self.inner.iter()
  }

  /// Returns the total number of environment variables.
  pub fn len(&self) -> usize {
    self.inner.len()
  }

  /// Checks whether the environment is empty.
  pub fn is_empty(&self) -> bool {
    self.inner.is_empty()
  }
}

impl std::ops::Index<&str> for CgiEnvironment {
  type Output = str;

  fn index(&self, key: &str) -> &Self::Output {
    let key = key.to_uppercase();
    self
      .inner
      .get(&key)
      .unwrap_or_else(|| panic!("Missing environment variable: {}", key))
      .as_str()
  }
}

impl IntoIterator for CgiEnvironment {
  type Item = (String, String);
  type IntoIter = hashlink::linked_hash_map::IntoIter<String, String>;

  fn into_iter(self) -> Self::IntoIter {
    self.inner.into_iter()
  }
}

impl<'a> IntoIterator for &'a CgiEnvironment {
  type Item = (&'a String, &'a String);
  type IntoIter = hashlink::linked_hash_map::Iter<'a, String, String>;

  fn into_iter(self) -> Self::IntoIter {
    self.inner.iter()
  }
}

impl From<LinkedHashMap<String, String>> for CgiEnvironment {
  fn from(map: LinkedHashMap<String, String>) -> Self {
    Self { inner: map }
  }
}

impl From<HashMap<String, String>> for CgiEnvironment {
  fn from(map: HashMap<String, String>) -> Self {
    Self {
      inner: map.into_iter().collect(),
    }
  }
}

impl FromIterator<(String, String)> for CgiEnvironment {
  fn from_iter<T: IntoIterator<Item = (String, String)>>(iter: T) -> Self {
    Self {
      inner: iter.into_iter().collect(),
    }
  }
}
