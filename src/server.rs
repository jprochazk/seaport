use crate::{
  error::Error,
  packet::{Deserializer, Packet, Serializer},
  peer::{recv_some, send_some, Handler, Peer, PeerManager},
  Protocol,
};
use crossbeam::channel::{self, Receiver as RawReceiver, Sender as RawSender};
use mio::{net::UdpSocket, Events, Interest, Poll, Token};
use rand::{rngs::SmallRng, SeedableRng};
use std::{net::SocketAddr, time::Duration};

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
  scratch_space: Vec<u8>,
  serializer: Serializer,
  deserializer: Deserializer,
  // TODO: configurable timeout (based on send rate)
  poll_timeout: Duration,
  poll: Poll,
  events: Events,
  max_peers: usize,
  peer_mgr: PeerManager,
  rng: SmallRng,
}

impl<H: Handler> State<H> {
  const SOCKET: Token = Token(0);
  fn new(
    config: Config,
    addr: SocketAddr,
    chan: RawReceiver<Command>,
    handler: H,
  ) -> Result<Self, Error> {
    let mut socket = UdpSocket::bind(addr)?;
    // enough to hold the maximum size of a UDP datagram
    let scratch_space = vec![0u8; 1 << 16];
    let poll_timeout = Duration::from_secs_f32(1.0 / 60.0);
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
      scratch_space,
      serializer: Serializer::new(config.protocol),
      deserializer: Deserializer::new(config.protocol),
      poll_timeout,
      poll,
      events: Events::with_capacity(1024),
      max_peers: config.max_peers,
      peer_mgr: PeerManager::new(config.max_peers),
      rng: SmallRng::from_entropy(),
    })
  }

  fn run(&mut self) {
    loop {
      if let Err(e) = self.run_inner() {
        self.handler.on_error(e);
        break;
      }
    }
  }

  fn run_inner(&mut self) -> Result<(), Error> {
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
              &mut self.scratch_space,
              &self.socket,
            )?;
          }
          if event.is_readable() {
            recv_some(
              &self.deserializer,
              &mut self.peer_mgr,
              &mut self.scratch_space,
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
          } => self
            .peer_mgr
            .enqueue_packet(peer.addr(), Packet::new(payload, is_reliable)),
          Command::Disconnect { peer } => {} // TODO: disconnect peer
          Command::Shutdown { signal } => {
            // TODO: disconnect all peers
            let _ = signal.send(());
          }
        },
        Err(channel::TryRecvError::Empty) => break Ok(()),
        Err(channel::TryRecvError::Disconnected) => return Ok(()),
      }
    }
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

/// Bind the server on `addr` and start the event loop.
///
/// This runs until the server encounters an error or is shut down.
pub fn listen<F, H>(addr: SocketAddr, factory: F) -> Result<(), Error>
where
  F: FnOnce(Sender) -> H,
  H: Handler,
{
  listen_with(Config::default(), addr, factory)
}

/// Bind the server on `addr` and start the event loop.
///
/// This runs until the server encounters an error or is shut down.
pub fn listen_with<F, H>(config: Config, addr: SocketAddr, factory: F) -> Result<(), Error>
where
  F: FnOnce(Sender) -> H,
  H: Handler,
{
  let (send, recv) = channel::bounded(32);
  State::new(config, addr, recv, factory(Sender::new(send)))?.run();
  Ok(())
}
