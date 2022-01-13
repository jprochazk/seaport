use crate::{
  packet::{Packet, PacketData, Serializer},
  peer::{PeerState, Signal},
  socket::Socket,
};
use std::{io, time::Instant};

// TODO: write tests

pub(crate) fn send_one<S: Socket>(
  serializer: &Serializer,
  peer: &mut PeerState,
  buffer: &mut [u8],
  socket: &S,
) -> io::Result<Signal> {
  use Signal::*;
  match peer.packet_queue.peek() {
    None => Ok(Continue),
    Some(outgoing) => match outgoing {
      Packet::Initial(unsent) => {
        // serialize and send the packet
        match socket.send_to(
          serializer.serialize(buffer, PacketData::new(peer, &unsent.payload[..])),
          peer.addr,
        ) {
          Ok(_) => {
            // Safety:
            //   - `get()` will return `Some`, because `peek()` did as well.
            //   - we already matched on the `Packet` being `Unsent`.
            unsafe { peer.sent_initial() };
            Ok(Continue)
          }
          Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            // the packet could not be send, so we will leave it in the queue at its current position.
            Ok(Stop)
          }
          Err(e) => Err(e),
        }
      }
      Packet::Pending(pending) => {
        if peer.is_acked(pending.sequence) {
          // we've received an ACK for this packet, which means it was received successfully.
          // we can remove the packet from the queue
          // Safety:
          //   - `get()` will return `Some`, because `peek()` did as well.
          //   - `into_pending_unchecked()` will not fail, because we already matched on it being `Pending`.
          unsafe { peer.sent_pending(true) }
          Ok(Continue)
        } else {
          // we haven't received an ACK yet.
          let now = Instant::now().elapsed();
          if now - pending.sent_at > peer.rtt {
            // the packet has been unsent for at least one round trip.
            // this may indicate that it is lost, so we will re-send it here.
            // TODO: increment a lost packet counter

            // serialize and send the packet
            match socket.send_to(
              serializer.serialize(buffer, PacketData::from_pending(peer, pending)),
              peer.addr,
            ) {
              Ok(_) => {
                // we successfully sent the packet, but we're still waiting for this packet to be acknowledged,
                // so it may have to be resent *again*. we will move the packet to the back of the queue.
                // Safety:
                //   - `get()` will return `Some`, because `peek()` did as well.
                //   - `into_pending_unchecked()` will not fail, because we already matched on it being `Pending`.
                unsafe { peer.sent_pending(false) };

                Ok(Continue)
              }
              Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // the packet could not be send, so we will leave it in the queue at its current position.
                Ok(Stop)
              }
              Err(e) => Err(e),
            }
          } else {
            // the packet has been unsent for less than RTT, so we just re-queue it for another checkup later.
            // Safety:
            //   - `get()` will return `Some`, because `peek()` did as well.
            //   - `into_pending_unchecked()` will not fail, because we already matched on it being `Pending`.
            unsafe { peer.sent_pending(false) };
            Ok(Continue)
          }
        }
      }
    },
  }
}

#[cfg(test)]
mod tests {
  #![allow(non_snake_case)]

  use super::*;
  use crate::{
    packet::{Initial, PacketInfo},
    socket::Socket,
    Protocol,
  };
  use crossbeam::channel::{self, Receiver, Sender, TryRecvError};
  use std::{cell::RefCell, io, net::SocketAddr, time::Duration};

  type Message = (SocketAddr, Vec<u8>);
  type FakeReceiver = Receiver<io::Result<Message>>;
  type FakeSender = Sender<io::Result<Message>>;

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

  fn socket(size: Option<usize>) -> (FakeSocket, FakeSender, FakeReceiver) {
    let (sender, receiver) = match size {
      Some(size) => channel::bounded(size),
      None => channel::unbounded(),
    };
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
  fn no_packet() {
    let (socket, _, _) = socket(None);
    let serializer = Serializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];
    let mut peer = PeerState::new("127.0.0.1:8080".parse().unwrap());

    assert_eq!(
      send_one(&serializer, &mut peer, &mut buffer, &socket).unwrap(),
      Signal::Continue
    );
  }
  #[test]
  fn unsent__unreliable__queue_non_full() {
    let (socket, _, receiver) = socket(None);
    let serializer = Serializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];
    let peer_addr = "127.0.0.1:8080".parse().unwrap();
    let mut peer = PeerState::new(peer_addr);

    let packet = Packet::initial(vec![], false);
    peer.packet_queue.put(packet);

    assert_eq!(
      send_one(&serializer, &mut peer, &mut buffer, &socket).unwrap(),
      Signal::Continue
    );
    assert_eq!(peer.local_sequence, 1);
    assert!(peer.send_buffer.get(0).is_some());
    assert_eq!(
      receiver.recv().unwrap().unwrap(),
      (peer_addr, vec![0u8; 20])
    );
    assert_eq!(peer.packet_queue.remaining(), 0);
  }

  #[test]
  fn unsent__reliable__queue_non_full() {
    let (socket, _, receiver) = socket(None);
    let serializer = Serializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];
    let peer_addr = "127.0.0.1:8080".parse().unwrap();
    let mut peer = PeerState::new(peer_addr);

    let payload = vec![];
    let packet = Packet::initial(payload.clone(), true);
    peer.packet_queue.put(packet);

    assert_eq!(
      send_one(&serializer, &mut peer, &mut buffer, &socket).unwrap(),
      Signal::Continue
    );
    assert_eq!(peer.local_sequence, 1);
    assert!(peer.send_buffer.get(0).is_some());
    assert_eq!(
      receiver.recv().unwrap().unwrap(),
      (peer_addr, vec![0u8; 20])
    );
    let packet = peer.packet_queue.get().unwrap();
    assert!(matches!(packet, Packet::Pending(..)));
    let packet = unsafe { packet.into_pending_unchecked() };
    assert_eq!(packet.payload, payload);
    assert_eq!(packet.sequence, 0);
  }

  #[test]
  fn unsent__queue_full() {
    // reliability doesn't matter in this case
    let (socket, sender, receiver) = socket(Some(1));
    let serializer = Serializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];
    let peer_addr = "127.0.0.1:8080".parse().unwrap();
    let mut peer = PeerState::new(peer_addr);

    sender
      .send(Err(io::Error::from(io::ErrorKind::Other)))
      .unwrap();

    let payload = vec![];
    peer
      .packet_queue
      .put(Packet::initial(payload.clone(), true));
    peer.packet_queue.put(Packet::initial(payload, false));

    assert_eq!(
      send_one(&serializer, &mut peer, &mut buffer, &socket).unwrap(),
      Signal::Stop
    );
    // remove the packet we manually put into the queue
    let _ = receiver.recv();

    assert_eq!(peer.local_sequence, 0);
    assert!(peer.send_buffer.get(0).is_none());
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    assert!(matches!(
      peer.packet_queue.get().unwrap(),
      Packet::Initial(Initial {
        is_reliable: true,
        ..
      })
    ));
  }

  #[test]
  fn pending__acked() {
    // reliability doesn't matter in this case
    let (socket, _, receiver) = socket(Some(1));
    let serializer = Serializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];
    let peer_addr = "127.0.0.1:8080".parse().unwrap();
    let mut peer = PeerState::new(peer_addr);

    // manually simulate a send of a reliable packet
    peer
      .packet_queue
      .put(Packet::pending(vec![], 0, Duration::ZERO));
    peer.send_buffer.insert(
      0,
      PacketInfo {
        acked: true,
        time: Duration::ZERO,
      },
    );
    peer.local_sequence += 1;

    assert_eq!(
      send_one(&serializer, &mut peer, &mut buffer, &socket).unwrap(),
      Signal::Continue
    );

    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    assert!(matches!(peer.packet_queue.get(), None));
  }

  #[test]
  fn pending__unacked__less_than_rtt() {
    // reliability doesn't matter in this case
    let (socket, _, receiver) = socket(Some(1));
    let serializer = Serializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];
    let peer_addr = "127.0.0.1:8080".parse().unwrap();
    let mut peer = PeerState::new(peer_addr);

    peer.rtt = Duration::MAX;
    // manually simulate a send of a reliable packet
    peer
      .packet_queue
      .put(Packet::pending(vec![], 0, Duration::ZERO));
    peer.send_buffer.insert(
      0,
      PacketInfo {
        acked: false,
        time: Duration::ZERO,
      },
    );
    peer.local_sequence += 1;

    assert_eq!(
      send_one(&serializer, &mut peer, &mut buffer, &socket).unwrap(),
      Signal::Continue
    );

    assert_eq!(peer.local_sequence, 1);
    assert!(matches!(peer.send_buffer.get(1), None));
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    assert!(matches!(peer.packet_queue.get(), Some(..)));
  }

  #[test]
  fn pending__unacked__more_than_rtt__queue_full() {
    // reliability doesn't matter in this case
    let (socket, sender, receiver) = socket(Some(1));
    let serializer = Serializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];
    let peer_addr = "127.0.0.1:8080".parse().unwrap();
    let mut peer = PeerState::new(peer_addr);

    // ensure queue is full (so `send_to` returns `WouldBlock`)
    sender
      .send(Err(io::Error::from(io::ErrorKind::Other)))
      .unwrap();
    // ensure that `Instant::now().elapsed()` is after `Duration::ZERO`
    std::thread::sleep(Duration::from_millis(10));

    peer.rtt = Duration::ZERO;
    // manually simulate a send of a reliable packet
    peer
      .packet_queue
      .put(Packet::pending(vec![], 0, Duration::ZERO));
    peer.send_buffer.insert(
      0,
      PacketInfo {
        acked: false,
        time: Duration::ZERO,
      },
    );
    peer.local_sequence += 1;

    assert_eq!(
      send_one(&serializer, &mut peer, &mut buffer, &socket).unwrap(),
      Signal::Stop
    );

    // remove the packet we pushed to make queue full
    let _ = receiver.recv();

    assert_eq!(peer.local_sequence, 1);
    assert!(matches!(peer.send_buffer.get(1), None));
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    assert!(matches!(peer.packet_queue.get(), Some(..)));
  }

  #[test]
  fn pending__unacked__more_than_rtt__queue_non_full() {
    // reliability doesn't matter in this case
    let (socket, _, receiver) = socket(Some(1));
    let serializer = Serializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];

    let peer_addr = "127.0.0.1:8080".parse().unwrap();
    let mut peer = PeerState::new(peer_addr);

    // ensure that `Instant::now().elapsed()` is after `Duration::ZERO`
    std::thread::sleep(Duration::from_millis(10));

    peer.rtt = Duration::ZERO;
    // manually simulate a send of a reliable packet
    peer
      .packet_queue
      .put(Packet::pending(vec![], 0, Duration::ZERO));
    peer.packet_queue.put(Packet::initial(vec![], true));
    peer.send_buffer.insert(
      0,
      PacketInfo {
        acked: false,
        time: Duration::ZERO,
      },
    );
    peer.local_sequence += 1;

    assert_eq!(
      send_one(&serializer, &mut peer, &mut buffer, &socket).unwrap(),
      Signal::Continue
    );

    assert_eq!(peer.local_sequence, 1);
    assert!(matches!(peer.send_buffer.get(1), None));
    assert_eq!(
      receiver.try_recv().unwrap().unwrap(),
      (peer_addr, vec![0u8; 20])
    );
    // remove packet we put in manually
    let _ = peer.packet_queue.get();
    // next packet should be the `Pending` that `send_one` added
    assert!(matches!(peer.packet_queue.get(), Some(Packet::Pending(..))));
  }
}
