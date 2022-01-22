use mio::net;
use std::{
  io,
  net::{Ipv4Addr, SocketAddr},
  time::Duration,
};

// TODO: how does sequencing work with channels?
// TODO: remove the assumption that we're sending at 30 Hz.

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
    set_dont_fragment(&inner)?;
    let addr = inner.local_addr()?;
    let poll = mio::Poll::new()?;
    poll.registry().register(
      &mut inner,
      TOKEN,
      mio::Interest::READABLE | mio::Interest::WRITABLE,
    )?;
    let events = mio::Events::with_capacity(1024);
    Ok(Self {
      inner,
      addr,
      poll,
      events,
    })
  }

  pub fn connect(addr: SocketAddr) -> io::Result<Self> {
    let mut inner = net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0).into())?;
    set_dont_fragment(&inner)?;
    inner.connect(addr)?;
    let addr = inner.peer_addr()?;
    let poll = mio::Poll::new()?;
    poll.registry().register(
      &mut inner,
      TOKEN,
      mio::Interest::READABLE | mio::Interest::WRITABLE,
    )?;
    let events = mio::Events::with_capacity(1024);
    Ok(Self {
      inner,
      addr,
      poll,
      events,
    })
  }

  /// Poll the socket for readiness events.
  ///
  /// Writable events will trigger `on_writable`, Readable events will trigger `on_readable`.
  #[inline]
  pub fn poll<W, R>(&mut self, mut on_writable: W, mut on_readable: R) -> io::Result<()>
  where
    W: FnMut(Sender<'_>) -> io::Result<()>,
    R: FnMut(Receiver<'_>) -> io::Result<()>,
  {
    self
      .poll
      .poll(&mut self.events, Some(Duration::from_millis(10)))?;

    for event in self.events.iter() {
      if event.token() == TOKEN {
        if event.is_writable() {
          on_writable(Sender(&self.inner))?;
        }
        if event.is_readable() {
          on_readable(Receiver(&self.inner))?;
        }
      }
    }

    Ok(())
  }
}

fn set_dont_fragment(socket: &net::UdpSocket) -> io::Result<()> {
  use std::mem::size_of_val;

  #[cfg(windows)]
  if socket.local_addr()?.is_ipv4() {
    use std::os::windows::prelude::AsRawSocket;
    use winapi::ctypes::c_int;
    use winapi::shared::ws2def::IPPROTO_IP;
    use winapi::shared::ws2ipdef::IP_DONTFRAGMENT;
    use winapi::um::winsock2::setsockopt;
    use winapi::um::winsock2::SOCKET;

    unsafe {
      let on: c_int = 1;
      let r = setsockopt(
        socket.as_raw_socket() as SOCKET,
        IPPROTO_IP,
        IP_DONTFRAGMENT,
        &on as *const _ as _,
        size_of_val(&on) as _,
      );
      if r == -1 {
        return Err(io::Error::last_os_error());
      }
    }
  }

  #[cfg(unix)]
  if socket.local_addr()?.is_ipv4() {
    use libc::c_int;
    use libc::setsockopt;
    use libc::IPPROTO_IP;
    use libc::IP_DONTFRAG;
    use libc::SOCKET;
    use std::os::unix::io::AsRawFd;
    unsafe {
      let on: c_int = 1;
      let r = setsockopt(
        socket.as_raw_fd(),
        IPPROTO_IP,
        IP_DONTFRAG,
        &on as *const _ as _,
        size_of_val(&on) as _,
      );
      if r == -1 {
        return Err(io::Error::last_os_error());
      }
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_set_dont_fragment() {
    let socket = net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0).into()).unwrap();
    set_dont_fragment(&socket).unwrap();
  }
}
