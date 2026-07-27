//! Native desktop demo for real-time capture and full-track playback.

use std::collections::VecDeque;
use std::env;
use std::error::Error;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use glowbeat::protocol::{
    chunk_count, encode_chunk, flags, FrameMetadata, WireLight, MAX_LIGHTS_PER_PACKET,
};
use glowbeat::wav::read_wav;
use glowbeat::{
    EngineConfig, FullTrackAnalyzer, LightPoint, LightingScore, MusicFrame, OutputMode,
    RenderConfig, Rgba, ScorePlayer, StreamingEngine,
};
use glowbeat_native::{
    convert_channels, devices, preferred_output_format, resample_linear, CaptureStream,
    PlaybackStream,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("glowbeat-native: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut args: VecDeque<String> = env::args().skip(1).collect();
    let Some(command) = args.pop_front() else {
        print_help();
        return Ok(());
    };
    match command.as_str() {
        "devices" => command_devices(args),
        "live" => command_live(args),
        "play" => command_play(args),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "version" | "--version" | "-V" => {
            println!("glowbeat-native {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err(format!("unknown command '{command}'").into()),
    }
}

fn command_devices(args: VecDeque<String>) -> Result<(), Box<dyn Error + Send + Sync>> {
    ensure_empty(&args)?;
    for device in devices()? {
        println!(
            "{}\tinput={}\toutput={}\t{}",
            device.id, device.input, device.output, device.name
        );
    }
    Ok(())
}

fn command_live(mut args: VecDeque<String>) -> Result<(), Box<dyn Error + Send + Sync>> {
    let input = take_string(&mut args, "--input")?;
    let udp: SocketAddr = take_string(&mut args, "--udp")?
        .ok_or("--udp HOST:PORT is required")?
        .parse()?;
    let lights: usize = parse_option(&mut args, "--lights", "64")?;
    let fps: f32 = parse_option(&mut args, "--fps", "120")?;
    let buffer_frames: u32 = parse_option(&mut args, "--buffer", "128")?;
    let queue_ms: u32 = parse_option(&mut args, "--queue-ms", "250")?;
    let lead_ms: u64 = parse_option(&mut args, "--lead-ms", "25")?;
    let seconds: f64 = parse_option(&mut args, "--seconds", "0")?;
    let amber = take_flag(&mut args, "--amber");
    let ring_layout = take_flag(&mut args, "--ring");
    ensure_empty(&args)?;
    validate_runtime(lights, fps)?;

    let mut capture = CaptureStream::open(input.as_deref(), buffer_frames, queue_ms)?;
    let points = if ring_layout {
        LightPoint::ring(lights)
    } else {
        LightPoint::line(lights)
    };
    let mut config = EngineConfig::default();
    config.analysis.sample_rate = capture.sample_rate();
    config.analysis.channels = capture.channels();
    config.analysis.max_frequency_hz = config
        .analysis
        .max_frequency_hz
        .min(config.analysis.sample_rate as f32 * 0.48);
    config.render.frames_per_second = fps;
    config.render.output_mode = if amber { OutputMode::Amber } else { OutputMode::Alpha };
    let mut engine = StreamingEngine::new(config, points.len())?;
    // The renderer and packet encoder both retain the layout for the lifetime of
    // the live session. This one-time clone stays outside the realtime loop.
    let mut sender = PacketSender::new(udp, points.clone(), lead_ms, amber)?;
    let mut capture_buffer = vec![0.0_f32; 32_768];
    let mut pending = Vec::<f32>::with_capacity(65_536);
    let mut colors = vec![Rgba::default(); lights];
    let render_step = capture.sample_rate() as f64 / fps as f64;
    let mut next_render = 0.0_f64;
    let started = Instant::now();
    let deadline = (seconds > 0.0).then(|| started + Duration::from_secs_f64(seconds));
    let mut last_status = Instant::now();

    println!(
        "live capture: {} Hz, {} channels, {} lights, {:.1} FPS -> {udp}",
        capture.sample_rate(),
        capture.channels(),
        lights,
        fps
    );
    if seconds <= 0.0 {
        println!("running until interrupted (Ctrl+C)");
    }

    loop {
        if deadline.is_some_and(|value| Instant::now() >= value) {
            break;
        }
        if capture.failed() {
            return Err("native capture stream failed".into());
        }
        let count = capture.drain(&mut capture_buffer);
        if count == 0 {
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        pending.extend_from_slice(&capture_buffer[..count]);
        let channels = capture.channels();
        let complete = pending.len() / channels * channels;
        if complete != 0 {
            engine.push_interleaved(&pending[..complete]);
            let remaining = pending.len() - complete;
            pending.copy_within(complete.., 0);
            pending.truncate(remaining);
        }

        let current_sample = engine.sample_index();
        if engine.latest_frame().is_some() && current_sample as f64 >= next_render {
            engine.render_at(current_sample, &points, &mut colors)?;
            let frame = engine.latest_frame().cloned().unwrap_or_default();
            sender.send(current_sample, &frame, &colors)?;
            while next_render <= current_sample as f64 {
                next_render += render_step;
            }
        }

        if last_status.elapsed() >= Duration::from_secs(2) {
            let frame = engine.latest_frame().cloned().unwrap_or_default();
            eprintln!(
                "t={:.1}s bpm={:.1} phase={:.2} confidence={:.2} dropped_samples={}",
                started.elapsed().as_secs_f64(),
                frame.bpm,
                frame.beat_phase,
                frame.beat_confidence,
                capture.dropped_samples()
            );
            last_status = Instant::now();
        }
    }
    Ok(())
}

fn command_play(mut args: VecDeque<String>) -> Result<(), Box<dyn Error + Send + Sync>> {
    let input = args.pop_front().ok_or("missing input WAV path")?;
    let output_device = take_string(&mut args, "--output")?;
    let udp: SocketAddr = take_string(&mut args, "--udp")?
        .ok_or("--udp HOST:PORT is required")?
        .parse()?;
    let lights: usize = parse_option(&mut args, "--lights", "64")?;
    let fps: f32 = parse_option(&mut args, "--fps", "120")?;
    let buffer_frames: u32 = parse_option(&mut args, "--buffer", "128")?;
    let lead_ms: u64 = parse_option(&mut args, "--lead-ms", "25")?;
    let amber = take_flag(&mut args, "--amber");
    let ring_layout = take_flag(&mut args, "--ring");
    ensure_empty(&args)?;
    validate_runtime(lights, fps)?;

    let wav = read_wav(&input)?;
    let source_channels = wav.channels as usize;
    let format = preferred_output_format(
        output_device.as_deref(),
        wav.sample_rate,
        source_channels,
    )?;
    let channel_converted = convert_channels(&wav.samples, source_channels, format.channels);
    let pcm = resample_linear(
        &channel_converted,
        format.channels,
        wav.sample_rate,
        format.sample_rate,
    );
    if pcm.is_empty() {
        return Err("decoded audio is empty".into());
    }

    eprintln!(
        "preanalyzing {:.2}s at {} Hz/{} ch...",
        pcm.len() as f64 / format.channels as f64 / format.sample_rate as f64,
        format.sample_rate,
        format.channels
    );
    let analysis_started = Instant::now();
    let analyzer = FullTrackAnalyzer::new(glowbeat::AnalysisConfig::default())?;
    let score = analyzer.analyze_pcm(&pcm, format.sample_rate, format.channels)?;
    eprintln!(
        "analysis complete in {:.2}s: {} frames, {} sections, {} accents",
        analysis_started.elapsed().as_secs_f64(),
        score.frames.len(),
        score.sections.len(),
        score.accents.len()
    );

    let points = if ring_layout {
        LightPoint::ring(lights)
    } else {
        LightPoint::line(lights)
    };
    let render_config = RenderConfig {
        frames_per_second: fps,
        output_mode: if amber {
            OutputMode::Amber
        } else {
            OutputMode::Alpha
        },
        ..RenderConfig::default()
    };
    let mut player = ScorePlayer::new(score, render_config, lights)?;
    let total_frames = pcm.len() as u64 / format.channels as u64;
    let playback = PlaybackStream::start(
        output_device.as_deref(),
        Arc::new(pcm),
        format.channels,
        format.sample_rate,
        buffer_frames,
    )?;
    let mut sender = PacketSender::new(udp, points.clone(), lead_ms, amber)?;
    let mut colors = vec![Rgba::default(); lights];
    let render_step = format.sample_rate as f64 / fps as f64;
    let mut next_render = 0.0_f64;
    let mut last_status = Instant::now();

    println!(
        "playing {} at {} Hz/{} ch, {} lights, {:.1} FPS -> {udp}",
        input, format.sample_rate, format.channels, lights, fps
    );
    loop {
        if playback.clock.failed() {
            return Err("native playback stream failed".into());
        }
        let audible = playback.audible_frame().min(total_frames);
        if audible as f64 >= next_render {
            player.render(audible, &points, &mut colors)?;
            let frame = active_frame(player.score(), audible).cloned().unwrap_or_default();
            sender.send(audible, &frame, &colors)?;
            while next_render <= audible as f64 {
                next_render += render_step;
            }
        }
        if last_status.elapsed() >= Duration::from_secs(2) {
            eprintln!(
                "playback {:.1}/{:.1}s",
                audible as f64 / format.sample_rate as f64,
                total_frames as f64 / format.sample_rate as f64
            );
            last_status = Instant::now();
        }
        if playback.clock.finished() && audible >= total_frames {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn active_frame(score: &LightingScore, sample_index: u64) -> Option<&MusicFrame> {
    score
        .frame_index_at(sample_index)
        .and_then(|index| score.frames.get(index))
}

struct PacketSender {
    socket: UdpSocket,
    target: SocketAddr,
    points: Vec<LightPoint>,
    wire: Vec<WireLight>,
    packet: Vec<u8>,
    sequence: u32,
    started: Instant,
    lead_us: u64,
    amber: bool,
    last_music_sample: Option<u64>,
}

impl PacketSender {
    fn new(
        target: SocketAddr,
        points: Vec<LightPoint>,
        lead_ms: u64,
        amber: bool,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let bind_address = if target.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
        let socket = UdpSocket::bind(bind_address)?;
        socket.set_nonblocking(false)?;
        Ok(Self {
            socket,
            target,
            wire: Vec::with_capacity(points.len()),
            points,
            packet: Vec::with_capacity(glowbeat::protocol::MAX_UDP_PAYLOAD),
            sequence: 0,
            started: Instant::now(),
            lead_us: lead_ms.saturating_mul(1_000),
            amber,
            last_music_sample: None,
        })
    }

    fn send(
        &mut self,
        sample_index: u64,
        frame: &MusicFrame,
        colors: &[Rgba],
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if colors.len() != self.points.len() {
            return Err("color/layout length mismatch".into());
        }
        self.wire.clear();
        self.wire.extend(
            self.points
                .iter()
                .zip(colors)
                .map(|(point, rgba)| WireLight { id: point.id, rgba: *rgba }),
        );
        let mut packet_flags = 0_u8;
        let new_music_frame = self.last_music_sample != Some(frame.sample_index);
        if new_music_frame && frame.beat_event {
            packet_flags |= flags::BEAT;
        }
        if new_music_frame && frame.downbeat_event {
            packet_flags |= flags::DOWNBEAT;
        }
        if new_music_frame && frame.drop > 0.15 {
            packet_flags |= flags::DROP;
        }
        self.last_music_sample = Some(frame.sample_index);
        if self.amber {
            packet_flags |= flags::AMBER;
        }
        let metadata = FrameMetadata {
            sequence: self.sequence,
            sample_index,
            render_time_us: self.started.elapsed().as_micros() as u64 + self.lead_us,
            bpm: frame.bpm,
            beat_phase: frame.beat_phase,
            flags: packet_flags,
        };
        let chunks = chunk_count(self.wire.len());
        for chunk_index in 0..chunks {
            let start = chunk_index as usize * MAX_LIGHTS_PER_PACKET;
            let end = (start + MAX_LIGHTS_PER_PACKET).min(self.wire.len());
            encode_chunk(
                metadata,
                chunk_index,
                chunks,
                &self.wire[start..end],
                &mut self.packet,
            )?;
            self.socket.send_to(&self.packet, self.target)?;
        }
        self.sequence = self.sequence.wrapping_add(1);
        Ok(())
    }
}

fn validate_runtime(lights: usize, fps: f32) -> Result<(), Box<dyn Error + Send + Sync>> {
    if lights == 0 || lights > 1_000_000 {
        return Err("--lights must be in 1..=1000000".into());
    }
    if !(1.0..=1_000.0).contains(&fps) {
        return Err("--fps must be in 1..=1000".into());
    }
    Ok(())
}

fn take_flag(args: &mut VecDeque<String>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|value| value == name) {
        args.remove(index);
        true
    } else {
        false
    }
}

fn take_string(
    args: &mut VecDeque<String>,
    name: &str,
) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
    if let Some(index) = args.iter().position(|value| value == name) {
        args.remove(index);
        let value = args
            .remove(index)
            .ok_or_else(|| format!("missing value after {name}"))?;
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

fn parse_option<T>(
    args: &mut VecDeque<String>,
    name: &str,
    default: &str,
) -> Result<T, Box<dyn Error + Send + Sync>>
where
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    Ok(take_string(args, name)?
        .unwrap_or_else(|| default.to_owned())
        .parse()?)
}

fn ensure_empty(args: &VecDeque<String>) -> Result<(), Box<dyn Error + Send + Sync>> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("unexpected arguments: {}", args.iter().cloned().collect::<Vec<_>>().join(" ")).into())
    }
}

fn print_help() {
    println!(
        r#"GlowBeat native CPAL adapter

USAGE:
  glowbeat-native devices
  glowbeat-native live --udp HOST:PORT [--input NAME_OR_ID]
                       [--lights N] [--fps N] [--buffer N]
                       [--queue-ms N] [--lead-ms N] [--seconds N]
                       [--amber] [--ring]
  glowbeat-native play INPUT.wav --udp HOST:PORT [--output NAME_OR_ID]
                       [--lights N] [--fps N] [--buffer N]
                       [--lead-ms N] [--amber] [--ring]

NOTES:
  • Windows screen audio: select a WASAPI loopback/capture device exposed by the OS.
  • macOS system audio: route through a virtual capture device when required.
  • Linux: select the PipeWire/Pulse monitor source exposed through ALSA.
  • The audio callback only converts and enqueues samples; DSP and UDP work run
    on this process's worker loop.
"#
    );
}
