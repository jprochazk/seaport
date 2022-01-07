use std::{error::Error as StdError, fmt::Debug, io};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
  #[error("IO error: {0}")]
  Io(#[from] io::Error),
  #[error("User error: {0}")]
  User(Box<dyn StdError + Send + Sync>),
}

impl Error {
  pub fn custom<T>(err: T) -> Error
  where
    T: StdError + Send + Sync + 'static,
  {
    Error::User(Box::new(err))
  }

  pub fn downcast<T>(&self) -> Option<&T>
  where
    T: StdError + Send + Sync + 'static,
  {
    match self {
      Error::User(err) => err.downcast_ref(),
      _ => None,
    }
  }
}

pub enum Reason {
  /// The connection was gracefully closed on either side via `.close()`.
  Normal,
  /// The connection timed out.
  Timeout,
  /// The connection encountered an issue as a result of the remote side
  /// engaging in behavior deviating from the protocol. This may indicate
  /// mismatching versions, or more likely the peer is attempting to forge
  /// packets.
  Deviant,
}

#[cfg(test)]
mod tests {
  use super::*;
  use pretty_assertions::assert_eq;

  #[test]
  fn create_and_print_custom_error_string() {
    use std::fmt::Write;
    #[derive(Debug, Error)]
    #[error("{0}")]
    struct MyError(&'static str);
    let input = MyError("test");
    let mut expected = String::new();
    write!(expected, "User error: {}", input).unwrap();

    let err = Error::custom(input);

    let mut actual = String::new();
    write!(actual, "{}", err).unwrap();

    assert_eq!(&expected, &actual)
  }

  #[allow(unreachable_patterns)]
  #[test]
  fn downcast_custom_error() {
    #[derive(Debug, Clone, Copy, PartialEq, Error)]
    #[error("{info}")]
    struct ErrorData {
      info: &'static str,
    }

    let input = ErrorData { info: "test" };
    let err = Error::custom(input);
    let output = match err.downcast::<ErrorData>() {
      Some(v) => *v,
      None => unreachable!(),
    };

    assert_eq!(input, output);
  }
}
