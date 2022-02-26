use {
  mio::net,
  std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
  },
};

pub struct Socket {
  inner: net::UdpSocket,
  addr: SocketAddr,
  poll: mio::Poll,
  events: mio::Events,
}

const TOKEN: mio::Token = mio::Token(0);

pub struct Sender<'a>(&'a net::UdpSocket);

impl<'a> Sender<'a> {
  /// Sends a packet to `target`, returning:
  /// - `true` if the packet could be written, meaning you can send another
  /// - `false` if the packet could not be written
  ///
  /// # Panics
  /// - If `buf` is larger than `65536`.
  #[inline]
  pub fn send(&self, buf: &[u8]) -> io::Result<bool> {
    debug_assert!(buf.len() <= 1 << 16);
    match self.0.send(buf) {
      Ok(_) => Ok(true),
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(false),
      Err(e) => Err(e),
    }
  }

  /// Sends a packet to `target`, returning:
  /// - `true` if the packet could be written, meaning you can send another
  /// - `false` if the packet could not be written
  ///
  /// # Panics
  /// - If `buf` is larger than `65536`.
  #[inline]
  pub fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<bool> {
    debug_assert!(buf.len() <= 1 << 16);
    match self.0.send_to(buf, target) {
      Ok(_) => Ok(true),
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(false),
      Err(e) => Err(e),
    }
  }
}

/// Used to receive raw packets from the socket.
pub struct Receiver<'a>(&'a net::UdpSocket);

impl<'a> Receiver<'a> {
  /// Receives a packet into `buf` and returns a slice of the read bytes,
  /// or `None` if the socket would block.
  ///
  /// # Panics
  /// - If `buf.len()` is not equal to `65536`
  /// - If `.connect()` has not been called on the underlying socket
  #[inline]
  pub fn recv<'b>(&self, buf: &'b mut [u8]) -> io::Result<Option<&'b [u8]>> {
    debug_assert!(buf.len() == 1 << 16);
    match self.0.recv(buf) {
      Ok(len) => Ok(Some(&buf[..len])),
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
      Err(e) => Err(e),
    }
  }

  /// Receives a packet into `buf` and returns a slice of the read bytes,
  /// and the source socket address, or `None` if the socket would block.
  ///
  /// # Panics
  /// - If `buf.len()` is not equal to `65536`
  #[inline]
  pub fn recv_from<'b>(&self, buf: &'b mut [u8]) -> io::Result<Option<(&'b [u8], SocketAddr)>> {
    debug_assert!(buf.len() == 1 << 16);
    match self.0.recv_from(buf) {
      Ok((len, addr)) => Ok(Some((&buf[..len], addr))),
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
      Err(e) => Err(e),
    }
  }
}

impl Socket {
  pub fn listen(addr: SocketAddr) -> io::Result<Self> {
    let mut inner = net::UdpSocket::bind(addr)?;
    let addr = inner.local_addr()?;
    let poll = mio::Poll::new()?;
    poll.registry().register(
      &mut inner,
      TOKEN,
      mio::Interest::READABLE | mio::Interest::WRITABLE,
    )?;
    let events = mio::Events::with_capacity(1024);
    Ok(Self { inner, addr, poll, events })
  }

  pub fn connect(addr: SocketAddr) -> io::Result<Self> {
    let mut inner = net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0).into())?;
    inner.connect(addr)?;
    let addr = inner.peer_addr()?;
    let poll = mio::Poll::new()?;
    poll.registry().register(
      &mut inner,
      TOKEN,
      mio::Interest::READABLE | mio::Interest::WRITABLE,
    )?;
    let events = mio::Events::with_capacity(1024);
    Ok(Self { inner, addr, poll, events })
  }

  /// Poll the socket for readiness events.
  #[inline]
  pub fn poll_timeout<W, R>(
    &mut self,
    mut write: W,
    mut read: R,
    timeout: Duration,
  ) -> io::Result<()>
  where
    W: FnMut(Sender<'_>) -> io::Result<()>,
    R: FnMut(Receiver<'_>) -> io::Result<()>,
  {
    self.poll.poll(&mut self.events, Some(timeout))?;

    for event in self.events.iter() {
      if event.token() == TOKEN {
        if event.is_writable() {
          write(Sender(&self.inner))?;
        }
        if event.is_readable() {
          read(Receiver(&self.inner))?;
        }
      }
    }

    Ok(())
  }

  /// Poll the socket for readiness events.
  ///
  /// Default timeout is `10ms`.
  #[inline]
  pub fn poll<W, R>(&mut self, write: W, read: R) -> io::Result<()>
  where
    W: FnMut(Sender<'_>) -> io::Result<()>,
    R: FnMut(Receiver<'_>) -> io::Result<()>,
  {
    self.poll_timeout(write, read, Duration::from_millis(10))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn polling() {
    let mut server = Socket::listen("127.0.0.1:0".parse().unwrap()).unwrap();
    let mut client = Socket::connect(server.addr).unwrap();
    let addr = server.addr;
    let data: &[u8] = &[0u8; 4];
    let mut got_data = false;

    client
      .poll(
        |s| {
          s.send_to(data, addr)?;
          Ok(())
        },
        |_| Ok(()),
      )
      .unwrap();
    server
      .poll(
        |_| Ok(()),
        |r| {
          let buf: &mut [u8] = &mut [0u8; 1 << 16];
          let (d, _) = r.recv_from(buf)?.unwrap();
          assert_eq!(d, data);
          got_data = true;
          Ok(())
        },
      )
      .unwrap();

    assert!(got_data);
  }
}
