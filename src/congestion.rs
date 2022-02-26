//! Congestion controller based on [New Reno from quinn](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/congestion/new_reno.rs)

use std::time::Instant;

pub const DATAGRAM_SIZE: u64 = 1232;
pub const MINIMUM_WINDOW: u64 = 2 * DATAGRAM_SIZE;

pub struct Window {
  cwnd: u64,
  sstresh: u64,
  acked: u64,
  recovery: Instant,
  pmtu: u64,
}

impl Window {
  pub fn new(now: Instant) -> Self {
    Self { cwnd: DATAGRAM_SIZE, sstresh: u64::MAX, acked: 0, recovery: now, pmtu: DATAGRAM_SIZE }
  }

  pub fn set_pmtu(&mut self, v: u64) {
    self.pmtu = v;
  }

  pub fn on_ack(&mut self, sent_at: Instant, bytes: u64) {
    if sent_at <= self.recovery {
      return;
    }

    if self.cwnd < self.sstresh {
      // slow start
      self.cwnd += bytes;
      if self.cwnd >= self.sstresh {
        // exit slow start
        self.acked = self.cwnd - self.sstresh;
      }
    } else {
      // congestion avoidance
      self.acked += bytes;
      if self.acked >= self.cwnd {
        self.acked -= self.cwnd;
        self.cwnd += self.pmtu;
      }
    }
  }

  pub fn on_loss(&mut self, now: Instant, sent_at: Instant, is_persistent: bool) {
    if sent_at <= self.recovery {
      return;
    }

    // enter recovery state.
    self.recovery = now;
    self.cwnd = u64::max(MINIMUM_WINDOW, self.cwnd / 2);
    self.sstresh = self.cwnd;

    // upon persistent loss, reset the window to minimum
    // this may cause the window to re-enter the `slow start` state.
    if is_persistent {
      self.cwnd = MINIMUM_WINDOW;
    }
  }

  pub fn get(&self) -> u64 {
    self.cwnd
  }
}

#[cfg(test)]
mod tests {
  use {super::*, std::time::Duration};

  struct Wait(Instant);
  impl Wait {
    fn new() -> Self {
      Self(Instant::now())
    }
    fn wait(&mut self, duration: Duration) {
      self.0 += duration;
    }
    fn get(&self) -> Instant {
      self.0
    }
  }

  macro_rules! ms {
    ($v:literal) => {
      Duration::from_millis($v)
    };
  }

  #[test]
  #[should_panic]
  fn no_loss() {
    // under perfect conditions, no loss ever occurs, and the window keeps growing until it overflows.
    // this cannot happen in practice, as no network has 0% packet loss, and even in case some do,
    // at 64 KiB per datagram, it would take well over 281 trillion packets to achieve the
    // maximum window size (`u64::MAX`)

    // still, here we assert that the window can overflow and will panic in that case.

    let mut now = Wait::new();
    let mut wnd = Window::new(now.get());
    now.wait(ms!(10));
    wnd.on_ack(now.get(), u64::MAX); // boom
  }

  #[test]
  fn some_loss() {
    // under ideal conditions, we expect at least 1-2% packet loss.

    let mut now = Wait::new();
    let mut wnd = Window::new(now.get());

    // successfully send a few datagrams
    now.wait(ms!(10));
    wnd.on_ack(now.get(), 5 * DATAGRAM_SIZE);
    assert_eq!(wnd.get(), 6 * DATAGRAM_SIZE);

    // then lose one
    let sent_at = now.get();
    now.wait(ms!(10));
    wnd.on_loss(now.get(), sent_at, false);
    assert_eq!(wnd.get(), 3 * DATAGRAM_SIZE);

    // after the first packet loss, we enter congestion avoidance mode,
    // where the window increases by one PMTU each time the maximum amount
    // of bytes are acknowledged.
    // at least 3 datagrams need to be acked before the window increases again.
    now.wait(ms!(10));
    let sent_at = now.get();
    wnd.on_ack(sent_at, 3 * DATAGRAM_SIZE);
    assert_eq!(wnd.get(), 4 * DATAGRAM_SIZE);

    // then 4, 5, 6, etc., forever, or until the next loss.
    now.wait(ms!(10));
    let sent_at = now.get();
    wnd.on_ack(sent_at, 4 * DATAGRAM_SIZE);
    assert_eq!(wnd.get(), 5 * DATAGRAM_SIZE);
  }

  #[test]
  fn persistent_loss() {
    // under very bad conditions, up to 100% of packets may be dropped for a short period of time
    // this is "persistent" loss, and it is handle it by dropping the window to the bare minimum,
    // and switching to the slow start state.

    let mut now = Wait::new();
    let mut wnd = Window::new(now.get());

    // sucessfully send a few packets
    now.wait(ms!(10));
    wnd.on_ack(now.get(), 11 * DATAGRAM_SIZE);
    assert_eq!(wnd.get(), 12 * DATAGRAM_SIZE);

    // lose a packet
    let sent_at = now.get();
    now.wait(ms!(10));
    wnd.on_loss(now.get(), sent_at, false);
    assert_eq!(wnd.get(), 6 * DATAGRAM_SIZE);

    // then encounter many more lost packets
    now.wait(ms!(1));
    let sent_at = now.get();
    now.wait(ms!(10));
    wnd.on_loss(now.get(), sent_at, true);
    assert_eq!(wnd.get(), 2 * DATAGRAM_SIZE);
    assert_eq!(wnd.sstresh, 3 * DATAGRAM_SIZE);

    // the window now increases by the amount of bytes acked again, until we
    // reach the "slow start threshold", which in this case is 3 datagrams.
    now.wait(ms!(10));
    wnd.on_ack(now.get(), DATAGRAM_SIZE);
    assert_eq!(wnd.get(), 3 * DATAGRAM_SIZE);

    // at this point we re-enter congestion avoidance mode,
    // where the window increases by one PMTU for every `cwnd` bytes acknowledged.
    now.wait(ms!(10));
    let bytes = wnd.get();
    wnd.on_ack(now.get(), bytes);
    assert_eq!(wnd.get(), 4 * DATAGRAM_SIZE);

    now.wait(ms!(10));
    let bytes = wnd.get();
    wnd.on_ack(now.get(), bytes);
    assert_eq!(wnd.get(), 5 * DATAGRAM_SIZE);
  }
}
