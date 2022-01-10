use crate::error::Error;
use std::net::SocketAddr;

pub fn connect<F, H>(addr: SocketAddr, factory: F) -> Result<(), Error> {
  // TODO: client connection
  Ok(())
}
