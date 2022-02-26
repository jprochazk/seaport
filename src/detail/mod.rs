#[inline]
pub const fn ceil_div(x: usize, d: usize) -> usize {
  (x + d - 1) / d
}

#[inline]
pub const fn align(size: usize, to: usize) -> usize {
  size.wrapping_add(to).wrapping_sub(1) & !(to.wrapping_sub(1))
}

#[cfg(test)]
mod tests {
  use {super::*, pretty_assertions::assert_eq};

  #[test]
  fn div_and_ceil() {
    assert_eq!(ceil_div(0, 16), 0);
    assert_eq!(ceil_div(15, 16), 1);
    assert_eq!(ceil_div(16, 16), 1);
    assert_eq!(ceil_div(17, 16), 2);
  }

  #[test]
  fn align_usize() {
    assert_eq!(align(15, 16), 16);
    assert_eq!(align(17, 16), 32);
    assert_eq!(align(0, 16), 0);
  }
}
