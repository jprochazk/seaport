use mio::net::UdpSocket;
use std::{io, net::SocketAddr};

pub trait Socket {
  fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize>;
  fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)>;
}

impl Socket for UdpSocket {
  fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
    self.send_to(buf, target)
  }

  fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
    self.recv_from(buf)
  }
}
