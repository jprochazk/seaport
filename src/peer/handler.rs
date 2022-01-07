use crate::{error::Error, peer::Peer};

pub trait Handler {
  fn on_payload(&mut self, peer: Peer, payload: &[u8]) -> Result<(), Error>;
}
