use crate::{
  error::{Reason, Result},
  peer::Peer,
};
use std::borrow::Cow;

pub enum Decision {
  Accept,
  Reject(Option<Cow<'static, str>>),
}

pub trait Handler {
  /// Called when the server receives a whole packet from a client.
  fn on_payload(&mut self, peer: Peer, payload: &[u8]) -> Result<()>;
  /// Called just before the server accepts a client with:
  /// - `peer`, an opaque token representing the connection, and used as the `send` field in the `Sender`.
  /// - `payload`, which is an opaque buffer storing the userdata associated with the handshake. The data is transported
  /// *after* encryption is established, so it may contain authentication credentials, for example.
  ///
  /// Returning `Reject(...)` will result in the server rejecting the client with the given reason, if any.
  #[allow(unused_variables)]
  fn on_before_connect(&mut self, peer: Peer, payload: &[u8]) -> Result<Decision> {
    Ok(Decision::Accept)
  }
  /// Called after the server accepts a client.
  fn on_connect(&mut self, peer: Peer) -> Result<()> {
    log::info!("{:?} connected", peer);
    Ok(())
  }
  /// Called after a client disconnects from the server.
  fn on_disconnect(&mut self, peer: Peer, reason: Reason) -> Result<()> {
    log::info!("{:?} disconnected ({:?})", peer, reason);
    Ok(())
  }
}
