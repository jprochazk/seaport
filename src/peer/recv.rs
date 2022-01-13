use crate::{
  error::Result,
  packet::{Deserializer, PacketData},
  peer::Peer,
  socket::Socket,
};
use std::io;

pub enum Recv<'a> {
  Stop,
  Bad(Peer),
  Packet(Peer, PacketData<'a>),
}

pub(crate) fn recv_one<'a, S: Socket>(
  deserializer: &Deserializer,
  buffer: &'a mut [u8],
  socket: &S,
) -> Result<Recv<'a>> {
  match socket.recv_from(buffer) {
    Ok((size, addr)) => {
      let peer = Peer { addr };
      match deserializer.deserialize(&buffer[0..size]) {
        Some(packet) => Ok(Recv::Packet(peer, packet)),
        None => Ok(Recv::Bad(peer)),
      }
    }
    Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(Recv::Stop),
    Err(e) => Err(e.into()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{socket::Socket, Protocol};
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

  #[test]
  fn socket_blocks() {
    let (socket, _, _) = socket();
    let deserializer = Deserializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];

    // nothing can be received

    // > recv blocks
    assert!(matches!(
      recv_one(&deserializer, &mut buffer, &socket).unwrap(),
      Recv::Stop
    ));
  }

  #[test]
  fn invalid_packet() {
    let (socket, sender, recv) = socket();
    let deserializer = Deserializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];

    // > recv doesn't block
    // > packet is discarded

    sender
      .send(Ok(("0.0.0.0:0".parse().unwrap(), vec![])))
      .unwrap();

    assert!(matches!(
      recv_one(&deserializer, &mut buffer, &socket).unwrap(),
      Recv::Bad(..)
    ));
    assert_eq!(recv.try_recv().unwrap_err(), TryRecvError::Empty);
  }

  // happy path
  // > recv doesn't block
  // > peer exists
  // > packet deserializes
  // TODO: test the missing parts
  // v now handled by `peer` itself
  // > inserted into recv buffer
  // > sent buffer acks updated
  // > `on_payload` called
  #[test]
  fn happy_path() {
    let (socket, sender, _) = socket();
    let deserializer = Deserializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];

    #[rustfmt::skip]
    let message = vec![
      /* protocol */  0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
      /* sequence */  0u8, 0u8, 0u8, 0u8,
      /* ack */       0u8, 0u8, 0u8, 0u8,
      /* ack_bits */  0u8, 0u8, 0u8, 0u8,
      /* payload (empty) */
    ];
    sender
      .send(Ok(("0.0.0.0:0".parse().unwrap(), message)))
      .unwrap();

    // > recv doesn't block
    // > packet deserializes
    assert!(matches!(
      recv_one(&deserializer, &mut buffer, &socket).unwrap(),
      Recv::Packet(..)
    ));
  }
}
