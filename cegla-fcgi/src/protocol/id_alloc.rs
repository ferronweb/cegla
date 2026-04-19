pub struct IdAllocator {
  used: [u64; 1024], // 1024 entries * 64 bits per entry = 65536 bits
}

impl Default for IdAllocator {
  fn default() -> Self {
    Self::new()
  }
}

impl IdAllocator {
  pub fn new() -> Self {
    let mut used = [0u64; 1024];
    used[0] = 1; // 0 is a reserved request ID for management records
    Self { used }
  }

  pub fn allocate(&mut self) -> Option<u16> {
    for (i, bits) in self.used.iter_mut().enumerate() {
      if *bits != u64::MAX {
        let free_bit = bits.trailing_ones() as u16;
        *bits |= 1u64 << free_bit;
        return Some((i as u16) * 64 + free_bit);
      }
    }
    None
  }

  pub fn free(&mut self, id: u16) {
    let entry = id / 64;
    let bit = id % 64;
    self.used[entry as usize] &= !(1u64 << bit);
  }

  pub fn mark_used(&mut self, id: u16) {
    let entry = id / 64;
    let bit = id % 64;
    self.used[entry as usize] |= 1u64 << bit;
  }

  pub fn is_used(&self, id: u16) -> bool {
    let entry = id / 64;
    let bit = id % 64;
    self.used[entry as usize] & (1u64 << bit) != 0
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_allocate() {
    let mut allocator = IdAllocator::new();
    for i in 1..65536 {
      assert_eq!(allocator.allocate(), Some(i as u16));
    }
    assert_eq!(allocator.allocate(), None);
  }

  #[test]
  fn test_free() {
    let mut allocator = IdAllocator::new();
    for i in 1..65536 {
      assert_eq!(allocator.allocate(), Some(i as u16));
    }
    assert_eq!(allocator.allocate(), None);
    allocator.free(64);
    assert_eq!(allocator.allocate(), Some(64));
  }
}
