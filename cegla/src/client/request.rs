use std::{net::SocketAddr, path::PathBuf, pin::Pin, task::Poll};

use bytes::Bytes;
use futures_util::Stream;
use http_body::Body;

use crate::CgiEnvironment;

/// A builder for CGI-like requests
pub struct CgiBuilder {
  inner: hashlink::LinkedHashMap<String, String>,
  request_uri_set: bool,
}

impl CgiBuilder {
  /// Creates a new `CgiBuilder` instance
  pub fn new() -> Self {
    Self {
      inner: hashlink::LinkedHashMap::new(),
      request_uri_set: false,
    }
  }

  /// Inserts an environment variable
  pub fn var(mut self, key: String, value: String) -> Self {
    self.inner.insert(key.to_uppercase(), value);
    self
  }

  /// Inserts an environment variable if it doesn't already exist
  pub fn var_noreplace(mut self, key: String, value: String) -> Self {
    if let hashlink::linked_hash_map::Entry::Vacant(entry) = self.inner.entry(key.to_uppercase()) {
      entry.insert(value);
    }
    self
  }

  /// Inserts HTTP authentication-related data
  pub fn auth(mut self, auth_type: Option<String>, username: String) -> Self {
    if let Some(auth_type) = auth_type {
      self.inner.insert("AUTH_TYPE".to_string(), auth_type);
    }
    self.inner.insert("REMOTE_USER".to_string(), username);
    self
  }

  /// Inserts server software information
  pub fn server(mut self, server_software: String) -> Self {
    self.inner.insert("SERVER_SOFTWARE".to_string(), server_software);
    self
  }

  /// Inserts server administrator information
  pub fn server_admin(mut self, server_admin: String) -> Self {
    self.inner.insert("SERVER_ADMIN".to_string(), server_admin);
    self
  }

  /// Inserts server address information
  pub fn server_address(mut self, server_address: SocketAddr) -> Self {
    self.inner.insert(
      "SERVER_ADDR".to_string(),
      server_address.ip().to_canonical().to_string(),
    );
    self
      .inner
      .insert("SERVER_PORT".to_string(), server_address.port().to_string());
    self
  }

  /// Inserts client address information
  pub fn client_address(mut self, client_address: SocketAddr) -> Self {
    self.inner.insert(
      "REMOTE_ADDR".to_string(),
      client_address.ip().to_canonical().to_string(),
    );
    self
      .inner
      .insert("REMOTE_PORT".to_string(), client_address.port().to_string());
    self
  }

  /// Inserts server hostname information
  pub fn hostname(mut self, server_name: String) -> Self {
    self.inner.insert("SERVER_NAME".to_string(), server_name);
    self
  }

  /// Inserts script path information
  pub fn script_path(mut self, script_path: PathBuf, wwwroot: PathBuf, path_info: Option<String>) -> Self {
    self
      .inner
      .insert("SCRIPT_FILENAME".to_string(), script_path.to_string_lossy().to_string());
    if let Ok(script_path) = script_path.as_path().strip_prefix(&wwwroot) {
      self.inner.insert(
        "SCRIPT_NAME".to_string(),
        format!(
          "/{}",
          match cfg!(windows) {
            true => script_path.to_string_lossy().to_string().replace("\\", "/"),
            false => script_path.to_string_lossy().to_string(),
          }
        ),
      );
    }
    self
      .inner
      .insert("DOCUMENT_ROOT".to_string(), wwwroot.to_string_lossy().to_string());
    self.inner.insert(
      "PATH_INFO".to_string(),
      match &path_info {
        Some(path_info) => format!("/{path_info}"),
        None => "".to_string(),
      },
    );
    self.inner.insert(
      "PATH_TRANSLATED".to_string(),
      match &path_info {
        Some(path_info) => {
          let mut path_translated = script_path.clone();
          path_translated.push(path_info);
          path_translated.to_string_lossy().to_string()
        }
        None => "".to_string(),
      },
    );
    self
  }

  /// Sets the HTTPS environment variable to "on"
  pub fn https(mut self) -> Self {
    self.inner.insert("HTTPS".to_string(), "on".to_string());
    self
  }

  /// Sets the REQUEST_URI environment variable
  pub fn request_uri(mut self, uri: &http::Uri) -> Self {
    self.request_uri_set = true;
    self.inner.insert(
      "REQUEST_URI".to_string(),
      format!(
        "{}{}",
        uri.path(),
        match uri.query() {
          Some(query) => format!("?{query}"),
          None => String::from(""),
        }
      ),
    );
    self
  }

  /// Inserts environment variables from the system
  pub fn system(mut self) -> Self {
    for (key, value) in std::env::vars_os() {
      if let Ok(key) = key.into_string() {
        if let Ok(value) = value.into_string() {
          self.inner.insert(key, value);
        }
      }
    }
    self
  }

  /// Builds the CGI request
  pub fn build<B>(mut self, request: http::Request<B>) -> (CgiEnvironment, CgiRequest<B>)
  where
    B: Body,
    B::Data: AsRef<[u8]> + Send + 'static,
    B::Error: Into<std::io::Error>,
  {
    let (parts, body) = request.into_parts();
    self.inner.insert(
      "QUERY_STRING".to_string(),
      match parts.uri.query() {
        Some(query) => query.to_string(),
        None => "".to_string(),
      },
    );
    self
      .inner
      .insert("REQUEST_METHOD".to_string(), parts.method.to_string());
    if !self.request_uri_set {
      self.inner.insert(
        "REQUEST_URI".to_string(),
        format!(
          "{}{}",
          parts.uri.path(),
          match parts.uri.query() {
            Some(query) => format!("?{query}"),
            None => String::from(""),
          }
        ),
      );
    }
    self.inner.insert(
      "SERVER_PROTOCOL".to_string(),
      match parts.version {
        http::Version::HTTP_09 => "HTTP/0.9".to_string(),
        http::Version::HTTP_10 => "HTTP/1.0".to_string(),
        http::Version::HTTP_11 => "HTTP/1.1".to_string(),
        http::Version::HTTP_2 => "HTTP/2.0".to_string(),
        http::Version::HTTP_3 => "HTTP/3.0".to_string(),
        _ => "HTTP/Unknown".to_string(),
      },
    );
    for (header_name, header_value) in parts.headers.iter() {
      let env_header_name = match *header_name {
        http::header::CONTENT_LENGTH => "CONTENT_LENGTH".to_string(),
        http::header::CONTENT_TYPE => "CONTENT_TYPE".to_string(),
        _ => {
          let mut result = String::new();

          result.push_str("HTTP_");

          for c in header_name.as_str().to_uppercase().chars() {
            if c.is_alphanumeric() {
              result.push(c);
            } else {
              result.push('_');
            }
          }

          result
        }
      };
      if self.inner.contains_key(&env_header_name) {
        let value = self.inner.get_mut(&env_header_name);
        if let Some(value) = value {
          if env_header_name == "HTTP_COOKIE" {
            value.push_str("; ");
          } else {
            // See https://stackoverflow.com/a/1801191
            value.push_str(", ");
          }
          value.push_str(String::from_utf8_lossy(header_value.as_bytes()).as_ref());
        } else {
          self.inner.insert(
            env_header_name,
            String::from_utf8_lossy(header_value.as_bytes()).to_string(),
          );
        }
      } else {
        self.inner.insert(
          env_header_name,
          String::from_utf8_lossy(header_value.as_bytes()).to_string(),
        );
      }
    }
    self
      .inner
      .insert("GATEWAY_INTERFACE".to_string(), "CGI/1.1".to_string());
    (
      CgiEnvironment { inner: self.inner },
      CgiRequest { body: Box::pin(body) },
    )
  }
}

impl Default for CgiBuilder {
  fn default() -> Self {
    Self::new()
  }
}

/// A CGI-like client request
pub struct CgiRequest<B> {
  body: Pin<Box<B>>,
}

impl<B> Stream for CgiRequest<B>
where
  B: Body,
  B::Data: AsRef<[u8]> + Send + 'static,
  B::Error: Into<std::io::Error>,
{
  type Item = Result<Bytes, std::io::Error>;

  fn poll_next(
    mut self: std::pin::Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<Option<Self::Item>> {
    match Pin::new(&mut self.body).poll_frame(cx) {
      Poll::Ready(Some(Ok(frame))) => {
        if let Ok(data) = frame.into_data() {
          Poll::Ready(Some(Ok(Bytes::from_owner(data))))
        } else {
          Poll::Ready(None)
        }
      }
      Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err.into()))),
      Poll::Ready(None) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}
