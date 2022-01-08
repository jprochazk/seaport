use crate::{
  packet::{Packet, PacketData, PacketInfo, Pending, Unsent},
  peer::PeerState,
  socket::Socket,
  Protocol,
};
use std::{io, time::Instant};

use super::PeerManager;

pub enum Signal {
  Stop,
  Continue,
}

// TODO: write tests

fn send_one<S: Socket>(
  protocol: Protocol,
  peer: &mut PeerState,
  buffer: &mut [u8],
  socket: &S,
) -> io::Result<Signal> {
  use Signal::*;
  match peer.unsent_packets.peek() {
    None => Ok(Continue),
    Some(outgoing) => match outgoing {
      Packet::Unsent(Unsent { payload, .. }) => {
        let local_sequence = peer.local_sequence;
        peer.local_sequence += 1;

        // track whether this packet was acked
        peer
          .send_buffer
          .insert(local_sequence, PacketInfo::default());

        // serialize the packet
        let size = PacketData::build_in(
          buffer,
          protocol,
          local_sequence,
          peer.remote_sequence,
          &peer.recv_buffer,
          &payload[..],
        );

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
            // if this packet is reliable, push the payload into the queue as a `Pending` packet,
            // because we're not sure that it has been successfully received yet, and we may need
            // to re-send it.
            if packet.is_reliable {
              peer.unsent_packets.put(Packet::pending(
                packet.payload,
                local_sequence,
                Instant::now().elapsed(),
              ));
            }

            Ok(Continue)
          }
          Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            // the packet could not be send, so we will leave it in the queue at its current position.
            Ok(Stop)
          }
          Err(e) => Err(e),
        }
      }
      Packet::Pending(Pending {
        payload,
        sequence,
        sent_at,
      }) => {
        let info = peer.send_buffer.get_mut(*sequence).unwrap_or_else(|| {
          // This should not happen, but it could if the packet is not sent within the amount of
          // tries it would take to overwrite the entire `send_buffer`.
          panic!(
            "Reliable packet (seq {}, {} bytes) was lost",
            sequence,
            payload.len()
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
          if now - *sent_at > peer.rtt {
            // the packet has been unsent for at least one round trip.
            // this may indicate that it is lost, so we will re-send it here.
            // TODO: increment a lost packet counter

            // serialize
            let size = PacketData::build_in(
              buffer,
              protocol,
              *sequence,
              peer.remote_sequence,
              &peer.recv_buffer,
              &payload[..],
            );

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
            peer.unsent_packets.put(Packet::pending(
              packet.payload,
              packet.sequence,
              packet.sent_at,
            ));
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
  protocol: Protocol,
  peers: &mut PeerManager,
  buffer: &mut Vec<u8>,
  socket: &S,
) -> io::Result<()> {
  while let Some(peer) = peers.dequeue_packet() {
    let now = Instant::now().elapsed();
    if now - peer.last_send > peer.send_interval {
      peer.last_send = now;
      match send_one(protocol, peer, buffer, socket)? {
        Signal::Continue => continue,
        Signal::Stop => break,
      }
    }
  }

  peers.maintain();

  Ok(())
}
