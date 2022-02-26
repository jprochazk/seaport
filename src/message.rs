use {
  crate::detail::ceil_div,
  std::{ops::Range, ptr},
};

// We need to be able to ack 256 segments at 1 bit per segment
const ACK_FIELD_LEN: usize = 256 / 8;
const SEGMENT_SIZE: usize = 256;

/// A message is a wrapper over a payload which keeps track of the acknowledgement status of disjoint segments.
#[repr(align(16))]
pub struct Message {
  /// Stores the ack field and the payload
  ///
  /// This is not a `Vec`, because it is never reallocated.
  /// It is also not a `Box`, because we do not need to store the length as `usize`,
  /// we can already obtain it by calculating `32 + num_segments * 256 + last_segment_len`,
  /// and those can be `u8` fields due to the limits on the maximum segment and message sizes.
  ///
  /// The layout is as follows:
  /// ```no_run
  /// [ ack_field : 32 , payload : (num_segments * 256 + last_segment_len) ]
  /// ```
  data: ptr::NonNull<u8>,
  /// The number of full segments. A full segment is 256 bytes.
  num_full_segments: u8,
  /// The length of the last segment in bytes.
  ///
  /// This may be 0 if the payload length is a multiple of 256.
  last_segment_len: u8,
  /// Whether or not this message is guaranteed to be delivered.
  is_reliable: bool,
}
static_assertions::assert_eq_size!(Message, [u8; 16]);

impl Message {
  /// Constructs a new outgoing message, wrapping `payload` with ack tracking.
  ///
  /// The payload must not be empty and it must not be larger than 64 KiB (u16::MAX + 1).
  ///
  /// ### Panics
  ///
  /// If `payload.len() > u16::MAX`
  pub fn outgoing(is_reliable: bool, payload: &[u8]) -> Self {
    if payload.len() > u16::MAX as usize {
      panic!("payload is too large");
    }

    let num_segments = ceil_div(payload.len(), SEGMENT_SIZE);
    let num_full_segments = payload.len() / SEGMENT_SIZE;
    let last_segment_len = payload.len() % SEGMENT_SIZE;

    // allocate data, copy payload into it
    let mut data = vec![0u8; ACK_FIELD_LEN + payload.len()];
    data[ACK_FIELD_LEN..ACK_FIELD_LEN + payload.len()].copy_from_slice(payload);

    // initialize all acks after `num_segments` to 1
    // this ensures that `get_unacked_segment` will not attempt to return segments which are out of bounds
    for offset in num_segments..ACK_FIELD_LEN * 8 {
      let byte = offset / 8;
      let bit = offset % 8;
      data[byte] |= 1 << bit;
    }

    // discard the capacity and length fields
    let data = ptr::NonNull::new(Box::into_raw(data.into_boxed_slice()) as *mut u8).unwrap();

    Self {
      data,
      num_full_segments: num_full_segments as u8,
      last_segment_len: last_segment_len as u8,
      is_reliable,
    }
  }

  pub fn incoming(num_full_segments: u8, last_segment_len: u8) -> Self {
    let len = num_full_segments as usize * SEGMENT_SIZE + last_segment_len as usize;
    let num_segments = ceil_div(len, SEGMENT_SIZE);

    // allocate data, copy payload into it
    let mut data = vec![0u8; ACK_FIELD_LEN + len];

    // initialize all acks after `num_segments` to 1
    for offset in num_segments..ACK_FIELD_LEN * 8 {
      let byte = offset / 8;
      let bit = offset % 8;
      data[byte] |= 1 << bit;
    }

    // discard the capacity and length fields
    let data = ptr::NonNull::new(Box::into_raw(data.into_boxed_slice()) as *mut u8).unwrap();

    Self { data, num_full_segments, last_segment_len, is_reliable: true }
  }

  /// Retrieve segment at `offset`.
  pub fn get(&self, offset: u8) -> &[u8] {
    let pos = offset as usize * SEGMENT_SIZE;
    &self.payload()[pos..pos + SEGMENT_SIZE]
  }

  /// Place data into the message.
  ///
  /// This also acknowledges the segment.
  pub fn put(&mut self, offset: u8, data: &[u8]) {
    let offset = offset as usize;
    let (acks, payload) = self.split_mut();
    let pos = offset * SEGMENT_SIZE;
    payload[pos..pos + SEGMENT_SIZE].copy_from_slice(data);
    let byte = offset / 8;
    let bit = offset % 8;
    acks[byte] |= 1 << bit;
  }

  /// Set segment ack status in range `start..start+length` to acknowledged.
  pub fn ack(&mut self, range: Range<u8>) {
    let acks = self.acks_mut();
    for offset in range {
      let byte = offset / 8;
      let bit = offset % 8;
      acks[byte as usize] |= 1 << bit;
    }
  }

  /// Returns the first unacked segment after `start`, and its offset.
  pub fn get_unacked_segment(&self, start: u8) -> Option<(u8, &[u8])> {
    let (acks, payload) = self.split();
    for offset in start..=(ACK_FIELD_LEN * 8 - 1) as u8 {
      let byte = offset / 8;
      let bit = offset % 8;
      if acks[byte as usize] & (1 << bit) != (1 << bit) {
        let pos = offset as usize * SEGMENT_SIZE;
        let len = self.segment_len(offset);
        return Some((offset as u8, &payload[pos..pos + len]));
      }
    }
    None
  }

  /// Length of the payload
  #[inline]
  pub fn len(&self) -> usize {
    self.num_full_segments as usize * SEGMENT_SIZE + self.last_segment_len as usize
  }

  #[inline]
  pub fn is_reliable(&self) -> bool {
    self.is_reliable
  }

  /// Return length of segment at `offset`.
  ///
  /// Returns `SEGMENT_SIZE` for any offset in range `0..=self.num_full_segments`,
  /// and an arbitrary number in the range `0..SEGMENT_SIZE` otherwise.
  #[inline]
  fn segment_len(&self, offset: u8) -> usize {
    if offset >= self.num_full_segments {
      self.last_segment_len as usize
    } else {
      SEGMENT_SIZE
    }
  }

  /// Length of `self.data`
  #[inline]
  fn data_len(&self) -> usize {
    ACK_FIELD_LEN + self.num_full_segments as usize * SEGMENT_SIZE + self.last_segment_len as usize
  }

  #[inline]
  fn acks_mut(&mut self) -> &mut [u8] {
    &mut self.data_mut()[0..ACK_FIELD_LEN]
  }

  #[inline]
  fn acks(&self) -> &[u8] {
    &self.data()[0..ACK_FIELD_LEN]
  }

  #[inline]
  fn payload(&self) -> &[u8] {
    &self.data()[ACK_FIELD_LEN..]
  }

  #[inline]
  fn payload_mut(&mut self) -> &mut [u8] {
    &mut self.data_mut()[ACK_FIELD_LEN..]
  }

  #[inline]
  fn split(&self) -> (&[u8], &[u8]) {
    self.data().split_at(ACK_FIELD_LEN)
  }

  #[inline]
  fn split_mut(&mut self) -> (&mut [u8], &mut [u8]) {
    self.data_mut().split_at_mut(ACK_FIELD_LEN)
  }

  #[inline]
  fn data_mut(&mut self) -> &mut [u8] {
    // SAFETY: requirements are fulfilled by allocating data through `Vec`
    // + data is always accessed through either `data` or `data_mut`, which means borrow checking rules are enforced
    unsafe { std::slice::from_raw_parts_mut(self.data.as_ptr(), self.data_len()) }
  }

  #[inline]
  fn data(&self) -> &[u8] {
    // SAFETY: requirements are fulfilled by allocating data through `Vec`
    // + data is always accessed through either `data` or `data_mut`, which means borrow checking rules are enforced
    unsafe { std::slice::from_raw_parts(self.data.as_ptr(), self.data_len()) }
  }
}

impl Drop for Message {
  fn drop(&mut self) {
    // SAFETY: requirements are fulfilled in `Message::new`
    let data = unsafe {
      Box::from_raw(std::ptr::slice_from_raw_parts_mut(self.data.as_ptr(), self.data_len()))
    };
    drop(data);
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  //use pretty_assertions::assert_eq;

  #[test]
  fn message_new_incoming() {
    let msg = Message::incoming(0, 255);
    assert_eq!(msg.len(), 255);
    assert_eq!(msg.data_len(), ACK_FIELD_LEN + 255);
    assert_eq!(msg.num_full_segments, 0);
    assert_eq!(msg.last_segment_len, 255);

    let msg = Message::incoming(1, 0);
    assert_eq!(msg.len(), 256);
    assert_eq!(msg.data_len(), ACK_FIELD_LEN + 256);
    assert_eq!(msg.num_full_segments, 1);
    assert_eq!(msg.last_segment_len, 0);

    let msg = Message::incoming(255, 255);
    assert_eq!(msg.len(), (1 << 16) - 1);
    assert_eq!(msg.data_len(), ACK_FIELD_LEN + (1 << 16) - 1);
    assert_eq!(msg.num_full_segments, 255);
    assert_eq!(msg.last_segment_len, 255);
  }

  #[test]
  fn message_new_outgoing() {
    let payload = &[0u8; 256][..];
    let msg = Message::outgoing(true, payload);
    assert_eq!(msg.len(), payload.len());
    assert_eq!(msg.data_len(), ACK_FIELD_LEN + payload.len());
    assert_eq!(msg.num_full_segments, 1);
    assert_eq!(msg.last_segment_len, 0);

    let payload = &[0u8; 257][..];
    let msg = Message::outgoing(true, payload);
    assert_eq!(msg.len(), payload.len());
    assert_eq!(msg.data_len(), ACK_FIELD_LEN + payload.len());
    assert_eq!(msg.num_full_segments, 1);
    assert_eq!(msg.last_segment_len, 1);

    let payload = &[0u8; 255][..];
    let msg = Message::outgoing(true, payload);
    assert_eq!(msg.len(), payload.len());
    assert_eq!(msg.data_len(), ACK_FIELD_LEN + payload.len());
    assert_eq!(msg.num_full_segments, 0);
    assert_eq!(msg.last_segment_len, 255);

    let payload = &[0u8; 0];
    let msg = Message::outgoing(true, payload);
    assert_eq!(msg.len(), payload.len());
    assert_eq!(msg.data_len(), ACK_FIELD_LEN + payload.len());
    assert_eq!(msg.num_full_segments, 0);
    assert_eq!(msg.last_segment_len, 0);
  }

  #[test]
  #[should_panic]
  fn message_too_large() {
    let payload = &[0u8; 1 << 16];
    Message::outgoing(true, payload);
  }

  #[test]
  fn message_largest_possible() {
    let payload = [255u8; (1 << 16) - 1];
    let msg = Message::outgoing(true, &payload[..]);
    assert_eq!(msg.len(), payload.len());
    assert_eq!(msg.data_len(), ACK_FIELD_LEN + payload.len());
    assert_eq!(msg.num_full_segments, 255);
    assert_eq!(msg.last_segment_len, 255);

    // we should be able to transparently reconstruct the original payload
    let mut buffer = [0u8; (1 << 16) - 1];
    for i in 0..=255 {
      let segment = msg.get_unacked_segment(i);
      assert!(segment.is_some(), "failed to get segment {i}");
      let (offset, data) = segment.unwrap();
      let pos = offset as usize * SEGMENT_SIZE;
      buffer[pos..pos + data.len()].copy_from_slice(data);
    }

    assert_eq!(payload, buffer);
  }

  #[test]
  fn message_unacked() {
    // 4 segments
    let payload = [0u8; 1024];
    let msg = Message::outgoing(true, &payload[..]);

    assert_eq!(msg.get_unacked_segment(0), Some((0u8, &payload[0..256])));
    assert_eq!(msg.get_unacked_segment(1), Some((1u8, &payload[256..512])));
    assert_eq!(msg.get_unacked_segment(2), Some((2u8, &payload[512..768])));
    assert_eq!(msg.get_unacked_segment(3), Some((3u8, &payload[768..1024])));
  }

  #[test]
  fn message_acked_start() {
    // 4 segments
    let payload = [0u8; 1024];
    let mut msg = Message::outgoing(true, &payload[..]);

    msg.ack(0..1);

    assert_eq!(msg.get_unacked_segment(0), Some((1u8, &payload[256..512])));
    assert_eq!(msg.get_unacked_segment(1), Some((1u8, &payload[256..512])));
    assert_eq!(msg.get_unacked_segment(2), Some((2u8, &payload[512..768])));
    assert_eq!(msg.get_unacked_segment(3), Some((3u8, &payload[768..1024])));
  }

  #[test]
  fn message_acked_mid() {
    // 4 segments
    let payload = [0u8; 1024];
    let mut msg = Message::outgoing(true, &payload[..]);

    msg.ack(1..3);

    assert_eq!(msg.get_unacked_segment(0), Some((0u8, &payload[0..256])));
    assert_eq!(msg.get_unacked_segment(1), Some((3u8, &payload[768..1024])));
    assert_eq!(msg.get_unacked_segment(2), Some((3u8, &payload[768..1024])));
    assert_eq!(msg.get_unacked_segment(3), Some((3u8, &payload[768..1024])));
  }

  #[test]
  fn message_acked_end() {
    // 4 segments
    let payload = [0u8; 1024];
    let mut msg = Message::outgoing(true, &payload[..]);

    msg.ack(3..4);

    assert_eq!(msg.get_unacked_segment(0), Some((0u8, &payload[0..256])));
    assert_eq!(msg.get_unacked_segment(1), Some((1u8, &payload[256..512])));
    assert_eq!(msg.get_unacked_segment(2), Some((2u8, &payload[512..768])));
    assert_eq!(msg.get_unacked_segment(3), None);
  }
}
