use std::io::Read;

pub struct NameValuePair {
  pub name: Vec<u8>,
  pub value: Vec<u8>,
}

impl NameValuePair {
  pub fn new(name: Vec<u8>, value: Vec<u8>) -> Self {
    Self { name, value }
  }

  pub fn from_slices(name: &[u8], value: &[u8]) -> Self {
    Self {
      name: name.to_vec(),
      value: value.to_vec(),
    }
  }

  pub fn encode(&self) -> Vec<u8> {
    let name_length = self.name.len();
    let value_length = self.value.len();

    let mut name_value_pair = Vec::with_capacity(
      if name_length < 128 { 1 } else { 4 } + if value_length < 128 { 1 } else { 4 } + name_length + value_length,
    );

    if name_length < 128 {
      name_value_pair.extend_from_slice(&(name_length as u8).to_be_bytes());
    } else {
      name_value_pair.extend_from_slice(&((name_length as u32) | 0x80000000).to_be_bytes());
    }

    if value_length < 128 {
      name_value_pair.extend_from_slice(&(value_length as u8).to_be_bytes());
    } else {
      name_value_pair.extend_from_slice(&((value_length as u32) | 0x80000000).to_be_bytes());
    }

    name_value_pair.extend_from_slice(&self.name);
    name_value_pair.extend_from_slice(&self.value);

    name_value_pair
  }

  pub fn decode<R: Read>(reader: &mut R) -> std::io::Result<Option<Self>> {
    let mut name_len_buf = [0u8; 1];
    let mut value_len_buf = [0u8; 1];

    if reader.read_exact(&mut name_len_buf).is_err() {
      return Ok(None);
    }

    let name_len = if name_len_buf[0] & 0x80 == 0 {
      name_len_buf[0] as usize
    } else {
      let mut name_len_bytes = [0u8; 4];
      name_len_bytes[0] = name_len_buf[0];
      reader.read_exact(&mut name_len_bytes[1..])?;
      (u32::from_be_bytes(name_len_bytes) & 0x7FFFFFFF) as usize
    };

    if reader.read_exact(&mut value_len_buf).is_err() {
      return Ok(None);
    }

    let value_len = if value_len_buf[0] & 0x80 == 0 {
      value_len_buf[0] as usize
    } else {
      let mut value_len_bytes = [0u8; 4];
      value_len_bytes[0] = value_len_buf[0];
      reader.read_exact(&mut value_len_bytes[1..])?;
      (u32::from_be_bytes(value_len_bytes) & 0x7FFFFFFF) as usize
    };

    let mut name = vec![0u8; name_len];
    let mut value = vec![0u8; value_len];

    reader.read_exact(&mut name)?;
    reader.read_exact(&mut value)?;

    Ok(Some(Self { name, value }))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_short_name_and_value() {
    let name_value = NameValuePair::from_slices(b"HOST", b"localhost");
    let expected = vec![
      0x04, 0x09, b'H', b'O', b'S', b'T', b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't',
    ];
    assert_eq!(name_value.encode(), expected);
  }

  #[test]
  fn test_long_name_and_value() {
    let name = vec![b'N'; 130];
    let value = vec![b'V'; 135];
    let name_value = NameValuePair::new(name.clone(), value.clone());
    let mut expected = vec![0x80, 0x00, 0x00, 0x82, 0x80, 0x00, 0x00, 0x87];
    expected.extend_from_slice(&name);
    expected.extend_from_slice(&value);
    assert_eq!(name_value.encode(), expected);
  }

  #[test]
  fn test_empty_name_and_value() {
    let name_value = NameValuePair::from_slices(b"", b"");
    let expected = vec![0x00, 0x00];
    assert_eq!(name_value.encode(), expected);
  }

  #[test]
  fn test_name_length_127() {
    let name_value = NameValuePair::new(vec![b'a'; 127], b"value".to_vec());
    let mut expected = vec![0x7f, 0x05];
    expected.extend_from_slice(&[b'a'; 127]);
    expected.extend_from_slice(b"value");
    assert_eq!(name_value.encode(), expected);
  }

  #[test]
  fn test_value_length_127() {
    let name_value = NameValuePair::new(b"name".to_vec(), vec![b'b'; 127]);
    let mut expected = vec![0x04, 0x7f];
    expected.extend_from_slice(b"name");
    expected.extend_from_slice(&[b'b'; 127]);
    assert_eq!(name_value.encode(), expected);
  }
}
