use crate::{seq, Protocol};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize)]
pub struct PacketData<'a> {
  protocol: Protocol,
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

pub struct Unsent {
  pub payload: Vec<u8>,
  pub is_reliable: bool,
}
pub struct Pending {
  pub payload: Vec<u8>,
  pub sequence: u32,
  pub sent_at: Duration,
}
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
        if let Some(entry) = self.get_mut(sequence).filter(|e| !e.acked) {
          // and if it is not already acked, ack it, and notify the caller
          entry.acked = true;
        }
      }
    }
  }
}

impl<'a> PacketData<'a> {
  pub fn build(
    protocol: Protocol,
    local_sequence: u32,
    remote_sequence: u32,
    recv_buffer: &Buffer,
    payload: &'a [u8],
  ) -> Self {
    Self {
      protocol,
      sequence: local_sequence,
      ack: remote_sequence,
      ack_bits: recv_buffer.get_ack_bits(remote_sequence),
      payload,
    }
  }

  pub fn build_in(
    buffer: &mut [u8],
    protocol: Protocol,
    local_sequence: u32,
    remote_sequence: u32,
    recv_buffer: &Buffer,
    payload: &'a [u8],
  ) -> usize {
    let packet = Self::build(
      protocol,
      local_sequence,
      remote_sequence,
      recv_buffer,
      payload,
    );
    packet.serialize_to(buffer)
  }

  pub fn serialize_to(&self, buffer: &mut [u8]) -> usize {
    let mut writer = std::io::Cursor::new(buffer);
    bincode::serialize_into(&mut writer, self).expect("Failed to serialize packet");
    writer.position() as usize
  }

  pub fn deserialize_from(buffer: &'a [u8]) -> Option<PacketData<'a>> {
    bincode::deserialize(buffer).ok()
  }

  #[inline]
  pub fn protocol(&self) -> Protocol {
    self.protocol
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
}
