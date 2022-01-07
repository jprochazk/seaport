use crate::error::{Error, Reason};
use std::net::SocketAddr;

pub trait Client {
  type Error;
  /// Called when the client receives a whole packet from a server.
  fn on_payload(&mut self, payload: &[u8]) -> Result<(), Self::Error>;
  /// Called after the client connects to the server.
  fn on_connect(&mut self) -> Result<(), Self::Error>;
  /// Called after the client is disconnected.
  fn on_disconnect(&mut self, reason: Reason) -> Result<(), Self::Error>;
  /// Called when the connection encounters an error, including those returned by the user-implemented handler methods.
  ///
  /// The implementation treats all errors as unrecoverable.
  fn on_error(&mut self, error: Error);
}

pub fn connect<F, H>(addr: SocketAddr, factory: F) -> Result<(), Error> {
  // TODO: client connection
  Ok(())
}
