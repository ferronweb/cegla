use std::io::Read;

pub struct Record {
  pub version: u8,
  pub record_type: u8,
  pub request_id: u16,
  pub content: Vec<u8>,
  pub padding_length: u8,
}

impl Record {
  pub fn new(record_type: u8, request_id: u16, content: Vec<u8>) -> Self {
    let content_length = content.len() as u16;
    let padding_length = match (content_length % 8) as u8 {
      0 => 0,
      remainder => 8 - remainder,
    };

    Self {
      version: 1,
      record_type,
      request_id,
      content,
      padding_length,
    }
  }

  pub fn encode(&self) -> Result<Vec<u8>, std::io::Error> {
    let content_length: u16 = self
      .content
      .len()
      .try_into()
      .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "content length too large"))?;
    let padding_length = self.padding_length;

    let mut record = Vec::with_capacity(8 + content_length as usize + padding_length as usize);

    record.push(self.version);
    record.extend_from_slice(&self.record_type.to_be_bytes());
    record.extend_from_slice(&self.request_id.to_be_bytes());
    record.extend_from_slice(&content_length.to_be_bytes());
    record.extend_from_slice(&padding_length.to_be_bytes());
    record.push(0);

    record.extend_from_slice(&self.content);
    record.extend(vec![0u8; padding_length as usize]);

    Ok(record)
  }

  pub fn decode<R: Read>(reader: &mut R) -> std::io::Result<Option<Self>> {
    let mut header = [0u8; 8];
    if reader.read_exact(&mut header).is_err() {
      return Ok(None);
    }

    let version = header[0];
    let record_type = header[1];
    let request_id = u16::from_be_bytes([header[2], header[3]]);
    let content_length = u16::from_be_bytes([header[4], header[5]]);
    let padding_length = header[6];

    let mut content = vec![0u8; content_length as usize];
    reader.read_exact(&mut content)?;

    let mut padding = vec![0u8; padding_length as usize];
    reader.read_exact(&mut padding)?;

    Ok(Some(Self {
      version,
      record_type,
      request_id,
      content,
      padding_length,
    }))
  }

  pub fn content_length(&self) -> u16 {
    self.content.len() as u16
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_empty_content() {
    let record = Record::new(1, 1234, vec![]);
    let expected = vec![1, 1, 4, 210, 0, 0, 0, 0];
    assert_eq!(record.encode().unwrap(), expected);
  }

  #[test]
  fn test_content_length_5() {
    let record = Record::new(2, 5678, b"Hello".to_vec());
    let expected = vec![1, 2, 22, 46, 0, 5, 3, 0, 72, 101, 108, 108, 111, 0, 0, 0];
    assert_eq!(record.encode().unwrap(), expected);
  }

  #[test]
  fn test_content_length_8_no_padding() {
    let record = Record::new(3, 9012, b"12345678".to_vec());
    let expected = vec![1, 3, 35, 52, 0, 8, 0, 0, 49, 50, 51, 52, 53, 54, 55, 56];
    assert_eq!(record.encode().unwrap(), expected);
  }

  #[test]
  fn test_content_length_too_long() {
    let record = Record::new(4, 1234, vec![0u8; 131072]); // 131072 bytes is too long for a single record
    assert!(record.encode().is_err());
  }
}
