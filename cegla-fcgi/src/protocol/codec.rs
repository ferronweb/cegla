use tokio_util::bytes::BufMut;
use tokio_util::codec::{Decoder as CodecDecoder, Encoder as CodecEncoder};

use crate::protocol::record::Record;

#[derive(Default)]
pub struct Encoder;

impl CodecEncoder<Record> for Encoder {
  type Error = std::io::Error;

  fn encode(&mut self, item: Record, dst: &mut tokio_util::bytes::BytesMut) -> Result<(), Self::Error> {
    dst.put_slice(&item.encode()?);
    Ok(())
  }
}

pub struct Decoder {
  current_buf: Vec<u8>,
}

impl Default for Decoder {
  fn default() -> Self {
    Self {
      current_buf: Vec::with_capacity(8),
    }
  }
}

impl CodecDecoder for Decoder {
  type Item = Record;
  type Error = std::io::Error;

  fn decode(&mut self, src: &mut tokio_util::bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
    if self.current_buf.len() < 8 {
      if src.len() >= 8 {
        self.current_buf.extend(src.split_to(8));
      } else {
        return Ok(None);
      }
    }

    if self.current_buf.len() >= 8 {
      let content_length = u16::from_be_bytes(
        self.current_buf[4..6]
          .try_into()
          .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?,
      ) as usize;
      let padding_length = self.current_buf[6] as usize;
      if src.len() >= content_length + padding_length {
        self.current_buf.extend(src.split_to(content_length + padding_length));
        let record = Record::decode(&mut &*self.current_buf);
        self.current_buf.clear();
        return record;
      }
    }

    Ok(None)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_encoder_single_record() {
    let mut encoder = Encoder;
    let record = Record::new(1, 1, b"test".to_vec());
    let mut dst = tokio_util::bytes::BytesMut::new();
    encoder.encode(record, &mut dst).unwrap();
    let expected = vec![
      1, 1, 0, 1, 0, 4, 4, 0, // header
      b't', b'e', b's', b't', // content
      0, 0, 0, 0, // padding
    ];
    assert_eq!(dst.to_vec(), expected);
  }

  #[test]
  fn test_encoder_empty_content() {
    let mut encoder = Encoder;
    let record = Record::new(3, 100, vec![]);
    let mut dst = tokio_util::bytes::BytesMut::new();
    encoder.encode(record, &mut dst).unwrap();
    let expected = vec![
      1, 3, 0, 100, 0, 0, 0, 0, // header, no padding needed
    ];
    assert_eq!(dst.to_vec(), expected);
  }

  #[test]
  fn test_decoder_single_record() {
    let mut decoder = Decoder::default();
    let data = tokio_util::bytes::BytesMut::from(
      &[
        1, 1, 0, 1, 0, 5, 3, 0, // header
        b'H', b'e', b'l', b'l', b'o', // content (5 bytes)
        0, 0, 0, // padding (3 bytes)
      ][..],
    );
    let result = decoder.decode(&mut data.clone()).unwrap();
    assert!(result.is_some());
    let record = result.unwrap();
    assert_eq!(record.version, 1);
    assert_eq!(record.record_type, 1);
    assert_eq!(record.request_id, 1);
    assert_eq!(record.content, b"Hello");
  }

  #[test]
  fn test_decoder_incomplete_header() {
    let mut decoder = Decoder::default();
    let mut data = tokio_util::bytes::BytesMut::from(&[1, 1, 0][..]);
    let result = decoder.decode(&mut data).unwrap();
    assert!(result.is_none());
  }

  #[test]
  fn test_decoder_incomplete_content() {
    let mut decoder = Decoder::default();
    let mut data = tokio_util::bytes::BytesMut::from(
      &[
        1, 1, 0, 1, 0, 10, 0, 0, // header says 10 bytes content
      ][..],
    );
    let result = decoder.decode(&mut data).unwrap();
    assert!(result.is_none());
  }

  #[test]
  fn test_encoder_decoder_roundtrip() {
    let mut encoder = Encoder;
    let record = Record::new(4, 42, b"FCGI_PARAMS data".to_vec());
    let mut encoded = tokio_util::bytes::BytesMut::new();
    encoder.encode(record, &mut encoded).unwrap();

    let mut decoder = Decoder::default();
    let decoded = decoder.decode(&mut encoded).unwrap().unwrap();
    assert_eq!(decoded.version, 1);
    assert_eq!(decoded.record_type, 4);
    assert_eq!(decoded.request_id, 42);
    assert_eq!(decoded.content, b"FCGI_PARAMS data");
  }

  #[test]
  fn test_decoder_multiple_records() {
    let mut decoder = Decoder::default();
    let mut data = tokio_util::bytes::BytesMut::from(
      &[
        // First record
        1, 1, 0, 1, 0, 4, 0, 0, b't', b'e', b's', b't', // Second record
        1, 3, 0, 2, 0, 5, 3, 0, b'h', b'e', b'l', b'l', b'o', 0, 0, 0,
      ][..],
    );

    let first = decoder.decode(&mut data).unwrap().unwrap();
    assert_eq!(first.record_type, 1);
    assert_eq!(first.content, b"test");

    let second = decoder.decode(&mut data).unwrap().unwrap();
    assert_eq!(second.record_type, 3);
    assert_eq!(second.content, b"hello");
  }

  #[test]
  fn test_decoder_no_padding() {
    let mut decoder = Decoder::default();
    let mut data = tokio_util::bytes::BytesMut::from(
      &[
        1, 6, 0, 1, 0, 8, 0, 0, // content length 8, padding 0
        b's', b't', b'd', b'o', b'u', b't', b'd', b'a', // 8 bytes
      ][..],
    );
    let result = decoder.decode(&mut data).unwrap().unwrap();
    assert_eq!(result.content, b"stdoutda");
    assert_eq!(result.padding_length, 0);
  }
}
