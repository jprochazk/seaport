struct Entry<T: Default> {
  sequence: u32,
  item: T,
}

impl<T> Default for Entry<T>
where
  T: Default,
{
  fn default() -> Self {
    Self {
      sequence: std::u32::MAX,
      item: Default::default(),
    }
  }
}

impl<T: Default + std::fmt::Debug> std::fmt::Debug for Entry<T> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Entry")
      .field("sequence", &self.sequence)
      .field("item", &self.item)
      .finish()
  }
}

pub struct Buffer<T: Default> {
  inner: Vec<Entry<T>>,
}

impl<T: Default> Buffer<T> {
  pub fn new(size: usize) -> Self {
    let mut inner = Vec::new();
    inner.resize_with(size, Default::default);
    Self { inner }
  }

  #[inline]
  fn entry(&self, sequence: u32) -> &Entry<T> {
    let index = sequence as usize % self.inner.len();
    // Safety: Due to the modulo above, `index` is always in (0..len)
    unsafe { self.inner.get_unchecked(index) }
  }

  #[inline]
  fn entry_mut(&mut self, sequence: u32) -> &mut Entry<T> {
    let index = sequence as usize % self.inner.len();
    // Safety: Due to the modulo above, `index` is always in (0..len)
    unsafe { self.inner.get_unchecked_mut(index) }
  }

  /// Get the entry at `sequence % buffer.size`, if the sequence matches.
  #[inline]
  pub fn get(&self, sequence: u32) -> Option<&T> {
    let entry = self.entry(sequence);
    if entry.sequence == sequence {
      Some(&entry.item)
    } else {
      None
    }
  }

  /// Get the entry at `sequence % buffer.size`, if the sequence matches.
  #[inline]
  pub fn get_mut(&mut self, sequence: u32) -> Option<&mut T> {
    let entry = self.entry_mut(sequence);
    if entry.sequence == sequence {
      Some(&mut entry.item)
    } else {
      None
    }
  }

  /// Insert an entry at `sequence % buffer.size`, overwriting the existing entry.
  #[inline]
  pub fn insert(&mut self, sequence: u32, item: T) {
    let entry = self.entry_mut(sequence);
    entry.sequence = sequence;
    entry.item = item;
  }
}

impl<T: Default + std::fmt::Debug> std::fmt::Debug for Buffer<T> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{:?}", self.inner)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn buffer_wraps() {
    let size = 64usize;
    let mut buffer = Buffer::<bool>::new(size);

    buffer.insert(64, true);
    assert!(buffer.inner[0].item);
  }
}
