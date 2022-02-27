use {
  bytes::{Buf, BufMut},
  thiserror::Error,
};

#[derive(Debug, Clone, Error)]
pub enum Error {
  #[error("unexpected end of input")]
  UnexpectedEof,
  #[error("invalid {0} kind")]
  InvalidKind(&'static str),
  #[error("maximum {0} size exceeded")]
  TooLarge(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait Encode: Sized {
  /// Encode a value of `Self` into `buf`.
  fn encode<B: BufMut>(&self, buf: &mut B);
}

pub trait Decode: Sized {
  /// Decode a value of `Self` from `buf`.
  fn decode<B: Buf>(buf: &mut B) -> Result<Self>;
}

macro_rules! impl_for {
  ($ty:ident, $put:ident, $get:ident) => {
    impl Encode for $ty {
      fn encode<B: BufMut>(&self, buf: &mut B) {
        buf.$put(*self)
      }
    }
    impl Decode for $ty {
      fn decode<B: Buf>(buf: &mut B) -> Result<Self> {
        if buf.remaining() < std::mem::size_of::<Self>() {
          Err(Error::UnexpectedEof)
        } else {
          Ok(buf.$get())
        }
      }
    }
  };
}

impl_for!(u8, put_u8, get_u8);
impl_for!(u16, put_u16, get_u16);
impl_for!(u32, put_u32, get_u32);
impl_for!(u64, put_u64, get_u64);
