use {
  crate::codec::{self, Decode, Encode},
  std::mem::size_of,
};

/// A variable length integer.
///
/// The maximum value is 2^62, because two bits are used to encode the integer's type.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VarInt(u64);

impl VarInt {
  pub const MAX: VarInt = VarInt((1 << 62) - 1);

  /// Creates a `VarInt` from a `u8`.
  ///
  /// This can never fail.
  pub const fn u8(value: u8) -> Self {
    Self(value as u64)
  }

  /// Creates a `VarInt` from a `u16`.
  ///
  /// This can never fail.
  pub const fn u16(value: u16) -> Self {
    Self(value as u64)
  }

  /// Creates a `VarInt` from a `u32`.
  ///
  /// This can never fail.
  pub const fn u32(value: u32) -> Self {
    Self(value as u64)
  }

  /// Create a `VarInt` from `u64`.
  ///
  /// This will fail if `value > (1 << 62) - 1`
  pub const fn u64(value: u64) -> Option<Self> {
    if value > Self::MAX.0 {
      None
    } else {
      Some(VarInt(value))
    }
  }
}

impl Encode for VarInt {
  fn encode<B: bytes::BufMut>(&self, buf: &mut B) {
    let v = self.0;
    if v < 2u64.pow(6) {
      (v as u8).encode(buf);
    } else if v < 2u64.pow(14) {
      (0b01 << 14 | v as u16).encode(buf);
    } else if v < 2u64.pow(30) {
      (0b10 << 30 | v as u32).encode(buf);
    } else {
      (0b11 << 62 | v).encode(buf);
    }
  }
}

impl Decode for VarInt {
  fn decode<B: bytes::Buf>(buf: &mut B) -> codec::Result<Self> {
    if buf.remaining() < 1 {
      return Err(codec::Error::UnexpectedEof);
    }

    let mut data = [buf.get_u8(), 0, 0, 0, 0, 0, 0, 0];
    // type tag is contained within the first byte,
    // because the value is encoded in big-endian
    let ty = data[0] & 0b1100_0000;
    // clear the type tag
    data[0] &= 0b0011_1111;

    if ty == 0b1100_0000 {
      // u64
      if buf.remaining() < size_of::<u64>() - 1 {
        return Err(codec::Error::UnexpectedEof);
      }
      buf.copy_to_slice(&mut data[1..size_of::<u64>()]);
      Ok(VarInt(u64::from_be_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
      ])))
    } else if ty == 0b1000_0000 {
      // u32
      if buf.remaining() < size_of::<u32>() - 1 {
        return Err(codec::Error::UnexpectedEof);
      }
      buf.copy_to_slice(&mut data[1..size_of::<u32>()]);
      Ok(VarInt(u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64))
    } else if ty == 0b0100_0000 {
      // u16
      if buf.remaining() < size_of::<u16>() - 1 {
        return Err(codec::Error::UnexpectedEof);
      }
      buf.copy_to_slice(&mut data[1..size_of::<u16>()]);
      Ok(VarInt(u16::from_be_bytes([data[0], data[1]]) as u64))
    } else {
      // u8
      Ok(VarInt(data[0] as u64))
    }
  }
}

impl std::fmt::Debug for VarInt {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let mut d = f.debug_tuple("Varint");
    if self.0 < 2u64.pow(6) {
      d.field(&"u8");
    } else if self.0 < 2u64.pow(14) {
      d.field(&"u16");
    } else if self.0 < 2u64.pow(30) {
      d.field(&"u32");
    } else {
      d.field(&"u64");
    }
    d.field(&self.0).finish()
  }
}

#[cfg(test)]
mod tests {
  use {super::*, pretty_assertions::assert_eq};

  #[allow(clippy::unusual_byte_groupings)]
  #[test]
  fn encoding() {
    macro_rules! assert_encode {
      ($value:expr, $expected:expr) => {{
        let mut buf = [0u8; 8];
        VarInt($value).encode(&mut &mut buf[..]);
        let ex: &[u8] = &$expected;
        assert_eq!(&buf[..ex.len()], ex);
      }};
    }

    assert_encode!(0, [0b00000000]);
    assert_encode!(1, [0b00000001]);
    assert_encode!(2u64.pow(6) - 1, [0b00_111111]);
    assert_encode!(2u64.pow(14) - 1, [0b01_111111, 0b11111111]);
    assert_encode!(2u64.pow(30) - 1, [0b10_111111, 0b11111111, 0b11111111, 0b11111111]);
    assert_encode!(
      2u64.pow(62) - 1,
      [
        0b11_111111,
        0b11111111,
        0b11111111,
        0b11111111,
        0b11111111,
        0b11111111,
        0b11111111,
        0b11111111
      ]
    );
  }

  #[allow(clippy::unusual_byte_groupings)]
  #[test]
  fn decoding() {
    macro_rules! assert_decode {
      ($bytes:expr, $expected:expr) => {{
        let bytes = $bytes;
        assert_eq!(VarInt::decode(&mut &bytes[..]).unwrap(), VarInt($expected));
      }};
    }

    assert_decode!([0b00000000], 0);
    assert_decode!([0b00000001], 1);
    assert_decode!([0b00_111111], 2u64.pow(6) - 1);
    assert_decode!([0b01_111111, 0b11111111], 2u64.pow(14) - 1);
    assert_decode!([0b10_111111, 0b11111111, 0b11111111, 0b11111111], 2u64.pow(30) - 1);
    assert_decode!(
      [
        0b11_111111,
        0b11111111,
        0b11111111,
        0b11111111,
        0b11111111,
        0b11111111,
        0b11111111,
        0b11111111
      ],
      2u64.pow(62) - 1
    );
  }
}
