use crate::{
  codec::{self, Decode, Encode},
  message::SEGMENT_SIZE,
  varint::VarInt,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Header {
  pub protocol: u8,
  pub packet_id: u32,
}

impl Encode for Header {
  fn encode<B: bytes::BufMut>(&self, buf: &mut B) {
    self.protocol.encode(buf);
    self.packet_id.encode(buf);
  }
}

impl Decode for Header {
  fn decode<B: bytes::Buf>(buf: &mut B) -> codec::Result<Self> {
    let protocol = u8::decode(buf)?;
    let packet_id = u32::decode(buf)?;
    Ok(Self { protocol, packet_id })
  }
}

/// Represents an ack range.
///
/// Used to efficiently encode large amounts of packet acknowledgement statuses, under
/// the assumption that unacknowledged packets will be far rarer than acknowledged ones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
  /// Highest acked packet id
  pub start: u64,
  /// Number of packets before `start` to ack
  pub len: u64,
}

impl Encode for Range {
  fn encode<B: bytes::BufMut>(&self, buf: &mut B) {
    VarInt::u64(self.start).unwrap().encode(buf);
    VarInt::u64(self.len).unwrap().encode(buf);
  }
}

impl Decode for Range {
  fn decode<B: bytes::Buf>(buf: &mut B) -> codec::Result<Self> {
    let start = VarInt::decode(buf)?.into_inner();
    let len = VarInt::decode(buf)?.into_inner();
    Ok(Self { start, len })
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ack {
  // TODO: use a simple bump allocator for this
  pub ranges: Vec<Range>,
}

impl Encode for Ack {
  fn encode<B: bytes::BufMut>(&self, buf: &mut B) {
    VarInt::u64(self.ranges.len() as u64).unwrap().encode(buf);
    for range in &self.ranges {
      range.encode(buf);
    }
  }
}

impl Decode for Ack {
  fn decode<B: bytes::Buf>(buf: &mut B) -> codec::Result<Self> {
    let len = VarInt::decode(buf)?.into_inner() as usize;
    if len > 64 {
      return Err(codec::Error::TooLarge("ack range list"));
    }
    let mut ranges = Vec::with_capacity(len);
    for _ in 0..len {
      ranges.push(Range::decode(buf)?);
    }
    Ok(Self { ranges })
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
  /// Id of the message this segment belongs to
  ///
  /// Maximum value is `(1 << 12) - 1`
  pub message_id: u16,
  /// Id of the channel this message belongs to
  ///
  /// Maximum value is `(1 << 4) - 1`
  pub channel_id: u16,
  /// The number of full segments
  pub num_full_segments: u8,
  /// The length of the last segment
  pub last_segment_len: u8,
  /// Offset of this segment
  pub segment_offset: u8,
  /// Length of this segment
  pub segment_len: u8,
  /// Opaque payload of the segment
  pub data: Vec<u8>,
}

impl Encode for Segment {
  fn encode<B: bytes::BufMut>(&self, buf: &mut B) {
    let header = (self.channel_id << 12) | self.message_id;
    header.encode(buf);
    self.num_full_segments.encode(buf);
    self.last_segment_len.encode(buf);
    self.segment_offset.encode(buf);
    self.segment_len.encode(buf);
    buf.put(&self.data[..]);
  }
}

impl Decode for Segment {
  fn decode<B: bytes::Buf>(buf: &mut B) -> codec::Result<Self> {
    let header = u16::decode(buf)?;
    let channel_id = header >> 12;
    let message_id = header & 0b0000_1111_1111_1111;
    let num_full_segments = u8::decode(buf)?;
    let last_segment_len = u8::decode(buf)?;
    let segment_offset = u8::decode(buf)?;
    let segment_len = u8::decode(buf)?;
    let len = if segment_offset == num_full_segments { segment_len as usize } else { SEGMENT_SIZE };
    if buf.remaining() < len {
      return Err(codec::Error::UnexpectedEof);
    }
    let mut data = vec![0u8; len];
    buf.copy_to_slice(&mut data[..]);

    Ok(Self {
      message_id,
      channel_id,
      num_full_segments,
      last_segment_len,
      segment_offset,
      segment_len,
      data,
    })
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
  header: Header,
  acks: Ack,
  segments: Vec<Segment>,
}

impl Encode for Packet {
  fn encode<B: bytes::BufMut>(&self, buf: &mut B) {
    self.header.encode(buf);
    self.acks.encode(buf);
    for segment in &self.segments {
      segment.encode(buf);
    }
  }
}

impl Decode for Packet {
  fn decode<B: bytes::Buf>(buf: &mut B) -> codec::Result<Self> {
    let header = Header::decode(buf)?;
    let acks = Ack::decode(buf)?;
    let mut segments = Vec::new();
    while buf.has_remaining() {
      segments.push(Segment::decode(buf)?);
    }
    Ok(Self { header, acks, segments })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn encode_and_decode_range() {
    let range = Range { start: 0, len: 0 };
    let mut buf = bytes::BytesMut::new();
    range.encode(&mut buf);
    let mut buf = buf.freeze();
    assert_eq!(buf.len(), 2);
    assert_eq!(Range::decode(&mut buf).unwrap(), range);
  }

  #[test]
  fn encode_and_decode_acks() {
    let range = Range { start: 0, len: 0 };
    let ack = Ack { ranges: vec![range, range] };
    let mut buf = bytes::BytesMut::new();
    ack.encode(&mut buf);
    let mut buf = buf.freeze();
    assert_eq!(buf.len(), 5);
    assert_eq!(Ack::decode(&mut buf).unwrap(), ack);
  }

  #[test]
  fn encode_and_decode_segment() {
    let mut segment = Segment {
      message_id: 1,
      channel_id: 1,
      num_full_segments: 1,
      last_segment_len: 1,
      segment_offset: 0,
      segment_len: 0,
      data: vec![0u8; 256],
    };

    let mut buf = bytes::BytesMut::new();
    segment.encode(&mut buf);
    let mut buf = buf.freeze();
    assert_eq!(buf.len(), /* segment header */ 6 + /* segment data */ 256);
    assert_eq!(Segment::decode(&mut buf).unwrap(), segment);

    segment.segment_offset += 1;
    segment.segment_len = 255;
    segment.data = vec![0u8; 255];

    let mut buf = bytes::BytesMut::new();
    segment.encode(&mut buf);
    let mut buf = buf.freeze();
    assert_eq!(buf.len(), /* segment header */ 6 + /* segment data */ 255);
    assert_eq!(Segment::decode(&mut buf).unwrap(), segment);
  }

  #[rustfmt::skip]
  #[allow(clippy::identity_op)]
  #[test]
  fn encode_and_decode_packet() {
    let packet = Packet {
      header: Header { protocol: 0, packet_id: 0 },
      acks: Ack { ranges: vec![] },
      segments: vec![Segment {
        message_id: 0,
        channel_id: 0,
        num_full_segments: 0,
        last_segment_len: 0,
        segment_offset: 0,
        segment_len: 0,
        data: vec![],
      }],
    };

    let mut buf = bytes::BytesMut::new();
    packet.encode(&mut buf);
    let mut buf = buf.freeze();
    assert_eq!(
      buf.len(),
      {
          1 // protocol
        + 4 // packet_id
        + 1 // acks.ranges.len()
        + 0 // acks.ranges[..]
        + 1 // segments[0].message_id
        + 1 // segments[0].channel_id
        + 1 // segments[0].num_full_segments
        + 1 // segments[0].last_segment_len
        + 1 // segments[0].segment_offset
        + 1 // segments[0].segment_len
        + 0 // segments[0].data[..]
      }
    );
    assert_eq!(Packet::decode(&mut buf).unwrap(), packet);
  }
}
