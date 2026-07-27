//! Small dependency-free RIFF/WAVE PCM decoder used by the CLI and tests.
//!
//! Production applications may decode any codec with their existing player and
//! feed the resulting interleaved `f32` PCM directly into GlowBeat.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

/// Decoded interleaved WAVE audio.
#[derive(Clone, Debug, PartialEq)]
pub struct WavData {
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Number of interleaved channels.
    pub channels: u16,
    /// Interleaved normalized `f32` samples.
    pub samples: Vec<f32>,
}

impl WavData {
    /// Number of complete sample frames.
    #[must_use]
    pub fn sample_frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    /// Duration in seconds.
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        self.sample_frames() as f64 / self.sample_rate.max(1) as f64
    }
}

/// Reads PCM or IEEE-float RIFF/WAVE audio.
pub fn read_wav<P: AsRef<Path>>(path: P) -> Result<WavData, WavError> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    decode_wav(&bytes)
}

/// Decodes PCM or IEEE-float RIFF/WAVE bytes.
pub fn decode_wav(bytes: &[u8]) -> Result<WavData, WavError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::InvalidFormat("expected RIFF/WAVE header"));
    }
    let mut offset = 12usize;
    let mut format: Option<WaveFormat> = None;
    let mut data_range: Option<(usize, usize)> = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = read_u32_at(bytes, offset + 4)? as usize;
        let data_start = offset + 8;
        let data_end = data_start
            .checked_add(size)
            .ok_or(WavError::InvalidFormat("chunk size overflow"))?;
        if data_end > bytes.len() {
            return Err(WavError::InvalidFormat("truncated WAVE chunk"));
        }
        match id {
            b"fmt " => format = Some(parse_format(&bytes[data_start..data_end])?),
            b"data" if data_range.is_none() => data_range = Some((data_start, data_end)),
            _ => {}
        }
        offset = data_end + (size & 1);
    }
    let format = format.ok_or(WavError::InvalidFormat("missing fmt chunk"))?;
    let (data_start, data_end) = data_range.ok_or(WavError::InvalidFormat("missing data chunk"))?;
    if format.channels == 0 || format.sample_rate == 0 || format.block_align == 0 {
        return Err(WavError::InvalidFormat("invalid WAVE format metadata"));
    }
    let data = &bytes[data_start..data_end];
    let complete_bytes = data.len() / format.block_align as usize * format.block_align as usize;
    let bytes_per_sample = format.bits_per_sample.div_ceil(8) as usize;
    if bytes_per_sample == 0
        || (format.block_align as usize) < bytes_per_sample * format.channels as usize
    {
        return Err(WavError::InvalidFormat("invalid block alignment"));
    }
    let sample_frames = complete_bytes / format.block_align as usize;
    let sample_count = sample_frames
        .checked_mul(format.channels as usize)
        .ok_or(WavError::InvalidFormat("sample count overflow"))?;
    let mut samples = Vec::with_capacity(sample_count);
    for frame in 0..sample_frames {
        let frame_start = frame * format.block_align as usize;
        for channel in 0..format.channels as usize {
            let sample_start = frame_start + channel * bytes_per_sample;
            let sample_end = sample_start + bytes_per_sample;
            let value = decode_sample(
                &data[sample_start..sample_end],
                format.encoding,
                format.bits_per_sample,
                format.valid_bits,
            )?;
            samples.push(value.clamp(-1.0, 1.0));
        }
    }
    Ok(WavData {
        sample_rate: format.sample_rate,
        channels: format.channels,
        samples,
    })
}

/// Writes interleaved `f32` PCM as a standards-compatible 32-bit float WAVE.
pub fn write_wav_f32<P: AsRef<Path>>(
    path: P,
    sample_rate: u32,
    channels: u16,
    samples: &[f32],
) -> Result<(), WavError> {
    if sample_rate == 0 || channels == 0 {
        return Err(WavError::InvalidFormat("invalid output metadata"));
    }
    let complete = samples.len() / channels as usize * channels as usize;
    let data_bytes = complete
        .checked_mul(4)
        .ok_or(WavError::InvalidFormat("output too large"))?;
    let riff_size = 4usize
        .checked_add(8 + 16)
        .and_then(|value| value.checked_add(8 + data_bytes))
        .ok_or(WavError::InvalidFormat("output too large"))?;
    let riff_size = u32::try_from(riff_size)
        .map_err(|_| WavError::InvalidFormat("RIFF output exceeds 4 GiB"))?;
    let data_size = u32::try_from(data_bytes)
        .map_err(|_| WavError::InvalidFormat("WAVE data exceeds 4 GiB"))?;
    let byte_rate = sample_rate
        .checked_mul(channels as u32)
        .and_then(|value| value.checked_mul(4))
        .ok_or(WavError::InvalidFormat("byte rate overflow"))?;
    let block_align = channels
        .checked_mul(4)
        .ok_or(WavError::InvalidFormat("block alignment overflow"))?;

    let mut file = File::create(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&3_u16.to_le_bytes())?; // IEEE float
    file.write_all(&channels.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&32_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_size.to_le_bytes())?;
    for &sample in &samples[..complete] {
        file.write_all(&sample.clamp(-1.0, 1.0).to_le_bytes())?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Encoding {
    Pcm,
    Float,
}

#[derive(Clone, Copy, Debug)]
struct WaveFormat {
    encoding: Encoding,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bits_per_sample: u16,
    valid_bits: u16,
}

fn parse_format(bytes: &[u8]) -> Result<WaveFormat, WavError> {
    if bytes.len() < 16 {
        return Err(WavError::InvalidFormat("fmt chunk is too short"));
    }
    let tag = u16::from_le_bytes([bytes[0], bytes[1]]);
    let channels = u16::from_le_bytes([bytes[2], bytes[3]]);
    let sample_rate = u32::from_le_bytes(bytes[4..8].try_into().expect("fixed slice"));
    let block_align = u16::from_le_bytes([bytes[12], bytes[13]]);
    let bits_per_sample = u16::from_le_bytes([bytes[14], bytes[15]]);
    let mut valid_bits = bits_per_sample;
    let encoding = match tag {
        1 => Encoding::Pcm,
        3 => Encoding::Float,
        0xfffe => {
            if bytes.len() < 40 {
                return Err(WavError::InvalidFormat("truncated WAVE extensible fmt"));
            }
            valid_bits = u16::from_le_bytes([bytes[18], bytes[19]]);
            let subformat_tag = u16::from_le_bytes([bytes[24], bytes[25]]);
            match subformat_tag {
                1 => Encoding::Pcm,
                3 => Encoding::Float,
                _ => {
                    return Err(WavError::UnsupportedFormat(
                        "unsupported extensible subformat",
                    ))
                }
            }
        }
        _ => {
            return Err(WavError::UnsupportedFormat(
                "only PCM and IEEE float are supported",
            ))
        }
    };
    match encoding {
        Encoding::Pcm if !matches!(bits_per_sample, 8 | 16 | 24 | 32) => {
            return Err(WavError::UnsupportedFormat("unsupported PCM bit depth"));
        }
        Encoding::Float if !matches!(bits_per_sample, 32 | 64) => {
            return Err(WavError::UnsupportedFormat("unsupported float bit depth"));
        }
        _ => {}
    }
    Ok(WaveFormat {
        encoding,
        channels,
        sample_rate,
        block_align,
        bits_per_sample,
        valid_bits: valid_bits.max(1).min(bits_per_sample),
    })
}

fn decode_sample(
    bytes: &[u8],
    encoding: Encoding,
    bits_per_sample: u16,
    valid_bits: u16,
) -> Result<f32, WavError> {
    match (encoding, bits_per_sample) {
        (Encoding::Pcm, 8) => Ok((bytes[0] as f32 - 128.0) / 128.0),
        (Encoding::Pcm, 16) => {
            let value = i16::from_le_bytes([bytes[0], bytes[1]]) as f32;
            Ok(value / 32_768.0)
        }
        (Encoding::Pcm, 24) => {
            let raw = i32::from_le_bytes([
                bytes[0],
                bytes[1],
                bytes[2],
                if bytes[2] & 0x80 != 0 { 0xff } else { 0x00 },
            ]);
            let shift = 24_u16.saturating_sub(valid_bits) as u32;
            let value = (raw >> shift) as f32;
            Ok(value / (1_u64 << (valid_bits - 1)) as f32)
        }
        (Encoding::Pcm, 32) => {
            let raw = i32::from_le_bytes(bytes.try_into().expect("validated sample width"));
            let shift = 32_u16.saturating_sub(valid_bits) as u32;
            let value = (raw >> shift) as f64;
            Ok((value / (1_u64 << (valid_bits - 1)) as f64) as f32)
        }
        (Encoding::Float, 32) => Ok(f32::from_le_bytes(
            bytes.try_into().expect("validated sample width"),
        )),
        (Encoding::Float, 64) => {
            Ok(f64::from_le_bytes(bytes.try_into().expect("validated sample width")) as f32)
        }
        _ => Err(WavError::UnsupportedFormat("unsupported sample encoding")),
    }
}

fn read_u32_at(bytes: &[u8], offset: usize) -> Result<u32, WavError> {
    let end = offset
        .checked_add(4)
        .ok_or(WavError::InvalidFormat("offset overflow"))?;
    let slice = bytes
        .get(offset..end)
        .ok_or(WavError::InvalidFormat("truncated integer"))?;
    Ok(u32::from_le_bytes(slice.try_into().expect("fixed slice")))
}

/// WAVE decoding/writing error.
#[derive(Debug)]
pub enum WavError {
    /// Underlying filesystem failure.
    Io(io::Error),
    /// Corrupted or inconsistent RIFF/WAVE structure.
    InvalidFormat(&'static str),
    /// Valid WAVE encoding not supported by the small built-in decoder.
    UnsupportedFormat(&'static str),
}

impl core::fmt::Display for WavError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::InvalidFormat(message) => write!(formatter, "invalid WAVE file: {message}"),
            Self::UnsupportedFormat(message) => {
                write!(formatter, "unsupported WAVE file: {message}")
            }
        }
    }
}

impl std::error::Error for WavError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for WavError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_wav_round_trip() {
        let samples = [0.0_f32, 0.25, -0.5, 1.0];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(4 + 8 + 16 + 8 + samples.len() as u32 * 4).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&3_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000_u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples.len() as u32 * 4).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let decoded = decode_wav(&bytes).unwrap();
        assert_eq!(decoded.sample_rate, 48_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples, samples);
    }
}
