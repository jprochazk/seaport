use crate::{peer::PeerState, seq, server::MTU, Protocol};
use std::{
  mem::size_of,
  time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq)]
pub struct PacketData<'a> {
  sequence: u32,
  ack: u32,
  ack_bits: u32,
  payload: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct PacketInfo {
  pub acked: bool,
  /// Time since EPOCH when the packet was sent or received, depending on context
  pub time: Duration,
}

impl Default for PacketInfo {
  fn default() -> Self {
    Self {
      acked: false,
      time: Instant::now().elapsed(),
    }
  }
}

pub type Buffer = seq::Buffer<PacketInfo>;

#[derive(Debug, Clone)]
pub struct Unsent {
  pub payload: Vec<u8>,
  pub is_reliable: bool,
}
#[derive(Debug, Clone)]
pub struct Pending {
  pub payload: Vec<u8>,
  pub sequence: u32,
  pub sent_at: Duration,
}
#[derive(Debug, Clone)]
pub enum Packet {
  Unsent(Unsent),
  /// A pending packet represents a reliable packet that is awaiting re-transmission in case it is lost.
  /// `is_reliable == true` is implied
  Pending(Pending),
}

impl Packet {
  pub fn new(payload: Vec<u8>, is_reliable: bool) -> Self {
    Packet::Unsent(Unsent {
      payload,
      is_reliable,
    })
  }

  #[inline]
  pub fn unsent(payload: Vec<u8>, is_reliable: bool) -> Packet {
    Packet::Unsent(Unsent {
      payload,
      is_reliable,
    })
  }

  #[inline]
  pub fn pending(payload: Vec<u8>, sequence: u32, sent_at: Duration) -> Packet {
    Packet::Pending(Pending {
      payload,
      sequence,
      sent_at,
    })
  }

  #[inline]
  pub fn into_payload(self) -> Vec<u8> {
    match self {
      Packet::Unsent(Unsent { payload, .. }) => payload,
      Packet::Pending(Pending { payload, .. }) => payload,
    }
  }

  /// Safety: `matches!(self, Unsent(..))` must be true.
  #[inline]
  pub unsafe fn into_unsent_unchecked(self) -> Unsent {
    match self {
      Packet::Unsent(v) => v,
      Packet::Pending(_) => core::hint::unreachable_unchecked(),
    }
  }

  /// Safety: `matches!(self, Pending(..))` must be true.
  #[inline]
  pub unsafe fn into_pending_unchecked(self) -> Pending {
    match self {
      Packet::Pending(v) => v,
      Packet::Unsent(_) => core::hint::unreachable_unchecked(),
    }
  }
}

pub trait AckBits {
  type Info;
  fn get_ack_bits(&self, start: u32) -> u32;
  fn set_ack_bits(&mut self, start: u32, bits: u32);
}

impl AckBits for Buffer {
  type Info = PacketInfo;
  fn get_ack_bits(&self, start: u32) -> u32 {
    let mut ack_bits = 0u32;
    for n in 0..32 {
      // for every entry that is acked, set its corresponding bit
      let sequence = start.wrapping_sub(n + 1);
      if self.get(sequence).filter(|v| v.acked).is_some() {
        ack_bits |= 1 << n;
      }
    }
    ack_bits
  }

  fn set_ack_bits(&mut self, start: u32, bits: u32) {
    // the ack representation stores 33 acks
    // the first one is checked separately
    if let Some(entry) = self.get_mut(start) {
      if !entry.acked {
        entry.acked = true;
      }
    }
    // then we check the bits, which represent sequence `start - (N + 1)`,
    // where `N` is the bit position.
    for n in 0..32 {
      if (bits >> n) & 1 == 1 {
        // if bit `N` is set, then we get the packet info at `start - (N + 1)`
        let sequence = start.wrapping_sub(n + 1);
        // ack it if it is not already acked
        if let Some(entry) = self.get_mut(sequence).filter(|e| !e.acked) {
          entry.acked = true;
        }
      }
    }
  }
}

impl<'a> PacketData<'a> {
  pub(crate) fn new(peer: &PeerState, payload: &'a [u8]) -> Self {
    Self {
      sequence: peer.local_sequence,
      ack: peer.remote_sequence,
      ack_bits: peer.recv_buffer.get_ack_bits(peer.remote_sequence),
      payload,
    }
  }

  pub(crate) fn from_pending(peer: &PeerState, packet: &'a Pending) -> Self {
    Self {
      sequence: packet.sequence,
      ack: peer.remote_sequence,
      ack_bits: peer.recv_buffer.get_ack_bits(peer.remote_sequence),
      payload: &packet.payload,
    }
  }

  #[inline]
  pub fn sequence(&self) -> u32 {
    self.sequence
  }

  #[inline]
  pub fn ack(&self) -> u32 {
    self.ack
  }

  #[inline]
  pub fn ack_bits(&self) -> u32 {
    self.ack_bits
  }

  #[inline]
  pub fn payload(&self) -> &[u8] {
    self.payload
  }
}

#[rustfmt::skip]
const MIN_PACKET_SIZE: u64 = (
  /* protocol */  size_of::<u64>() +
  /* sequence */  size_of::<u32>() + 
  /* ack */       size_of::<u32>() +
  /* ack_bits */  size_of::<u32>()
) as u64;
const MAX_PACKET_SIZE: u64 = MIN_PACKET_SIZE + MTU as u64;

pub struct Serializer {
  protocol: Protocol,
}

impl Serializer {
  pub fn new(protocol: Protocol) -> Self {
    Self { protocol }
  }

  /// # Panics
  /// If `buffer` is not large enough to hold `size_of::<PacketData> + MTU`.
  pub fn serialize(&self, buffer: &mut [u8], data: PacketData<'_>) -> usize {
    assert!(buffer.len() >= MAX_PACKET_SIZE as usize);
    use std::io::{Cursor, Write};
    let mut writer = Cursor::new(buffer);
    let _ = writer.write(&self.protocol.0.to_le_bytes());
    let _ = writer.write(&data.sequence.to_le_bytes());
    let _ = writer.write(&data.ack.to_le_bytes());
    let _ = writer.write(&data.ack_bits.to_le_bytes());
    let _ = writer.write(data.payload);
    writer.position() as usize
  }
}

pub struct Deserializer {
  protocol: Protocol,
}

struct Reader<'a> {
  cursor: usize,
  data: &'a [u8],
}

impl<'a> Reader<'a> {
  fn new(data: &'a [u8]) -> Self {
    Self { cursor: 0, data }
  }

  fn advance(&mut self, n: usize) -> usize {
    let c = self.cursor;
    self.cursor += n;
    c
  }

  fn rest(&mut self) -> &'a [u8] {
    &self.data[self.cursor..]
  }

  fn slice(&mut self, n: usize) -> &'a [u8] {
    let start = self.advance(n);
    &self.data[start..start + n]
  }

  fn array<const N: usize>(&mut self) -> [u8; N] {
    // Safety: We read slices of size `N`
    unsafe { *(self.slice(N).as_ptr() as *const [u8; N]) }
  }

  fn u32(&mut self) -> u32 {
    u32::from_le_bytes(self.array())
  }

  fn u64(&mut self) -> u64 {
    u64::from_le_bytes(self.array())
  }
}

impl Deserializer {
  pub fn new(protocol: Protocol) -> Self {
    Self { protocol }
  }

  pub fn deserialize<'a>(&self, buffer: &'a [u8]) -> Option<PacketData<'a>> {
    let mut reader = Reader::new(buffer);

    if buffer.len() < MIN_PACKET_SIZE as usize
      || buffer.len() > MAX_PACKET_SIZE as usize
      || Protocol(reader.u64()) != self.protocol
    {
      None
    } else {
      Some(PacketData {
        sequence: reader.u32(),
        ack: reader.u32(),
        ack_bits: reader.u32(),
        payload: reader.rest(),
      })
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn ack_bits() {
    let mut remote_sequence = 0;
    let mut recv_buffer = Buffer::new(64);

    assert_eq!(recv_buffer.get_ack_bits(remote_sequence), 0);

    // we received 32 + 1 packets
    for n in 0..(32 + 1) {
      recv_buffer.insert(
        n,
        PacketInfo {
          acked: true,
          time: Instant::now().elapsed(),
        },
      );
      remote_sequence = n;
    }

    // but the earliest 4 of them got lost
    for i in 0..4 {
      recv_buffer.get_mut(i).unwrap().acked = false;
    }

    assert_eq!(remote_sequence, 32);
    assert_eq!(
      recv_buffer.get_ack_bits(remote_sequence),
      // the highest 4 bits are missing
      0b0000_1111_1111_1111_1111_1111_1111_1111u32
    );
  }

  #[test]
  fn serialize_deserialize_valid() {
    let protocol = Protocol(0);
    let serializer = Serializer::new(protocol);
    let deserializer = Deserializer::new(protocol);
    let mut buffer = [0u8; MAX_PACKET_SIZE as usize];

    let expected = PacketData {
      sequence: 255,
      ack: 255,
      ack_bits: 0,
      payload: &[3, 2, 1, 0],
    };
    let size = serializer.serialize(&mut buffer, expected.clone());

    assert_eq!(deserializer.deserialize(&buffer[0..size]), Some(expected));
  }

  #[test]
  fn deserialize_small() {
    let protocol = Protocol(0);
    let deserializer = Deserializer::new(protocol);

    let data = &[0u8; 4];

    assert_eq!(deserializer.deserialize(&data[0..data.len()]), None);
  }

  #[test]
  fn deserialize_large() {
    let protocol = Protocol(0);
    let deserializer = Deserializer::new(protocol);

    let data = [0u8; 1 << 16];

    assert_eq!(deserializer.deserialize(&data[0..data.len()]), None);
  }

  #[test]
  fn deserialize_wrong_protocol() {
    let protocol = Protocol(0);
    let deserializer = Deserializer::new(protocol);

    #[rustfmt::skip]
    let data = [
      /* protocol */  1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
      /* sequence */  0u8, 0u8, 0u8, 0u8,
      /* ack */       0u8, 0u8, 0u8, 0u8,
      /* ack_bits */  0u8, 0u8, 0u8, 0u8,
      /* payload (empty) */
    ];

    assert_eq!(deserializer.deserialize(&data[0..data.len()]), None);
  }
}
