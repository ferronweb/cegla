pub const FCGI_LISTENSOCK_FILENO: u8 = 0;
pub const FCGI_HEADER_LEN: usize = 8;
pub const FCGI_VERSION_1: u8 = 1;
pub const FCGI_NULL_REQUEST_ID: u16 = 0;
pub const FCGI_KEEP_CONN: u8 = 1;

pub const FCGI_MAX_CONNS: &str = "FCGI_MAX_CONNS";
pub const FCGI_MAX_REQS: &str = "FCGI_MAX_REQS";
pub const FCGI_MPXS_CONNS: &str = "FCGI_MPXS_CONNS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordType {
  BeginRequest = 1,
  AbortRequest = 2,
  EndRequest = 3,
  Params = 4,
  Stdin = 5,
  Stdout = 6,
  Stderr = 7,
  Data = 8,
  GetValues = 9,
  GetValuesResult = 10,
  UnknownType = 11,
}

impl RecordType {
  pub fn from_u8(value: u8) -> Option<Self> {
    match value {
      1 => Some(Self::BeginRequest),
      2 => Some(Self::AbortRequest),
      3 => Some(Self::EndRequest),
      4 => Some(Self::Params),
      5 => Some(Self::Stdin),
      6 => Some(Self::Stdout),
      7 => Some(Self::Stderr),
      8 => Some(Self::Data),
      9 => Some(Self::GetValues),
      10 => Some(Self::GetValuesResult),
      11 => Some(Self::UnknownType),
      _ => None,
    }
  }

  pub fn as_u8(self) -> u8 {
    self as u8
  }
}

impl std::fmt::Display for RecordType {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::BeginRequest => write!(f, "FCGI_BEGIN_REQUEST"),
      Self::AbortRequest => write!(f, "FCGI_ABORT_REQUEST"),
      Self::EndRequest => write!(f, "FCGI_END_REQUEST"),
      Self::Params => write!(f, "FCGI_PARAMS"),
      Self::Stdin => write!(f, "FCGI_STDIN"),
      Self::Stdout => write!(f, "FCGI_STDOUT"),
      Self::Stderr => write!(f, "FCGI_STDERR"),
      Self::Data => write!(f, "FCGI_DATA"),
      Self::GetValues => write!(f, "FCGI_GET_VALUES"),
      Self::GetValuesResult => write!(f, "FCGI_GET_VALUES_RESULT"),
      Self::UnknownType => write!(f, "FCGI_UNKNOWN_TYPE"),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Role {
  Responder = 1,
  Authorizer = 2,
  Filter = 3,
}

impl Role {
  pub fn from_u16(value: u16) -> Option<Self> {
    match value {
      1 => Some(Self::Responder),
      2 => Some(Self::Authorizer),
      3 => Some(Self::Filter),
      _ => None,
    }
  }

  pub fn as_u16(self) -> u16 {
    self as u16
  }
}

impl std::fmt::Display for Role {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::Responder => write!(f, "FCGI_RESPONDER"),
      Self::Authorizer => write!(f, "FCGI_AUTHORIZER"),
      Self::Filter => write!(f, "FCGI_FILTER"),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProtocolStatus {
  RequestComplete = 0,
  CantMpxConn = 1,
  Overloaded = 2,
  UnknownRole = 3,
}

impl ProtocolStatus {
  pub fn from_u8(value: u8) -> Option<Self> {
    match value {
      0 => Some(Self::RequestComplete),
      1 => Some(Self::CantMpxConn),
      2 => Some(Self::Overloaded),
      3 => Some(Self::UnknownRole),
      _ => None,
    }
  }

  pub fn as_u8(self) -> u8 {
    self as u8
  }
}

impl std::fmt::Display for ProtocolStatus {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::RequestComplete => write!(f, "FCGI_REQUEST_COMPLETE"),
      Self::CantMpxConn => write!(f, "FCGI_CANT_MPX_CONN"),
      Self::Overloaded => write!(f, "FCGI_OVERLOADED"),
      Self::UnknownRole => write!(f, "FCGI_UNKNOWN_ROLE"),
    }
  }
}
