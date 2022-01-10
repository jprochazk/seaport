use crate::{
  packet::{Packet, PacketData, PacketInfo, Pending, Serializer, Unsent},
  peer::PeerState,
  socket::Socket,
};
use std::{io, time::Instant};

use super::PeerManager;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
  Stop,
  Continue,
}

// TODO: write tests

fn send_one<S: Socket>(
  serializer: &Serializer,
  peer: &mut PeerState,
  buffer: &mut [u8],
  socket: &S,
) -> io::Result<Signal> {
  use Signal::*;
  match peer.unsent_packets.peek() {
    None => Ok(Continue),
    Some(outgoing) => match outgoing {
      Packet::Unsent(unsent) => {
        // serialize the packet
        let size = serializer.serialize(buffer, PacketData::new(peer, &unsent.payload[..]));

        // send the packet
        match socket.send_to(&buffer[0..size], peer.addr) {
          Ok(_) => {
            // since we successfully sent the packet, remove it from the queue.
            // Safety:
            //   - `get()` will return `Some`, because `peek()` did as well.
            //   - `into_unsent_unchecked()` will not fail, because we already matched on it being `Unsent`.
            let packet = unsafe {
              peer
                .unsent_packets
                .get()
                .unwrap_unchecked()
                .into_unsent_unchecked()
            };
            // push into the sequence buffer, so that we can track if this packet was acked
            peer
              .send_buffer
              .insert(peer.local_sequence, PacketInfo::default());
            // if this packet is reliable, push the payload into the queue as a `Pending` packet,
            // because we're not sure that it has been successfully received yet, and we may need
            // to re-send it.
            if packet.is_reliable {
              peer.unsent_packets.put(Packet::pending(
                packet.payload,
                peer.local_sequence,
                Instant::now().elapsed(),
              ));
            }
            peer.local_sequence += 1;

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
        let info = peer
          .send_buffer
          .get_mut(pending.sequence)
          .unwrap_or_else(|| {
            // This should not happen, but it could if the packet is not sent within the amount of
            // tries it would take to overwrite the entire `send_buffer`.
            panic!(
              "Reliable packet (seq {}, {} bytes) was lost",
              pending.sequence,
              pending.payload.len()
            )
          });
        if info.acked {
          // we've received an ACK for this packet, which means it was received successfully.
          // we can remove the packet from the queue
          // Safety:
          //   - `get()` will return `Some`, because `peek()` did as well.
          //   - `into_pending_unchecked()` will not fail, because we already matched on it being `Pending`.
          let _packet = unsafe {
            peer
              .unsent_packets
              .get()
              .unwrap_unchecked()
              .into_pending_unchecked()
          };
          // TODO: step RTT
          Ok(Continue)
        } else {
          // we haven't received an ACK yet.
          let now = Instant::now().elapsed();
          if now - pending.sent_at > peer.rtt {
            // the packet has been unsent for at least one round trip.
            // this may indicate that it is lost, so we will re-send it here.
            // TODO: increment a lost packet counter

            // serialize
            let size = serializer.serialize(buffer, PacketData::from_pending(peer, pending));

            // send the packet
            match socket.send_to(&buffer[0..size], peer.addr) {
              Ok(_) => {
                // since we successfully sent the packet, remove it from the queue.
                // Safety:
                //   - `get()` will return `Some`, because `peek()` did as well.
                //   - `into_pending_unchecked()` will not fail, because we already matched on it being `Pending`.
                let packet = unsafe {
                  peer
                    .unsent_packets
                    .get()
                    .unwrap_unchecked()
                    .into_pending_unchecked()
                };
                // we're still waiting for this packet to be acknowledged, so it may have to be resent *again*.
                // that means we have to put it back in the packet queue.
                peer
                  .unsent_packets
                  .put(Packet::pending(packet.payload, packet.sequence, now));

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
            let packet = unsafe {
              peer
                .unsent_packets
                .get()
                .unwrap_unchecked()
                .into_pending_unchecked()
            };
            peer.unsent_packets.put(Packet::Pending(packet));
            Ok(Continue)
          }
        }
      }
    },
  }
}

/// Try to send packets to random peers which have unsent payloads
/// until the socket returns `WouldBlock`.
pub(crate) fn send_some<S: Socket>(
  serializer: &Serializer,
  peers: &mut PeerManager,
  buffer: &mut Vec<u8>,
  socket: &S,
) -> io::Result<()> {
  while let Some(peer) = peers.dequeue_packet() {
    let now = Instant::now().elapsed();
    if now - peer.last_send > peer.send_interval {
      peer.last_send = now;
      match send_one(serializer, peer, buffer, socket)? {
        Signal::Continue => continue,
        Signal::Stop => break,
      }
    }
  }

  peers.maintain();

  Ok(())
}

#[cfg(test)]
mod tests {
  #![allow(non_snake_case)]

  use super::*;
  use crate::{
    error::Error,
    peer::{Handler, Peer, PeerManager},
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

    /*
    unsent (unreliable, space in queue)
      sequence updated
      send_buffer gets entry
      queue gets packet
      unsent_packets does not get packet
      Continue
    */

    let packet = Packet::unsent(vec![], false);
    peer.unsent_packets.put(packet);

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
    assert_eq!(peer.unsent_packets.remaining(), 0);
  }
  /*
  unsent (reliable, space in queue)
    sequence updated
    send_buffer gets entry
    queue gets packet
    unsent_packets gets packet back (at end)
    Continue

    */
  #[test]
  fn unsent__reliable__queue_non_full() {
    let (socket, _, receiver) = socket(None);
    let serializer = Serializer::new(PROTOCOL);
    let mut buffer = vec![0u8; 1 << 16];
    let peer_addr = "127.0.0.1:8080".parse().unwrap();
    let mut peer = PeerState::new(peer_addr);

    let payload = vec![];
    let packet = Packet::unsent(payload.clone(), true);
    peer.unsent_packets.put(packet);

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
    let packet = peer.unsent_packets.get().unwrap();
    assert!(matches!(packet, Packet::Pending(..)));
    let packet = unsafe { packet.into_pending_unchecked() };
    assert_eq!(packet.payload, payload);
    assert_eq!(packet.sequence, 0);
  }
  /*
  unsent (no space in queue)
    sequence not updated
    send_buffer does not get entry
    queue does not get packet
    unsent_packets gets packet back (same position)
    Stop

    */
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
      .unsent_packets
      .put(Packet::unsent(payload.clone(), true));
    peer.unsent_packets.put(Packet::unsent(payload, false));

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
      peer.unsent_packets.get().unwrap(),
      Packet::Unsent(Unsent {
        is_reliable: true,
        ..
      })
    ));
  }
  /*
  pending (acked)
    sequence not updated
    send_buffer does not get new entry
    queue does not get packet
    unsent_packets does not get packet

    */
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
      .unsent_packets
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
    assert!(matches!(peer.unsent_packets.get(), None));
  }
  /*
  pending (unacked, less than RTT)
    sequence not updated
    send_buffer does not get new entry
    queue does not get packet
    unsent_packets gets packet back (at end)

    */
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
      .unsent_packets
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
    assert!(matches!(peer.unsent_packets.get(), Some(..)));
  }
  /*
  pending (unacked, more than RTT, no space in queue)
    sequence not updated
    send_buffer does not get new entry
    queue does not get packet
    unsent_packets gets packet back (same position)

    */
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
      .unsent_packets
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
    assert!(matches!(peer.unsent_packets.get(), Some(..)));
  }
  /*
  pending (unacked, more than RTT, space in queue)
    sequence not updated
    send_buffer does not get new entry
    queue gets packet
    unsent_packets gets packet back (at end)

    */
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
      .unsent_packets
      .put(Packet::pending(vec![], 0, Duration::ZERO));
    peer.unsent_packets.put(Packet::unsent(vec![], true));
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
    let _ = peer.unsent_packets.get();
    // next packet should be the `Pending` that `send_one` added
    assert!(matches!(
      peer.unsent_packets.get(),
      Some(Packet::Pending(..))
    ));
  }
}
