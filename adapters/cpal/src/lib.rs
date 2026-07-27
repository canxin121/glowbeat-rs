//! Native audio capture and playback helpers built on CPAL.
//!
//! The audio callback performs no heap allocation, locking, FFT, rendering, or
//! network I/O. It only converts samples and moves them through an SPSC ring.

use std::error::Error;
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, DeviceId, FromSample, Sample, SampleFormat, SizedSample, Stream,
    StreamConfig, SupportedBufferSize, I24, U24,
};
use rtrb::{Consumer, Producer, RingBuffer};

/// Boxed adapter error type.
pub type NativeResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// A discoverable audio device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceInfo {
    /// Stable CPAL device identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Whether capture is supported.
    pub input: bool,
    /// Whether playback is supported.
    pub output: bool,
}

/// Concrete native output format selected for the demo player.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputFormat {
    /// Output sample rate in hertz.
    pub sample_rate: u32,
    /// Number of interleaved output channels.
    pub channels: usize,
    /// Native CPAL sample representation.
    pub sample_format: SampleFormat,
}

/// Lists devices from the default host.
pub fn devices() -> NativeResult<Vec<DeviceInfo>> {
    let host = cpal::default_host();
    let mut result = Vec::new();
    for device in host.devices()? {
        let description = device.description()?;
        result.push(DeviceInfo {
            id: device.id()?.to_string(),
            name: description.name().to_owned(),
            input: description.supports_input(),
            output: description.supports_output(),
        });
    }
    Ok(result)
}

/// Chooses the default device or one matching a stable ID/name substring.
pub fn select_device(selector: Option<&str>, input: bool) -> NativeResult<Device> {
    let host = cpal::default_host();
    if let Some(selector) = selector {
        if let Ok(id) = DeviceId::from_str(selector) {
            if let Some(device) = host.device_by_id(&id) {
                return Ok(device);
            }
        }
        let needle = selector.to_ascii_lowercase();
        for device in host.devices()? {
            let description = device.description()?;
            let direction_ok = if input {
                description.supports_input()
            } else {
                description.supports_output()
            };
            if direction_ok && description.name().to_ascii_lowercase().contains(&needle) {
                return Ok(device);
            }
        }
        return Err(format!("no {} device matched '{selector}'", if input { "input" } else { "output" }).into());
    }
    let device = if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    };
    device.ok_or_else(|| format!("no default {} device", if input { "input" } else { "output" }).into())
}

/// Resolves a native output format close to the requested PCM layout.
pub fn preferred_output_format(
    selector: Option<&str>,
    requested_sample_rate: u32,
    requested_channels: usize,
) -> NativeResult<OutputFormat> {
    let device = select_device(selector, false)?;
    let supported = choose_output_config(&device, requested_sample_rate, requested_channels)?;
    Ok(OutputFormat {
        sample_rate: supported.sample_rate(),
        channels: supported.channels() as usize,
        sample_format: supported.sample_format(),
    })
}

/// Capture stream whose callback writes interleaved `f32` samples to a
/// wait-free SPSC ring.
pub struct CaptureStream {
    stream: Stream,
    consumer: Consumer<f32>,
    sample_rate: u32,
    channels: usize,
    dropped_samples: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
}

impl fmt::Debug for CaptureStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CaptureStream")
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .field("dropped_samples", &self.dropped_samples())
            .field("failed", &self.failed())
            .finish_non_exhaustive()
    }
}

impl CaptureStream {
    /// Opens and starts a native input stream.
    ///
    /// `requested_buffer_frames` is clamped to the range reported by the
    /// device. `queue_milliseconds` controls only the callback-to-worker queue,
    /// not hardware latency.
    pub fn open(
        selector: Option<&str>,
        requested_buffer_frames: u32,
        queue_milliseconds: u32,
    ) -> NativeResult<Self> {
        let device = select_device(selector, true)?;
        let supported = device.default_input_config()?;
        let sample_format = supported.sample_format();
        let sample_rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        let mut config = supported.config();
        config.buffer_size = requested_buffer_size(supported.buffer_size(), requested_buffer_frames);
        let queue_samples = ((sample_rate as u64
            * channels as u64
            * queue_milliseconds.max(20) as u64)
            / 1_000)
            .clamp(4_096, 16_777_216) as usize;
        let (producer, consumer) = RingBuffer::new(queue_samples);
        let dropped_samples = Arc::new(AtomicU64::new(0));
        let failed = Arc::new(AtomicBool::new(false));
        let dropped_for_callback = dropped_samples.clone();
        let failed_for_callback = failed.clone();
        let error_callback = move |error: cpal::Error| {
            failed_for_callback.store(true, Ordering::Release);
            eprintln!("GlowBeat input stream error: {error}");
        };
        let stream = match sample_format {
            SampleFormat::I8 => build_input::<i8, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            SampleFormat::I16 => build_input::<i16, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            SampleFormat::I24 => build_input::<I24, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            SampleFormat::U24 => build_input::<U24, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            SampleFormat::I32 => build_input::<i32, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            SampleFormat::I64 => build_input::<i64, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            SampleFormat::U8 => build_input::<u8, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            SampleFormat::U16 => build_input::<u16, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            SampleFormat::U32 => build_input::<u32, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            SampleFormat::U64 => build_input::<u64, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            SampleFormat::F32 => build_input::<f32, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            SampleFormat::F64 => build_input::<f64, _>(&device, config, producer, dropped_for_callback, error_callback)?,
            other => return Err(format!("unsupported input sample format {other}").into()),
        };
        stream.play()?;
        Ok(Self {
            stream,
            consumer,
            sample_rate,
            channels,
            dropped_samples,
            failed,
        })
    }

    /// Device sample rate.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Device channel count.
    #[must_use]
    pub const fn channels(&self) -> usize {
        self.channels
    }

    /// Number of callback samples discarded because the worker fell behind.
    #[must_use]
    pub fn dropped_samples(&self) -> u64 {
        self.dropped_samples.load(Ordering::Acquire)
    }

    /// Whether CPAL reported a stream error.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    /// Drains up to `output.len()` interleaved samples without blocking.
    pub fn drain(&mut self, output: &mut [f32]) -> usize {
        let mut written = 0;
        while written < output.len() {
            match self.consumer.pop() {
                Ok(value) => {
                    output[written] = value;
                    written += 1;
                }
                Err(_) => break,
            }
        }
        written
    }

    /// Explicitly pauses capture.
    pub fn pause(&self) -> NativeResult<()> {
        self.stream.pause()?;
        Ok(())
    }
}

fn requested_buffer_size(supported: &SupportedBufferSize, requested: u32) -> BufferSize {
    match supported {
        SupportedBufferSize::Range { min, max } => BufferSize::Fixed(requested.clamp(*min, *max)),
        SupportedBufferSize::Unknown => {
            if requested == 0 {
                BufferSize::Default
            } else {
                BufferSize::Fixed(requested)
            }
        }
    }
}

fn build_input<T, E>(
    device: &Device,
    config: StreamConfig,
    mut producer: Producer<f32>,
    dropped: Arc<AtomicU64>,
    error_callback: E,
) -> NativeResult<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            for &sample in data {
                let converted = f32::from_sample(sample);
                if producer.push(converted).is_err() {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        },
        error_callback,
        None,
    )?;
    Ok(stream)
}

/// Shared sample-clock state produced by a native output callback.
#[derive(Debug, Default)]
pub struct PlaybackClock {
    frames_written: AtomicU64,
    finished: AtomicBool,
    failed: AtomicBool,
}

impl PlaybackClock {
    /// Number of PCM frames submitted to the device.
    #[must_use]
    pub fn frames_written(&self) -> u64 {
        self.frames_written.load(Ordering::Acquire)
    }

    /// True after the final sample was submitted.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// True when CPAL reported a stream error.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

/// Active native PCM playback stream.
pub struct PlaybackStream {
    stream: Stream,
    /// Shared sample clock.
    pub clock: Arc<PlaybackClock>,
    /// Output sample rate.
    pub sample_rate: u32,
    /// Output channels.
    pub channels: usize,
    /// Requested callback buffer size.
    pub buffer_frames: u32,
}

impl PlaybackStream {
    /// Starts playback of interleaved normalized PCM.
    ///
    /// PCM must already use the output device sample rate and channel count.
    pub fn start(
        selector: Option<&str>,
        pcm: Arc<Vec<f32>>,
        channels: usize,
        sample_rate: u32,
        requested_buffer_frames: u32,
    ) -> NativeResult<Self> {
        let device = select_device(selector, false)?;
        let supported = choose_output_config(&device, sample_rate, channels)?;
        let format = supported.sample_format();
        let mut config = supported.config();
        let selected_buffer = requested_buffer_size(supported.buffer_size(), requested_buffer_frames);
        let effective_buffer_frames = match selected_buffer {
            BufferSize::Fixed(frames) => frames,
            BufferSize::Default => requested_buffer_frames,
        };
        config.buffer_size = selected_buffer;
        let output_channels = config.channels as usize;
        let output_rate = config.sample_rate;
        if output_channels != channels || output_rate != sample_rate {
            return Err(format!(
                "selected stream is {output_rate} Hz/{output_channels} ch but PCM is {sample_rate} Hz/{channels} ch"
            )
            .into());
        }
        let clock = Arc::new(PlaybackClock::default());
        let failed = clock.clone();
        let error_callback = move |error: cpal::Error| {
            failed.failed.store(true, Ordering::Release);
            eprintln!("GlowBeat output stream error: {error}");
        };
        let stream = match format {
            SampleFormat::I8 => build_output::<i8, _>(&device, config, pcm, clock.clone(), error_callback)?,
            SampleFormat::I16 => build_output::<i16, _>(&device, config, pcm, clock.clone(), error_callback)?,
            SampleFormat::I24 => build_output::<I24, _>(&device, config, pcm, clock.clone(), error_callback)?,
            SampleFormat::U24 => build_output::<U24, _>(&device, config, pcm, clock.clone(), error_callback)?,
            SampleFormat::I32 => build_output::<i32, _>(&device, config, pcm, clock.clone(), error_callback)?,
            SampleFormat::I64 => build_output::<i64, _>(&device, config, pcm, clock.clone(), error_callback)?,
            SampleFormat::U8 => build_output::<u8, _>(&device, config, pcm, clock.clone(), error_callback)?,
            SampleFormat::U16 => build_output::<u16, _>(&device, config, pcm, clock.clone(), error_callback)?,
            SampleFormat::U32 => build_output::<u32, _>(&device, config, pcm, clock.clone(), error_callback)?,
            SampleFormat::U64 => build_output::<u64, _>(&device, config, pcm, clock.clone(), error_callback)?,
            SampleFormat::F32 => build_output::<f32, _>(&device, config, pcm, clock.clone(), error_callback)?,
            SampleFormat::F64 => build_output::<f64, _>(&device, config, pcm, clock.clone(), error_callback)?,
            other => return Err(format!("unsupported output sample format {other}").into()),
        };
        stream.play()?;
        Ok(Self {
            stream,
            clock,
            sample_rate: output_rate,
            channels: output_channels,
            buffer_frames: effective_buffer_frames,
        })
    }

    /// Approximate currently audible sample, compensating one callback buffer.
    #[must_use]
    pub fn audible_frame(&self) -> u64 {
        self.clock
            .frames_written()
            .saturating_sub(self.buffer_frames as u64)
    }

    /// Pauses playback.
    pub fn pause(&self) -> NativeResult<()> {
        self.stream.pause()?;
        Ok(())
    }
}

fn choose_output_config(
    device: &Device,
    sample_rate: u32,
    channels: usize,
) -> NativeResult<cpal::SupportedStreamConfig> {
    let mut same_channels_fallback = None;
    for range in device.supported_output_configs()? {
        if range.channels() as usize != channels {
            continue;
        }
        if sample_rate >= range.min_sample_rate() && sample_rate <= range.max_sample_rate() {
            return Ok(range.with_sample_rate(sample_rate));
        }
        if same_channels_fallback.is_none() {
            same_channels_fallback = Some(range.with_max_sample_rate());
        }
    }
    if let Some(fallback) = same_channels_fallback {
        return Ok(fallback);
    }
    Ok(device.default_output_config()?)
}

fn build_output<T, E>(
    device: &Device,
    config: StreamConfig,
    pcm: Arc<Vec<f32>>,
    clock: Arc<PlaybackClock>,
    error_callback: E,
) -> NativeResult<Stream>
where
    T: SizedSample + FromSample<f32>,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let channels = config.channels as usize;
    let mut scalar_position = 0_usize;
    let stream = device.build_output_stream(
        config,
        move |output: &mut [T], _| {
            for destination in output.iter_mut() {
                let sample = pcm.get(scalar_position).copied().unwrap_or(0.0);
                *destination = T::from_sample(sample);
                scalar_position = scalar_position.saturating_add(1);
            }
            clock
                .frames_written
                .store((scalar_position / channels) as u64, Ordering::Release);
            if scalar_position >= pcm.len() {
                clock.finished.store(true, Ordering::Release);
            }
        },
        error_callback,
        None,
    )?;
    Ok(stream)
}


/// Converts an interleaved PCM buffer to a new channel count.
///
/// Mono conversion averages all source channels. Expansion preserves the first
/// channels and repeats the last available source channel for additional
/// outputs. This helper is intended for the native demo player; embedded media
/// stacks may apply their own channel matrix.
#[must_use]
pub fn convert_channels(input: &[f32], source_channels: usize, target_channels: usize) -> Vec<f32> {
    if input.is_empty() || source_channels == 0 || target_channels == 0 {
        return Vec::new();
    }
    let frames = input.len() / source_channels;
    if source_channels == target_channels {
        return input[..frames * source_channels].to_vec();
    }
    let mut output = vec![0.0_f32; frames * target_channels];
    for frame in 0..frames {
        let source = &input[frame * source_channels..(frame + 1) * source_channels];
        if target_channels == 1 {
            output[frame] = source.iter().copied().sum::<f32>() / source_channels as f32;
        } else if source_channels == 1 {
            for channel in 0..target_channels {
                output[frame * target_channels + channel] = source[0];
            }
        } else {
            for channel in 0..target_channels {
                output[frame * target_channels + channel] = source[channel.min(source_channels - 1)];
            }
        }
    }
    output
}

/// Linear interleaved resampler used only by the native demo player.
/// Production embedded players should feed GlowBeat from their own decoded
/// sample clock and can use a higher-order resampler already present in the
/// media stack.
#[must_use]
pub fn resample_linear(
    input: &[f32],
    channels: usize,
    source_rate: u32,
    target_rate: u32,
) -> Vec<f32> {
    if source_rate == target_rate || input.is_empty() || channels == 0 {
        return input.to_vec();
    }
    let source_frames = input.len() / channels;
    if source_frames == 0 {
        return Vec::new();
    }
    let target_frames = ((source_frames as u128 * target_rate as u128 + source_rate as u128 / 2)
        / source_rate as u128) as usize;
    let mut output = vec![0.0_f32; target_frames.saturating_mul(channels)];
    let ratio = source_rate as f64 / target_rate as f64;
    for target_frame in 0..target_frames {
        let source = target_frame as f64 * ratio;
        let left = source.floor() as usize;
        let right = (left + 1).min(source_frames.saturating_sub(1));
        let fraction = (source - left as f64) as f32;
        for channel in 0..channels {
            let a = input[left.min(source_frames.saturating_sub(1)) * channels + channel];
            let b = input[right * channels + channel];
            output[target_frame * channels + channel] = a + (b - a) * fraction;
        }
    }
    output
}
