//! Compact versioned UDP transport for synchronized light-stick fleets.
//!
//! The packet carries a future render timestamp and audio sample index. A
//! controller should split large layouts into chunks and send all chunks with
//! the same sequence number and timestamp.

use core::fmt;

use crate::types::Rgba;

/// Packet magic.
pub const MAGIC: [u8; 4] = *b"GLB1";
/// Current wire version.
pub const VERSION: u8 = 1;
/// Fixed header bytes.
pub const HEADER_BYTES: usize = 40;
/// Bytes per light entry (`id` plus four `u16` channels).
pub const LIGHT_BYTES: usize = 12;
/// Trailing CRC-32 bytes.
pub const CRC_BYTES: usize = 4;
/// Conservative payload size below common network MTUs.
pub const MAX_UDP_PAYLOAD: usize = 1_200;
/// Maximum light entries per conservative UDP packet.
pub const MAX_LIGHTS_PER_PACKET: usize = (MAX_UDP_PAYLOAD - HEADER_BYTES - CRC_BYTES) / LIGHT_BYTES;

/// Header flags.
pub mod flags {
    /// Current frame is on a beat.
    pub const BEAT: u8 = 1 << 0;
    /// Current frame is on a downbeat.
    pub const DOWNBEAT: u8 = 1 << 1;
    /// A confirmed release/drop is active.
    pub const DROP: u8 = 1 << 2;
    /// The fourth channel is physical amber instead of alpha.
    pub const AMBER: u8 = 1 << 3;
}

/// One transport light entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WireLight {
    /// Stable light identifier.
    pub id: u32,
    /// Normalized output channels.
    pub rgba: Rgba,
}

/// Metadata shared by all chunks of one rendered frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameMetadata {
    /// Monotonic sequence number.
    pub sequence: u32,
    /// Audio sample position represented by this frame.
    pub sample_index: u64,
    /// Absolute synchronized render time in microseconds.
    pub render_time_us: u64,
    /// Estimated tempo.
    pub bpm: f32,
    /// Beat phase in `[0, 1)`.
    pub beat_phase: f32,
    /// Bitwise combination of [`flags`].
    pub flags: u8,
}

/// Decoded owned packet.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedPacket {
    /// Shared frame metadata.
    pub metadata: FrameMetadata,
    /// Zero-based chunk index.
    pub chunk_index: u16,
    /// Total chunks for the sequence.
    pub chunk_count: u16,
    /// Lights in this chunk.
    pub lights: Vec<WireLight>,
}

/// Protocol error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    /// Packet is too short or internally truncated.
    Truncated,
    /// Magic or version is unsupported.
    Unsupported,
    /// Header fields are invalid.
    Invalid,
    /// CRC verification failed.
    CrcMismatch,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "truncated GlowBeat packet",
            Self::Unsupported => "unsupported GlowBeat packet",
            Self::Invalid => "invalid GlowBeat packet",
            Self::CrcMismatch => "GlowBeat packet CRC mismatch",
        })
    }
}

impl std::error::Error for ProtocolError {}

/// Encodes a single chunk into `output`, clearing its previous contents.
///
/// `lights.len()` must not exceed [`MAX_LIGHTS_PER_PACKET`].
pub fn encode_chunk(
    metadata: FrameMetadata,
    chunk_index: u16,
    chunk_count: u16,
    lights: &[WireLight],
    output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
    if chunk_count == 0
        || chunk_index >= chunk_count
        || lights.len() > MAX_LIGHTS_PER_PACKET
        || !metadata.bpm.is_finite()
        || !metadata.beat_phase.is_finite()
        || lights.iter().any(|light| !light.rgba.is_finite())
    {
        return Err(ProtocolError::Invalid);
    }
    output.clear();
    output.reserve(HEADER_BYTES + lights.len() * LIGHT_BYTES + CRC_BYTES);
    output.extend_from_slice(&MAGIC);
    output.push(VERSION);
    output.push(metadata.flags);
    push_u16(output, HEADER_BYTES as u16);
    push_u32(output, metadata.sequence);
    push_u64(output, metadata.sample_index);
    push_u64(output, metadata.render_time_us);
    push_u16(
        output,
        (metadata.bpm.clamp(0.0, 655.35) * 100.0).round() as u16,
    );
    push_u16(
        output,
        (metadata.beat_phase.rem_euclid(1.0) * 65_535.0).round() as u16,
    );
    push_u16(output, chunk_index);
    push_u16(output, chunk_count);
    push_u16(output, lights.len() as u16);
    push_u16(output, 0);
    debug_assert_eq!(output.len(), HEADER_BYTES);
    for light in lights {
        push_u32(output, light.id);
        push_u16(output, quantize(light.rgba.r));
        push_u16(output, quantize(light.rgba.g));
        push_u16(output, quantize(light.rgba.b));
        push_u16(output, quantize(light.rgba.a));
    }
    let crc = crc32(output);
    push_u32(output, crc);
    Ok(())
}

/// Decodes and CRC-checks one packet.
pub fn decode_packet(packet: &[u8]) -> Result<DecodedPacket, ProtocolError> {
    if packet.len() < HEADER_BYTES + CRC_BYTES {
        return Err(ProtocolError::Truncated);
    }
    if packet.get(0..4) != Some(MAGIC.as_slice()) || packet[4] != VERSION {
        return Err(ProtocolError::Unsupported);
    }
    let header_bytes = read_u16(packet, 6)? as usize;
    if header_bytes != HEADER_BYTES {
        return Err(ProtocolError::Unsupported);
    }
    let expected_crc = read_u32(packet, packet.len() - CRC_BYTES)?;
    if crc32(&packet[..packet.len() - CRC_BYTES]) != expected_crc {
        return Err(ProtocolError::CrcMismatch);
    }
    let light_count = read_u16(packet, 36)? as usize;
    let expected_length = HEADER_BYTES
        .checked_add(
            light_count
                .checked_mul(LIGHT_BYTES)
                .ok_or(ProtocolError::Invalid)?,
        )
        .and_then(|value| value.checked_add(CRC_BYTES))
        .ok_or(ProtocolError::Invalid)?;
    if packet.len() != expected_length || light_count > MAX_LIGHTS_PER_PACKET {
        return Err(ProtocolError::Invalid);
    }

    let metadata = FrameMetadata {
        sequence: read_u32(packet, 8)?,
        sample_index: read_u64(packet, 12)?,
        render_time_us: read_u64(packet, 20)?,
        bpm: read_u16(packet, 28)? as f32 / 100.0,
        beat_phase: read_u16(packet, 30)? as f32 / 65_535.0,
        flags: packet[5],
    };
    let chunk_index = read_u16(packet, 32)?;
    let chunk_count = read_u16(packet, 34)?;
    if chunk_count == 0 || chunk_index >= chunk_count {
        return Err(ProtocolError::Invalid);
    }
    let mut lights = Vec::with_capacity(light_count);
    let mut cursor = HEADER_BYTES;
    for _ in 0..light_count {
        lights.push(WireLight {
            id: read_u32(packet, cursor)?,
            rgba: Rgba::new(
                read_u16(packet, cursor + 4)? as f32 / 65_535.0,
                read_u16(packet, cursor + 6)? as f32 / 65_535.0,
                read_u16(packet, cursor + 8)? as f32 / 65_535.0,
                read_u16(packet, cursor + 10)? as f32 / 65_535.0,
            ),
        });
        cursor += LIGHT_BYTES;
    }
    Ok(DecodedPacket {
        metadata,
        chunk_index,
        chunk_count,
        lights,
    })
}

/// Computes the number of required chunks.
#[must_use]
pub fn chunk_count(light_count: usize) -> u16 {
    light_count
        .div_ceil(MAX_LIGHTS_PER_PACKET)
        .max(1)
        .min(u16::MAX as usize) as u16
}

fn quantize(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * 65_535.0).round() as u16
}

fn push_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}
fn push_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}
fn push_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}
fn read_u16(input: &[u8], offset: usize) -> Result<u16, ProtocolError> {
    let bytes = input
        .get(offset..offset + 2)
        .ok_or(ProtocolError::Truncated)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}
fn read_u32(input: &[u8], offset: usize) -> Result<u32, ProtocolError> {
    let bytes = input
        .get(offset..offset + 4)
        .ok_or(ProtocolError::Truncated)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
fn read_u64(input: &[u8], offset: usize) -> Result<u64, ProtocolError> {
    let bytes = input
        .get(offset..offset + 8)
        .ok_or(ProtocolError::Truncated)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_round_trip() {
        let metadata = FrameMetadata {
            sequence: 42,
            sample_index: 123_456,
            render_time_us: 9_876_543,
            bpm: 127.5,
            beat_phase: 0.25,
            flags: flags::BEAT | flags::AMBER,
        };
        let lights = vec![
            WireLight {
                id: 7,
                rgba: Rgba::new(0.1, 0.2, 0.3, 0.4),
            },
            WireLight {
                id: 8,
                rgba: Rgba::new(1.0, 0.0, 0.7, 1.0),
            },
        ];
        let mut bytes = Vec::new();
        encode_chunk(metadata, 0, 1, &lights, &mut bytes).unwrap();
        let decoded = decode_packet(&bytes).unwrap();
        assert_eq!(decoded.metadata.sequence, metadata.sequence);
        assert_eq!(decoded.lights.len(), lights.len());
        assert!((decoded.lights[0].rgba.g - 0.2).abs() < 2.0 / 65_535.0);
    }

    #[test]
    fn crc_detects_damage() {
        let mut bytes = Vec::new();
        encode_chunk(FrameMetadata::default(), 0, 1, &[], &mut bytes).unwrap();
        bytes[10] ^= 0x80;
        assert_eq!(decode_packet(&bytes), Err(ProtocolError::CrcMismatch));
    }
}
