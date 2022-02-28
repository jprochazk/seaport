use crate::path::Window;

/// Manages the state of a single authenticated peer
pub struct Connection {
  cwnd: Window,
}
