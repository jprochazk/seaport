use crate::{
  error::Result,
  packet::{Deserializer, Packet, Serializer},
  peer::{
    recv::{recv_one, Recv},
    send::send_one,
    Handler, Peer, PeerQueue, PeerState, PeerTable, Signal,
  },
  Protocol,
};
use crossbeam::channel::{self, Receiver as RawReceiver, Sender as RawSender};
use mio::{net::UdpSocket, Events, Interest, Poll, Token};
use std::{
  net::SocketAddr,
  thread::{self, JoinHandle},
  time::{Duration, Instant},
};

enum Command {
  Send {
    peer: Peer,
    payload: Vec<u8>,
    is_reliable: bool,
  },
  Disconnect {
    peer: Peer,
  },
  Shutdown {
    signal: channel::Sender<()>,
  },
}

// TODO: congestion control + fragmentation/assembly

/// Try to send packets to random peers which have unsent payloads
/// until the socket returns `WouldBlock`.
fn send_some(
  serializer: &Serializer,
  peers: &mut PeerManager,
  buffer: &mut Vec<u8>,
  socket: &UdpSocket,
) -> Result<()> {
  while let Some(peer) = peers.next_peer() {
    let now = Instant::now().elapsed();
    if now - peer.last_send > peer.send_interval {
      peer.last_send = now;
      // TODO: send ping if nothing else is queued at this point
      match send_one(serializer, peer, buffer, socket)? {
        Signal::Continue => continue,
        Signal::Stop => break,
      }
    }
  }

  peers.maintain();

  Ok(())
}

fn recv_some<H: Handler>(
  deserializer: &Deserializer,
  peers: &mut PeerTable,
  buffer: &mut [u8],
  socket: &UdpSocket,
  handler: &mut H,
) -> Result<()> {
  loop {
    match recv_one(deserializer, buffer, socket)? {
      Recv::Stop => break,
      Recv::Bad(_peer) => {
        // TODO: disconnect bad peer
      }
      Recv::Packet(peer, packet) => {
        // TODO: maybe some kind of early-exit mechanism for unknown peers (without a full PeerTable in `recv_some`)
        if let Some(peer) = peers.get_mut(&peer.addr()) {
          peer.received(packet, handler)?;
        }
      }
    }
  }

  Ok(())
}

struct PeerManager {
  table: PeerTable,
  // Safety: Every address in `queue` must be present in `table`
  queue: PeerQueue,
}

impl PeerManager {
  fn new(capacity: usize) -> Self {
    Self {
      table: PeerTable::with_capacity(capacity),
      queue: PeerQueue::new(capacity),
    }
  }

  fn is_full(&self) -> bool {
    self.table.len() == self.table.capacity()
  }

  /// Note: Do not call if `mgr.is_full()` is true
  fn add_peer(&mut self, addr: SocketAddr) {
    self.table.insert(addr, PeerState::new(addr));
    self.queue.put(addr);
  }

  fn remove_peer(&mut self, addr: SocketAddr) {
    self.table.remove(&addr);
    self.queue.remove(|v| *v == addr);
  }

  fn next_peer(&mut self) -> Option<&mut PeerState> {
    self.queue.get().and_then(|a| {
      // get the peer, then insert it back into the queue if it still exists in `table`
      let peer = self.table.get_mut(&a);
      self.queue.put(a);
      peer
    })
  }

  fn enqueue_packet(&mut self, addr: SocketAddr, packet: Packet) {
    if let Some(peer) = self.table.get_mut(&addr) {
      peer.packet_queue.put(packet);
    } else {
      // TODO: just drop packets or notify user?
    }
  }

  fn get(&self, addr: &SocketAddr) -> Option<&PeerState> {
    self.table.get(addr)
  }

  fn get_mut(&mut self, addr: &SocketAddr) -> Option<&mut PeerState> {
    self.table.get_mut(addr)
  }

  fn maintain(&mut self) {
    if self.queue.is_empty() {
      self.queue.swap();
    }
  }
}

pub const MTU: usize = 1024;

pub struct Config {
  pub protocol: Protocol,
  pub max_peers: usize,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      protocol: Protocol::from(0),
      max_peers: 64,
    }
  }
}

struct State<H: Handler> {
  addr: SocketAddr,
  socket: UdpSocket,
  chan: RawReceiver<Command>,
  handler: H,
  buffer: Vec<u8>,
  serializer: Serializer,
  deserializer: Deserializer,
  poll_timeout: Duration,
  poll: Poll,
  events: Events,
  max_peers: usize,
  peer_mgr: PeerManager,
  running: bool,
}

impl<H: Handler> State<H> {
  const SOCKET: Token = Token(0);
  fn new(config: Config, addr: SocketAddr, chan: RawReceiver<Command>, handler: H) -> Result<Self> {
    let mut socket = UdpSocket::bind(addr)?;
    let poll = Poll::new()?;
    poll.registry().register(
      &mut socket,
      Self::SOCKET,
      Interest::READABLE | Interest::WRITABLE,
    )?;
    Ok(Self {
      addr,
      socket,
      chan,
      handler,
      // enough to hold the maximum size of a UDP datagram
      buffer: vec![0u8; 1 << 16],
      serializer: Serializer::new(config.protocol),
      deserializer: Deserializer::new(config.protocol),
      poll_timeout: Duration::from_secs_f32(1.0 / 60.0),
      poll,
      events: Events::with_capacity(1024),
      max_peers: config.max_peers,
      peer_mgr: PeerManager::new(config.max_peers),
      running: true,
    })
  }

  fn run(&mut self) -> Result<()> {
    while self.running {
      // 1. socket events
      self.poll.poll(&mut self.events, Some(self.poll_timeout))?;
      for event in self.events.iter() {
        match event.token() {
          Self::SOCKET => {
            // write, then read
            if event.is_writable() {
              send_some(
                &self.serializer,
                &mut self.peer_mgr,
                &mut self.buffer,
                &self.socket,
              )?;
            }
            if event.is_readable() {
              recv_some(
                &self.deserializer,
                &mut self.peer_mgr.table,
                &mut self.buffer,
                &self.socket,
                &mut self.handler,
              )?;
            }
          }
          _ => unreachable!(),
        }
      }

      // TODO: 2. connections
      // we want to do this before commands to free up space in the peer manager, if possible

      // 3. commands
      loop {
        match self.chan.try_recv() {
          Ok(cmd) => match cmd {
            Command::Send {
              peer,
              payload,
              is_reliable,
            } => {
              self
                .peer_mgr
                .enqueue_packet(peer.addr(), Packet::new(payload, is_reliable));
            }
            Command::Disconnect { peer } => {} // TODO: disconnect peer
            Command::Shutdown { signal } => {
              // TODO: disconnect all peers
              signal.send(()).unwrap();
              self.running = false;
              break;
            }
          },
          Err(channel::TryRecvError::Empty) => break,
          Err(channel::TryRecvError::Disconnected) => return Ok(()),
        }
      }
    }

    Ok(())
  }
}

#[derive(Clone)]
pub struct Sender {
  chan: RawSender<Command>,
}

impl Sender {
  fn new(chan: RawSender<Command>) -> Self {
    Self { chan }
  }

  /// Send a packet to a peer.
  ///
  /// `payload.len()` must not be greater than `1024`.
  pub fn send(&self, peer: Peer, payload: Vec<u8>, is_reliable: bool) {
    if payload.len() > MTU {
      // TODO: fragmentation + reassembly
      panic!("Packet is larger than MTU");
    }
    // `unwrap` is fine because if the channel disconnects, then
    // the server is shutting down anyway, and you shouldn't
    // be sending any data.
    self
      .chan
      .send(Command::Send {
        peer,
        payload,
        is_reliable,
      })
      .expect("Command channel is disconnected, did you call `send` after `shutdown`?")
  }

  /// Attemp to gracefully disconnect a peer.
  pub fn disconnect(&self, peer: Peer) {
    self
      .chan
      .send(Command::Disconnect { peer })
      .expect("Command channel is disconnected, did you call `close` after `shutdown`?")
  }

  /// Attempt to gracefully shutdown. This call blocks until the server shuts down.
  pub fn shutdown(&self) {
    let (signal, wait) = channel::bounded(0);
    self
      .chan
      .send(Command::Shutdown { signal })
      .expect("Command channel is disconnected, did you call `shutdown` after `shutdown`?");
    wait.recv().unwrap();
  }
}

/// Bind the server on `addr` with default configuration, and start the event loop on a new thread.
pub fn listen<F, H>(addr: SocketAddr, factory: F) -> Result<(Sender, JoinHandle<Result<()>>)>
where
  F: FnOnce(Sender) -> H,
  H: Handler + Send + 'static,
{
  listen_with(Config::default(), addr, factory)
}

/// Bind the server on `addr` with custom configuration, and start the event loop on a new thread.
pub fn listen_with<F, H>(
  config: Config,
  addr: SocketAddr,
  factory: F,
) -> Result<(Sender, JoinHandle<Result<()>>)>
where
  F: FnOnce(Sender) -> H,
  H: Handler + Send + 'static,
{
  let (sender, receiver) = channel::bounded(32);
  let sender = Sender::new(sender);
  let handler = factory(sender.clone());
  let handle = thread::spawn(move || State::new(config, addr, receiver, handler)?.run());
  Ok((sender, handle))
}
