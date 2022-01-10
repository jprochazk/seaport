use crate::{
  error::Error,
  packet::{AckBits, Deserializer, PacketInfo},
  peer::{handler::Handler, Peer, PeerManager},
  socket::Socket,
};
use std::io;

// TODO: write tests

/// Try to receive packets until the socket returns `WouldBlock`.
pub(crate) fn recv_some<S: Socket, H: Handler>(
  deserializer: &Deserializer,
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

    // TODO: ensure that `sequence` and `ack` or not far off from previous value
    let packet = match deserializer.deserialize(&buffer[0..size]) {
      Some(packet) => packet,
      // TODO: invalid packet - may indicate tampering with packets - how to handle?
      None => continue,
    };

    // TODO: validate packet:
    // - protocol
    //   - avoids version mismatch and random packets
    // - sequence/ack should not be very far from previous
    //   - avoids packet getting to very high sequence numbers,
    //     which should not happen under normal operation for months
    // QQQ: involve `handler.on_payload` in validation?
    // - allow returning `false` to reject the packet.

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

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{
    error::Error,
    peer::{Handler, Peer, PeerManager},
    socket::Socket,
    Protocol,
  };
  use crossbeam::channel::{self, Receiver, Sender, TryRecvError};
  use std::{cell::RefCell, io, net::SocketAddr};

  type Packet = (SocketAddr, Vec<u8>);
  type FakeReceiver = Receiver<io::Result<Packet>>;
  type FakeSender = Sender<io::Result<Packet>>;

  #[derive(Clone)]
  struct FakeSocket {
    recv: RefCell<FakeReceiver>,
    send: RefCell<FakeSender>,
  }

  fn would_block<T>(_: T) -> io::Error {
    io::Error::from(io::ErrorKind::WouldBlock)
  }

  impl Socket for FakeSocket {
    fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
      self
        .send
        .borrow_mut()
        .try_send(Ok((target, buf.to_owned())))
        .map(|_| buf.len())
        .map_err(would_block)
    }

    fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
      let (addr, data) = self.recv.borrow_mut().try_recv().map_err(would_block)??;
      buf[0..data.len()].copy_from_slice(&data[..]);
      Ok((data.len(), addr))
    }
  }

  fn socket() -> (FakeSocket, FakeSender, FakeReceiver) {
    let (sender, receiver) = channel::unbounded();
    (
      FakeSocket {
        recv: RefCell::new(receiver.clone()),
        send: RefCell::new(sender.clone()),
      },
      sender,
      receiver,
    )
  }

  const PROTOCOL: Protocol = Protocol(0);
  const MAX_PEERS: usize = 64;

  fn handler<F>(f: F) -> impl Handler
  where
    F: FnMut(Peer, Vec<u8>) -> Result<(), Error>,
  {
    struct _Handler<F: FnMut(Peer, Vec<u8>) -> Result<(), Error>> {
      f: F,
    }
    impl<F: FnMut(Peer, Vec<u8>) -> Result<(), Error>> Handler for _Handler<F> {
      fn on_payload(&mut self, peer: Peer, payload: &[u8]) -> Result<(), Error> {
        (self.f)(peer, payload.to_owned())
      }
    }
    _Handler { f }
  }

  #[test]
  fn socket_blocks() {
    let (socket, _, _) = socket();
    let deserializer = Deserializer::new(PROTOCOL);
    let mut peers = PeerManager::new(MAX_PEERS);
    let mut buffer = vec![0u8; 1 << 16];
    let mut handler = handler(|_, _| Ok(()));

    // nothing can be received

    // > recv data blocks
    recv_some(
      &deserializer,
      &mut peers,
      &mut buffer,
      &socket,
      &mut handler,
    )
    .unwrap();
  }

  #[test]
  fn unknown_peer() {
    let (socket, sender, recv) = socket();
    let deserializer = Deserializer::new(PROTOCOL);
    let mut peers = PeerManager::new(MAX_PEERS);
    let mut buffer = vec![0u8; 1 << 16];
    let mut handler = handler(|_, _| Ok(()));

    sender
      .send(Ok(("0.0.0.0:0".parse().unwrap(), vec![])))
      .unwrap();

    // > recv doesn't block
    // > peer isn't in `peers`
    // > packet is discarded
    recv_some(
      &deserializer,
      &mut peers,
      &mut buffer,
      &socket,
      &mut handler,
    )
    .unwrap();
    assert_eq!(recv.try_recv().unwrap_err(), TryRecvError::Empty);
  }

  // invalid packet
  // > recv doesn't block
  // > peer exists
  // > packet does not deserialize
  #[test]
  fn invalid_packet() {
    let (socket, sender, recv) = socket();
    let deserializer = Deserializer::new(PROTOCOL);
    let mut peers = PeerManager::new(MAX_PEERS);
    let mut buffer = vec![0u8; 1 << 16];
    let mut handler = handler(|_, _| Ok(()));

    let peer = "0.0.0.0:0".parse().unwrap();
    peers.add_peer(peer);
    sender.send(Ok((peer, vec![]))).unwrap();

    // > recv doesn't block
    // > peer isn't in `peers`
    // > packet is discarded
    recv_some(
      &deserializer,
      &mut peers,
      &mut buffer,
      &socket,
      &mut handler,
    )
    .unwrap();
    assert_eq!(recv.try_recv().unwrap_err(), TryRecvError::Empty);
  }

  // happy path
  // > recv doesn't block
  // > peer exists
  // > packet deserializes
  // > inserted into recv buffer
  // > sent buffer acks updated
  // > `on_payload` called
  #[test]
  fn happy_path() {
    let (socket, sender, recv) = socket();
    let deserializer = Deserializer::new(PROTOCOL);
    let mut peers = PeerManager::new(MAX_PEERS);
    let mut buffer = vec![0u8; 1 << 16];

    let handle_called = RefCell::new(false);
    let mut handler = handler(|_, _| {
      *handle_called.borrow_mut() = true;
      Ok(())
    });

    let peer = "0.0.0.0:0".parse().unwrap();
    peers.add_peer(peer);

    #[rustfmt::skip]
    let message = vec![
      /* protocol */  0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
      /* sequence */  0u8, 0u8, 0u8, 0u8,
      /* ack */       0u8, 0u8, 0u8, 0u8,
      /* ack_bits */  0u8, 0u8, 0u8, 0u8,
      /* payload (empty) */
    ];
    sender.send(Ok((peer, message))).unwrap();

    // > recv doesn't block
    // > peer exists
    // > packet deserializes
    // > inserted into recv buffer
    // > sent buffer acks updated
    // > `on_payload` called
    recv_some(
      &deserializer,
      &mut peers,
      &mut buffer,
      &socket,
      &mut handler,
    )
    .unwrap();

    assert_eq!(recv.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(*handle_called.borrow());
    let peer = peers.get(&peer).unwrap();
    assert!(peer.recv_buffer.get(0).is_some());
  }
}
