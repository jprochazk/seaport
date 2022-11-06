use std::time::{Duration, Instant};

pub const INITIAL_PMTU: u64 = 1232;

// TODO: pmtu probes

// New reno style controller
pub struct Window {
  cwnd: u64,
  sstresh: u64,
  acked: u64,
  recovery: Instant,
}

impl Window {
  pub fn new(now: Instant) -> Self {
    Self {
      cwnd: INITIAL_PMTU,
      sstresh: u64::MAX,
      acked: 0,
      recovery: now,
    }
  }

  pub fn on_ack(&mut self, sent_at: Instant, bytes: u64, pmtu: u64) {
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
        self.cwnd += pmtu;
      }
    }
  }

  // persistent congestion means there are two acks, A and B, where:
  // - B was received after A
  // - there was at least one packet sent inbetween the packets they acknowledge
  // - the interval between them is greater than `latency.
  pub fn on_loss(
    &mut self,
    now: Instant,
    sent_at: Instant,
    is_persistent: bool,
    pmtu: u64,
  ) {
    if sent_at <= self.recovery {
      return;
    }

    let minimum_window = 2 * pmtu;

    // enter recovery state.
    self.recovery = now;
    self.cwnd = std::cmp::max(minimum_window, self.cwnd / 2);
    self.sstresh = self.cwnd;

    // upon persistent loss, reset the window to minimum
    // this may cause the window to re-enter the `slow start` state.
    if is_persistent {
      self.cwnd = minimum_window;
    }
  }

  pub fn get(&self) -> u64 {
    self.cwnd
  }
}

pub struct Latency {
  /// The latency variance
  variance: Duration,
  /// The minimum observed latency
  min: Duration,
  /// Last latency measurement
  last: Duration,
  /// The smoothed latency
  smooth: Option<Duration>,
}

impl Latency {
  pub fn new() -> Self {
    let initial = Duration::from_millis(333);
    Self {
      variance: initial / 2,
      min: initial,
      last: initial,
      smooth: None,
    }
  }

  /// Best latency estimate available
  #[inline]
  pub fn get(&self) -> Duration {
    self.smooth.unwrap_or(self.last).max(self.last)
  }

  /// How long a sender should wait before treating a packet as lost
  #[inline]
  pub fn packet_timeout(&self, delay: Duration) -> Duration {
    self.get()
      + std::cmp::max(4 * self.variance, Duration::from_millis(2))
      + delay
  }

  // Based on RFC6298
  /// Update the latency estimate
  ///
  /// - latency: Time between the sender sending a packet and receiving an acknowledgement for it
  /// - delay: Time between the receiver receiving a packet and sending an acknowledgement for it
  ///
  /// `delay` should not be more than the configured `max_ack_delay`, and global `ACK_DELAY_LIMIT`.
  pub fn update(&mut self, latency: Duration, delay: Duration) {
    self.last = latency;
    self.min = std::cmp::min(self.min, latency);
    if let Some(smooth) = self.smooth {
      let delay_adjusted = if self.min + delay <= self.last {
        self.last - delay
      } else {
        self.last
      };
      let variance_sample = if smooth > delay_adjusted {
        smooth - delay_adjusted
      } else {
        delay_adjusted - smooth
      };
      self.variance = (3 * self.variance + variance_sample) / 4;
      self.smooth = Some((7 * smooth + delay_adjusted) / 8);
    } else {
      self.smooth = Some(self.last);
      self.variance = self.last / 2;
      self.min = self.last;
    }
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
    wnd.on_ack(now.get(), u64::MAX, INITIAL_PMTU); // boom
  }

  #[test]
  fn some_loss() {
    // under ideal conditions, we expect at least 1-2% packet loss.

    let mut now = Wait::new();
    let mut wnd = Window::new(now.get());

    // successfully send a few datagrams
    now.wait(ms!(10));
    wnd.on_ack(now.get(), 5 * INITIAL_PMTU, INITIAL_PMTU);
    assert_eq!(wnd.get(), 6 * INITIAL_PMTU);

    // then lose one
    let sent_at = now.get();
    now.wait(ms!(10));
    wnd.on_loss(now.get(), sent_at, false, INITIAL_PMTU);
    assert_eq!(wnd.get(), 3 * INITIAL_PMTU);

    // after the first packet loss, we enter congestion avoidance mode,
    // where the window increases by one PMTU each time the maximum amount
    // of bytes are acknowledged.
    // at least 3 datagrams need to be acked before the window increases again.
    now.wait(ms!(10));
    let sent_at = now.get();
    wnd.on_ack(sent_at, 3 * INITIAL_PMTU, INITIAL_PMTU);
    assert_eq!(wnd.get(), 4 * INITIAL_PMTU);

    // then 4, 5, 6, etc., forever, or until the next loss.
    now.wait(ms!(10));
    let sent_at = now.get();
    wnd.on_ack(sent_at, 4 * INITIAL_PMTU, INITIAL_PMTU);
    assert_eq!(wnd.get(), 5 * INITIAL_PMTU);
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
    wnd.on_ack(now.get(), 11 * INITIAL_PMTU, INITIAL_PMTU);
    assert_eq!(wnd.get(), 12 * INITIAL_PMTU);

    // lose a packet
    let sent_at = now.get();
    now.wait(ms!(10));
    wnd.on_loss(now.get(), sent_at, false, INITIAL_PMTU);
    assert_eq!(wnd.get(), 6 * INITIAL_PMTU);

    // then encounter many more lost packets
    now.wait(ms!(1));
    let sent_at = now.get();
    now.wait(ms!(10));
    wnd.on_loss(now.get(), sent_at, true, INITIAL_PMTU);
    assert_eq!(wnd.get(), 2 * INITIAL_PMTU);
    assert_eq!(wnd.sstresh, 3 * INITIAL_PMTU);

    // the window now increases by the amount of bytes acked again, until we
    // reach the "slow start threshold", which in this case is 3 datagrams.
    now.wait(ms!(10));
    wnd.on_ack(now.get(), INITIAL_PMTU, INITIAL_PMTU);
    assert_eq!(wnd.get(), 3 * INITIAL_PMTU);

    // at this point we re-enter congestion avoidance mode,
    // where the window increases by one PMTU for every `cwnd` bytes acknowledged.
    now.wait(ms!(10));
    let bytes = wnd.get();
    wnd.on_ack(now.get(), bytes, INITIAL_PMTU);
    assert_eq!(wnd.get(), 4 * INITIAL_PMTU);

    now.wait(ms!(10));
    let bytes = wnd.get();
    wnd.on_ack(now.get(), bytes, INITIAL_PMTU);
    assert_eq!(wnd.get(), 5 * INITIAL_PMTU);
  }
}
