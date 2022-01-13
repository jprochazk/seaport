use crate::{
  error::Result,
  packet::{Deserializer, Packet, Serializer},
  peer::{recv::Recv, recv_one, send_one, Handler, PeerState},
  Protocol,
};
use crossbeam::channel::{self, Receiver as RawReceiver, Sender as RawSender};
use mio::{net::UdpSocket, Events, Interest, Poll, Token};
use std::{
  net::{Ipv4Addr, SocketAddr},
  thread::{self, JoinHandle},
  time::{Duration, Instant},
};

fn send_some(
  serializer: &Serializer,
  peer: &mut PeerState,
  buffer: &mut [u8],
  socket: &UdpSocket,
) -> Result<()> {
  let now = Instant::now().elapsed();
  if now - peer.last_send > peer.send_interval {
    peer.last_send = now;
    send_one(serializer, peer, buffer, socket)?;
  }

  Ok(())
}

fn recv_some<H: Handler>(
  deserializer: &Deserializer,
  peer: &mut PeerState,
  buffer: &mut [u8],
  socket: &UdpSocket,
  handler: &mut H,
) -> Result<()> {
  loop {
    match recv_one(deserializer, buffer, socket)? {
      Recv::Stop => break,
      Recv::Bad(_) => {
        // TODO: disconnect
        todo!()
      }
      Recv::Packet(_, packet) => peer.received(packet, handler)?,
    }
  }

  Ok(())
}

enum Command {
  Send { payload: Vec<u8>, is_reliable: bool },
  Disconnect { signal: channel::Sender<()> },
}

// TODO: congestion control + fragmentation/assembly

pub const MTU: usize = 1024;

pub struct Config {
  /// An opaque value that represents you protocol (and its version).
  pub protocol: Protocol,
  /// The frequency at which packets are sent.
  pub send_rate: u32,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      protocol: Protocol::from(0),
      send_rate: 30,
    }
  }
}

struct State<H: Handler> {
  socket: UdpSocket,
  chan: RawReceiver<Command>,
  handler: H,
  buffer: Vec<u8>,
  serializer: Serializer,
  deserializer: Deserializer,
  poll_timeout: Duration,
  poll: Poll,
  events: Events,
  peer: PeerState,
  running: bool,
}

impl<H: Handler> State<H> {
  const SOCKET: Token = Token(0);
  fn new(config: Config, addr: SocketAddr, chan: RawReceiver<Command>, handler: H) -> Result<Self> {
    let mut socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0).into())?;
    socket.connect(addr)?;
    let poll = Poll::new()?;
    poll.registry().register(
      &mut socket,
      Self::SOCKET,
      Interest::READABLE | Interest::WRITABLE,
    )?;

    Ok(Self {
      socket,
      chan,
      handler,
      // enough to hold the maximum size of a UDP datagram
      buffer: vec![0u8; 1 << 16],
      serializer: Serializer::new(config.protocol),
      deserializer: Deserializer::new(config.protocol),
      poll_timeout: Duration::from_secs_f32(1.0 / (config.send_rate * 2) as f32),
      poll,
      events: Events::with_capacity(128),
      peer: PeerState::new(addr),
      running: true,
    })
  }

  fn run(&mut self) -> Result<()> {
    // TODO: connection handshake

    while self.running {
      // 1. socket events
      self.poll.poll(&mut self.events, Some(self.poll_timeout))?;
      for event in self.events.iter() {
        match event.token() {
          Self::SOCKET => {
            if event.is_writable() {
              send_some(
                &self.serializer,
                &mut self.peer,
                &mut self.buffer,
                &self.socket,
              )?;
            }
            if event.is_readable() {
              recv_some(
                &self.deserializer,
                &mut self.peer,
                &mut self.buffer,
                &self.socket,
                &mut self.handler,
              )?;
            }
          }
          _ => unreachable!(),
        }
      }

      // 2. commands
      loop {
        match self.chan.try_recv() {
          Ok(cmd) => match cmd {
            Command::Send {
              payload,
              is_reliable,
            } => {
              self
                .peer
                .packet_queue
                .put(Packet::new(payload, is_reliable));
            }
            Command::Disconnect { signal } => {
              // TODO: disconnect
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

  /// Send a packet.
  ///
  /// `payload.len()` must not be greater than `1024`.
  pub fn send(&self, payload: Vec<u8>, is_reliable: bool) {
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
        payload,
        is_reliable,
      })
      .expect("Command channel is disconnected, did you call `send` after `shutdown`?")
  }

  /// Attemp to gracefully disconnect.
  pub fn disconnect(&self) {
    let (signal, wait) = channel::bounded(1);
    self
      .chan
      .send(Command::Disconnect { signal })
      .expect("Command channel is disconnected, did you call `close` after `shutdown`?");
    wait.recv().unwrap();
  }
}

/// Bind the server on `addr` with default configuration, and start the event loop on a new thread.
pub fn connect<F, H>(addr: SocketAddr, factory: F) -> Result<(Sender, JoinHandle<Result<()>>)>
where
  F: FnOnce(Sender) -> H,
  H: Handler + Send + 'static,
{
  connect_with(Config::default(), addr, factory)
}

/// Bind the server on `addr` with custom configuration, and start the event loop on a new thread.
pub fn connect_with<F, H>(
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
