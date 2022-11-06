use crate::path::Window;

/// Manages the state of a single authenticated peer
pub struct Connection {
  cwnd: Window,
  // needs to manage:
  // - per channel:
  //   - message queue
  //   - reliable message buffer
  // - packet id sequence
  // - ack buffer
  // - pmtu
  // - latency
  // - max_ack_delay
  // - auth token
  // - address

  // for unreliable messages, try to fit the entire thing into a single packet
  // if it's not possible, fragment it as little as possible
  // for reliable messages, just grab segments until you fill a packet
  // once a reliable message has been sent in full, add it to the reliable message buffer
  //
  // TODO: persistent iteration for message buffers
  // with channels we can just store a cursor, because their number doesn't change
  // with message queues, once an element has been processed, it can be removed
  // with message buffers, it becomes tricky, because messages may be added or removed
  // at any time, so a simple cursor is not enough. need to come up with something better

  // TODO: design the entire protocol:
  // think of it in terms of the lifetime of the connection
  // starts with a handshake, sends data, then terminates at some point
  //
  // what does a handshake look like? what are the parameters? what are their limits?
  // what does sending data look like? congestion, fragmentation, encryption
  // channels vs streams
  //
  // ALSO think about the API surface:
  // - what does creating a connection look like?
  // - how does a connection terminate?
  // - are channels/streams configurable? if so, how? how does the user interact with them?
  // - how does the user receive data? how do they assign meaning to their channels/streams?
}
