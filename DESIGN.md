# seaport

# Usage overview

## Shared
```rust
use seaport::prelude::shared::*;

fn generate_challenge_token(data: &mut [u8]) {
  generate_random_bytes(data);
}
fn solve_challenge_token(data: &mut [u8]) {
  data[0] = data[0].wrapping_add(1);
}
fn check_challenge_token(unsolved: &[u8], solved: &[u8]) -> bool {
  unsolved[0].wrapping_add(1) == solved[0]
}

const PROTOCOL: Protocol = Protocol(0);

mod channel {
  use super::*;
  pub const UNRELIABLE: Token = Token(0);
  pub const RELIABLE: Token = Token(1);
  pub const RELIABLE_ORDERED: Token = Token(2);
}

const SHARED_CONFIG: Config = Config {
  protocol: PROTOCOL,
  challenge: Some(Challenge::new(
    /* generate: */ generate_challenge_token,
    /* solve: */ solve_challenge_token,
    /* check: */ check_challenge_token,
  )),
  channels: Some(Channels::new()
    .unreliable(channel::UNRELIABLE)
    .reliable(channel::RELIABLE)
    .reliable_ordered(channel::RELIABLE_ORDERED)
    .finish()
  ),
  max_ack_delay: Some(25),
  timeout: Some(5000)
};
```

## Client

```rust
use seaport::prelude::client::*;

struct State;
impl State {
  fn update(&mut self, client: &mut Client) {
    /* ... */
    client.send(channel::UNRELIABLE, &[0u8; 1024]);
  }
}
impl Handler for State {
  fn on_message(&mut self, peer: Peer, token: u16, data: &[u8]) { /* ... */ }
  fn on_stream(&mut self, peer: Peer, token: u16, data: &[u8]) { /* ... */ }
  fn on_disconnect(&mut self, reason: Reason) { /* ... */ }
}

let identity_token = /* received from authentication service */;

let mut client = SHARED_CONFIG.connect("127.0.0.1:8080", identity_token)?;
let mut state = State;
loop {
  client.poll(&mut state);
  state.update(&mut client);
}
```

## Server

```rust
use seaport::prelude::server::*;

struct State;
impl State {
  fn update(&mut self, server: &mut Server) {
    /* ... */
    server.send(channel::UNRELIABLE, &[0u8; 1024]);
  }
}
impl Handler for State {
  fn on_message(&mut self, peer: Peer, token: u16, data: &[u8]) { /* ... */ }
  fn on_stream(&mut self, peer: Peer, token: u16, data: &[u8]) { /* ... */ }
  fn on_connect(&mut self, peer: Peer) { /* ... */ }
  fn on_disconnect(&mut self, peer: Peer, reason: Reason) { /* ... */ }
}

let mut server = SHARED_CONFIG.listen("127.0.0.1:8080")?;
let mut state = State;

loop {
  server.poll(&mut state);
  state.update(&mut server);
}
```

# Protocol

A connection-oriented semi-reliable protocol for games based on UDP.

Notable features:
- Optional reliability and ordering
- Channels
- Virtual connections
- Congestion control
- Path MTU discovery
- Encryption
- Automatic fragmentation + reassembly

Reliability and ordering are the main features, but how we get there is important, too. Each connection consists of channels, which allow for multiplexing on a single socket. The approach taken to connections and encryption bypasses most issues by depending on a secure third party channel, which is used to exchange encryption keys, and identity info. Congestion control and PMTU discovery automatically handle send rate limiting, instead of having it be based on a configurable bandwidth value. This gives the library a lot of freedom to fragment + reassemble messages as required, and coalesce segments of messages into packets, taking full advantage of the available PMTU.

## Connection handshake

The goal of the handshake is to securely establish transport parameters and identity.

Transport parameters are:
- Channel configuration
- Maximum packet processing delay
- Timeout

Before the connection to the server may begin, the client authenticates via the authentication service, receiving an identity token.

TODO: maybe the challenge can be optional?

The connection procedure:
1. The client initiates the connection using the identity token, and transitions into the `connecting` state.
1. The server receives the connection request, validates the identity token, stores the client as `connecting`, and responds with a connection challenge.
1. The client receives the connection challenge, and responds with the solution.
1. The server receives the connection challenge response, checks the solution, transitions the client from `connecting` to `connected`, and sends a connection success.
1. The client receives the connection success, and transitions into the `connected` state.

Each packet is sent repeatedly until a response or timeout, with monotonically increasing sequence numbers.

Once the server/client reach the `connected` state, channels are constructed, and the regular send/receive loop begins.

### Packets

Connection error
```rust
kind: u8 = 0
sequence: u32
protocol: u64

reason: u8
message: str
```

`reason` has these known values:
- 0 = invalid packet
- 1 = timed out
- 2 = server is full

`message` describes the specific error that occurred.

The underlying reasons for `invalid packet` include, but are not limited to:
- Invalid packet length
- Packet decryption failed
- Invalid protocol ID
- Invalid identity token
- Identity token already in use
- Identity token expired

In release mode, `message` is always empty.

Connection request
```rust
kind: u8 = 1
sequence: u32
protocol: u64

// transport parameters
num_channels: u16
num_streams: u16
max_ack_delay: u16
timeout: u16

// identity token
created_at: u64
expires_at: u64
server_to_client_key: u32
client_to_server_key: u32
nonce: [u8; 24]
identity_token: [u8; 1024] {
  client_id: u128
  created_at: u64
  expires_at: u64
  server_to_client_key: u32
  client_to_server_key: u32
  user_data: [u8; 984]
}
```

`identity_token` is encrypted using a private key shared between the authentication service and the server.

`user_data` is contains an arbitrary payload. The payload is zero-padded, and it is the user's responsibility to know the length of the data.

Connection challenge
```rust
kind: u8 = 2
sequence: u32
protocol: u64

client_id: u128
challenge_data: [u8; 256];
padding: [u8; 739];
```

The application is responsible for generating and checking `challenge_data` on the server-side, and solving it on the client-side.

Connection success
```rust
kind: u8 = 3
sequence: u32
protocol: u64
```

Packet
```rust
kind: u8 = 4
sequence: u32
protocol: u64
[u8; ...]
```

NOTE: validation
- Packet length must match expected based on the current state
- Packet must decrypt successfully
- Packet sequence number must be greater than last recorded sequence number
- (Server) Identity token must decrypt successfully
- (Server) Client ID must match
- (Server) Identity token must not be expired

## Channels, Messages, Segments

Channels facilitate full-duplex transmission of messages. Messages are 

- unreliable + unordered
- reliable + unordered
- reliable + ordered

Each connection has its own set of full-duplex channels. Each channel is sequenced independently of other channels. Each channel sends data in the order it was queued.

The maximum size of a message that can be sent over a channel is 32KiB (32768 bytes).

INVARIANT: channels must never receive a duplicate ack

### Unreliable

sending: avoid fragmentation. data is discarded as soon as it is sent, as we do not require re-sending.

receiving: on the receive side, discard the whole message if any of its segments arrives out-of-order, or after 1 RTT has passed. upon receiving all the segments of a message, return it.

QQQ: maybe discarding out of order data is too harsh?

acknowledging: no action required for acks.

```rust
let msg_id = 0;
let buffer = Vec::<u8>::with_capacity(2048);

fn push(data: &[u8]) {
  for chunk in data.chunks()
  buffer.write(Segment { msg_id, data });
}

fn send(packet: &mut Packet) -> usize {
  let mut pos = 0;
  let mut len = usize::from(&buffer[pos..pos+4]);
  while packet.remaining() <= len {
    pos += 4;
    packet.write_segment(

    )
  }
}

recv ({msg_id, total_len, offset, data}, output) {}
ack (msg_id, offset) {}
lost (msg_id, offset) {}
```

### Reliable

Data is fragmented into segments, each of which is tracked independently. When sending, segments are discarded only once they are acknowledged. 

TODO: use the hashmap to store segment data, queue only stores keys
TODO: receive stores segment data in a hashmap
QQQ: how to track full messages on receive? a second hashmap? they can be received in an arbitrary order...

sending: fragment data into segments, each of which is queued independently as an unsent segment. upon dequeueing an unsent segment, re-queue it as a pending segment, add it to the ack buffer, and send it. The ack buffer is a hashmap associating segment IDs with ack status. upon dequeueing a pending segment, check if it has been acked, if not, re-queue it + send it, otherwise discard it.

receiving: if it has not been done yet, allocate enough space for the whole message in the recv buffer. copy the segment into the buffer at the message offset + segment offset. the recv buffer is a `VecDeque<u8>`, and the message state is stored in a `HashMap<u32, State>`. the state contains the offset into the recv buffer, the (unpadded) length, and the number of unacked segments. upon receiving all the segments of a message, return it.

acknowledging: set any acknowledged segments' ack status to true.

```rust
msg_id = 0
send_queue = []
msg_buffer = {}

push(data) {
  for (chunk, offset) in data.chunks(256).enumerate() {
    key = key(msg_id, offset)
    msg_buffer[key] = { msg_id, total_len: data.len, offset, data: chunk }
    send_queue.push(key)
  }
  msg_id += 1
}
send(packet) {
  while packet.remaining() > send_queue.peek().len() {
    packet.push_segment(send_queue.dequeue())
  }
}
recv({msg_id, total_len, offset, data}, output) {

}
ack(msg_id, offset) {
  key = key(msg_id, offset)
  msg_buffer.delete(key)
}
```

### Reliable + ordered

TODO: send works the same way as reliable
TODO: recv stores segments in a hashmap, but also a sorted stack where the top is always the oldest message

sending: fragment data into segments, each of which is queued independently as an unsent segment. upon dequeueing an unsent segment, re-queue it as a pending segment, add it to the ack buffer, and send it. The ack buffer is a hashmap associating segment IDs with ack status. upon dequeueing a pending segment, check if it has been acked, if not, re-queue it + send it, otherwise discard it.

receiving: if it has not been done yet, allocate enough space for the whole message in the recv buffer. copy the segment into the buffer at the message offset + segment offset. the recv buffer is a `VecDeque<u8>`, and the message offsets are stored in a `HashMap<MessageId, MessageOffset>`. upon receiving all the segments of a message, check if it is the oldest pending message, if yes, then return it and all the complete messages after it, until the first incomplete message, which then becomes the oldest pending message.

acknowledging: set any acknowledged segments' ack status to true.

```rust
push(data) {}
send(packet) {}
recv(msg_id, total_len, segment_offset, data) {}
ack(msg_id, segment_offset, segment_length) {}
```

## Packet structure

coalescing process
acknowledgement process
encryption

### Frames

what they are, why they exist

#### Ack frames

ack ranges
frequency
binary encoding

#### Segment frames

binary encoding

#### Padding

binary encoding

## Path information

### PMTU

pmtu probes

### Latency

how the estimate works

### Congestion

congestion window
which things factor and dont factor into congestion window
loss + persistent loss