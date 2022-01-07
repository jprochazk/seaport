pub(crate) mod handler;
pub(crate) mod recv;
pub(crate) mod send;

use crate::{
  packet::{self, Packet},
  queue::{Queue, SwapQueue},
};
use indexmap::IndexMap;
use std::{net::SocketAddr, time::Duration};

pub(crate) use handler::Handler;
pub(crate) use recv::recv_some;
pub(crate) use send::send_some;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peer {
  pub addr: SocketAddr,
}

impl Peer {
  pub fn addr(&self) -> SocketAddr {
    self.addr
  }
}

pub(crate) struct PeerState {
  pub addr: SocketAddr,
  // TODO: multiple channels to avoid head-of-line blocking caused by reliable packets
  pub local_sequence: u32,
  pub send_buffer: packet::Buffer,
  pub remote_sequence: u32,
  pub recv_buffer: packet::Buffer,
  pub unsent_packets: Queue<Packet>,
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
      unsent_packets: Queue::new(64),
      rtt: Duration::from_millis(100),
      send_interval: Duration::from_secs_f32(1.0 / 30.0),
      last_send: Duration::new(0, 0),
    }
  }
}

type PeerTable = IndexMap<SocketAddr, PeerState>;
type PeerQueue = SwapQueue<SocketAddr>;

pub(crate) struct PeerManager {
  capacity: usize,
  table: PeerTable,
  // Safety: Every address in `queue` must be present in `table`
  queue: PeerQueue,
}

impl PeerManager {
  pub fn new(capacity: usize) -> Self {
    Self {
      capacity,
      table: PeerTable::with_capacity(capacity),
      queue: PeerQueue::new(capacity),
    }
  }

  pub fn dequeue(&mut self) -> Option<&mut PeerState> {
    self.queue.get().map(|a| {
      // get the peer, then insert it back into the queue
      let peer = self.table.get_mut(&a);
      self.queue.put(a);
      // Safety: Every `SocketAddr` in `peer_queue` is also in `peer_table`.
      unsafe { peer.unwrap_unchecked() }
    })
  }

  pub fn enqueue(&mut self, addr: SocketAddr, packet: Packet) {
    if let Some(peer) = self.table.get_mut(&addr) {
      peer.unsent_packets.put(packet);
    } else {
      // TODO: just drop packets or notify user?
    }
  }

  pub fn get(&self, addr: &SocketAddr) -> Option<&PeerState> {
    self.table.get(addr)
  }

  pub fn get_mut(&mut self, addr: &SocketAddr) -> Option<&mut PeerState> {
    self.table.get_mut(addr)
  }

  pub fn maintain(&mut self) {
    if self.queue.is_empty() {
      self.queue.swap();
    }
  }
}
