use {
  mio::{net::UdpSocket, Events, Interest, Poll, Token},
  std::{io, net::SocketAddr, sync::mpsc::channel, time::Duration},
};

fn init_log() {
  // default RUST_LOG=info
  std::env::set_var(
    "RUST_LOG",
    std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
  );
  env_logger::init();
}

fn main() {
  init_log();

  let addr = "127.0.0.1:9000";
  let mut socket = UdpSocket::bind(addr.parse().unwrap()).unwrap();
  log::info!("Server listening on {}", addr);

  let mut poll = Poll::new().unwrap();

  const SOCKET: Token = Token(0);
  poll
    .registry()
    .register(&mut socket, SOCKET, Interest::WRITABLE | Interest::READABLE)
    .unwrap();

  // messages are queued and echoed back to their source address
  let (sender, receiver) = channel::<(SocketAddr, String)>();

  let mut read_buffer = vec![0u8; 65536];

  let mut events = Events::with_capacity(128);
  loop {
    poll
      .poll(&mut events, Some(Duration::from_millis(100)))
      .unwrap();
    for event in events.iter() {
      match event.token() {
        // write one message per writable event
        SOCKET if event.is_writable() => {
          if let Ok((addr, msg)) = receiver.try_recv() {
            socket.send_to(msg.as_bytes(), addr).unwrap();
            log::info!("Echoed '{}'", msg);
          }
        }
        // read all packets
        SOCKET if event.is_readable() => loop {
          match socket.recv_from(read_buffer.as_mut_slice()) {
            Ok((len, addr)) => {
              log::info!("Received {} bytes from {}", len, addr);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
              break;
            }
            Err(e) => {
              return Err(e).unwrap();
            }
          }
        },
        _ => unreachable!(),
      }
    }
  }
}
