pub(crate) mod handler;
pub(crate) mod recv;
pub(crate) mod send;

use crate::{
  error::Result,
  packet::{self, AckBits, Packet, PacketData, PacketInfo},
  queue::{Queue, SwapQueue},
};
use std::{
  collections::HashMap,
  net::SocketAddr,
  time::{Duration, Instant},
};

pub use handler::Handler;
pub(crate) use recv::recv_one;
pub(crate) use send::send_one;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
  Stop,
  Continue,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peer {
  pub addr: SocketAddr,
}

impl Peer {
  pub fn addr(&self) -> SocketAddr {
    self.addr
  }
}

// TODO: symmetric key encryption established during handshake

pub(crate) struct PeerState {
  pub addr: SocketAddr,
  // TODO: multiple channels to avoid head-of-line blocking caused by reliable packets
  // channels should be configurable and
  pub local_sequence: u32,
  // TODO: configurable buffer sizes
  // maybe based on time, e.g. N packets where `N = send_rate * T` and `T = x seconds`
  pub send_buffer: packet::Buffer,
  pub remote_sequence: u32,
  pub recv_buffer: packet::Buffer,
  pub packet_queue: Queue<Packet>,
  // TODO: exponentially smoothed moving average RTT
  pub rtt: Duration,
  // TODO: configurable send rate
  pub send_interval: Duration,
  pub last_send: Duration,
}

impl PeerState {
  pub fn new(addr: SocketAddr) -> Self {
    Self {
      addr,
      local_sequence: 0,
      send_buffer: packet::Buffer::new(512),
      remote_sequence: 0,
      recv_buffer: packet::Buffer::new(512),
      packet_queue: Queue::new(64),
      rtt: Duration::from_millis(100),
      send_interval: Duration::from_secs_f32(1.0 / 30.0),
      last_send: Duration::new(0, 0),
    }
  }

  pub fn is_acked(&self, sequence: u32) -> bool {
    // TODO: handle the case where the packet is pending+unacked, and it is overwritten
    // In what scenario can it be overwritten?
    self
      .send_buffer
      .get(sequence)
      .map(|v| v.acked)
      .unwrap_or(false)
  }

  /// Notifies this peer that an `Initial` packet has been sent.
  ///
  ///
  /// Safety: `packet_queue.get().is_some() && matches!(packet_queue.get(), Some(Initial(...)))`
  pub unsafe fn sent_initial(&mut self) {
    let packet = self
      .packet_queue
      .get()
      .unwrap_unchecked()
      .into_initial_unchecked();
    // push into the sequence buffer, so that we can track if this packet was acked
    self
      .send_buffer
      .insert(self.local_sequence, PacketInfo::default());
    // transform reliable packets to `Pending` and push them to the back of the packet queue.
    if packet.is_reliable {
      self.packet_queue.put(Packet::pending(
        packet.payload,
        self.local_sequence,
        Instant::now().elapsed(),
      ));
    }
    self.local_sequence += 1;
  }

  /// Notifies this peer that a `Pending` packet has been sent.
  ///
  /// Safety: `packet_queue.get().is_some() && matches!(packet_queue.get(), Some(Pending(...)))`
  pub unsafe fn sent_pending(&mut self, acked: bool) {
    let packet = self
      .packet_queue
      .get()
      .unwrap_unchecked()
      .into_pending_unchecked();
    // If `acked` is false, the packet is moved to the end of the packet queue.
    if !acked {
      self.packet_queue.put(Packet::Pending(packet));
    }
  }

  pub fn received<H: Handler>(&mut self, packet: PacketData<'_>, handler: &mut H) -> Result<()> {
    // TODO: validate packet:
    // - [x] protocol
    //   - avoids version mismatch and random packets
    // TODO: reinforce against integer overflow panics
    // search for `+` and `-` and use either checked or wrapping add/sub and handling the effects
    // - [ ] sequence/ack should not be very far from previous
    //   - avoids packet getting to very high sequence numbers,
    //     which should not happen under normal operation for months
    // QQQ: involve `handler.on_payload` in validation?
    // - allow returning `false` to reject the packet.
    // TODO: ensure that `sequence` and `ack` or not far off from previous value

    if packet.sequence() > self.remote_sequence {
      self.remote_sequence = packet.sequence();
    }
    self
      .recv_buffer
      .insert(self.remote_sequence, PacketInfo::default());
    self
      .send_buffer
      .set_ack_bits(packet.ack(), packet.ack_bits());
    // TODO: step RTT
    handler.on_payload(Peer { addr: self.addr }, packet.payload())?;

    Ok(())
  }
}

pub(crate) type PeerTable = HashMap<SocketAddr, PeerState>;
pub(crate) type PeerQueue = SwapQueue<SocketAddr>;
