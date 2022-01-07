use crate::{
  error::Error,
  packet::{AckBits, PacketData, PacketInfo},
  peer::{handler::Handler, Peer, PeerManager},
  socket::Socket,
  Protocol,
};
use std::io;

// TODO: write tests

/// Try to receive packets until the socket returns `WouldBlock`.
pub(crate) fn recv_some<S: Socket, H: Handler>(
  protocol: Protocol,
  peers: &mut PeerManager,
  buffer: &mut Vec<u8>,
  socket: &S,
  handler: &mut H,
) -> Result<(), Error> {
  loop {
    let (size, addr) = match socket.recv_from(&mut buffer[..]) {
      Ok((size, addr)) => (size, addr),
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
        return Ok(());
      }
      Err(e) => return Err(e.into()),
    };

    let peer = match peers.get_mut(&addr) {
      Some(peer) => peer,
      // TODO: handle unknown peer (connection request?)
      None => continue,
    };

    let packet = match PacketData::deserialize_from(&buffer[0..size]) {
      Some(packet) => packet,
      None => continue,
    };

    // TODO: validate packet

    if packet.sequence() > peer.remote_sequence {
      peer.remote_sequence = packet.sequence();
    }
    peer
      .recv_buffer
      .insert(peer.remote_sequence, PacketInfo::default());
    peer
      .send_buffer
      .set_ack_bits(packet.ack(), packet.ack_bits());
    handler.on_payload(Peer { addr }, packet.payload())?;
  }
}
