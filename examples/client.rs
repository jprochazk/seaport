use std::{io, net, time::Instant};

fn init_log() {
  // default RUST_LOG=info
  std::env::set_var(
    "RUST_LOG",
    std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
  );
  env_logger::init();
}

fn main() -> io::Result<()> {
  init_log();

  const DATA: &[u8] = b"testtesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttesttest";

  let socket = net::UdpSocket::bind("0.0.0.0:0").unwrap();
  socket.connect("127.0.0.1:9000").unwrap();
  socket.set_nonblocking(true).unwrap();

  let mut nb_sent = 0usize;
  let mut blocked_at = Option::<Instant>::None;
  loop {
    match socket.send(DATA) {
      Ok(..) => {
        nb_sent += DATA.len();
      }
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
        if let Some(t) = &blocked_at {
          log::info!("blocked for {} ns", t.elapsed().as_nanos());
        }
        log::info!("{nb_sent}");
        nb_sent = 0;
        blocked_at = Some(Instant::now());
      }
      Err(e) => return Err(e),
    }
  }
}
