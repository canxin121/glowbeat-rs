//! GlowBeat reference CLI. Run without arguments for usage.

use std::env;
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use glowbeat::config::{AnalysisConfig, EngineConfig, RenderConfig};
use glowbeat::score::LightingScore;
use glowbeat::types::{LightPoint, Rgba};
use glowbeat::wav::{read_wav, write_wav_f32, WavData};
use glowbeat::{FullTrackAnalyzer, ScorePlayer, StreamingEngine};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    let Some(command) = args.get(1).map(String::as_str) else {
        print_usage();
        return Ok(());
    };
    match command {
        "analyze" => {
            require_args(&args, 4, "glowbeat analyze <input.wav> <output.gbs>")?;
            let audio = read_wav(&args[2])?;
            let score = analyze_audio(&audio)?;
            score.save(&args[3])?;
            print_score_summary(&score);
            println!("saved {}", args[3]);
        }
        "inspect" => {
            require_args(&args, 3, "glowbeat inspect <score.gbs>")?;
            let score = LightingScore::load(&args[2])?;
            print_score_summary(&score);
            for (index, section) in score.sections.iter().enumerate() {
                println!(
                    "section {index:02}: {:7.2}s..{:7.2}s {:?} group={} intensity={:.2} narrative={:.2} {:?} hue={:.3}",
                    section.start_sample as f64 / score.sample_rate as f64,
                    section.end_sample as f64 / score.sample_rate as f64,
                    section.scene,
                    section.repeat_group,
                    section.intensity,
                    section.narrative_rank,
                    section.spatial_mode,
                    section.palette_hue,
                );
            }
        }
        "render" => {
            require_args(
                &args,
                4,
                "glowbeat render <score.gbs> <output.csv> [lights=64] [fps=60]",
            )?;
            let lights = parse_or(&args, 4, 64usize)?;
            let fps = parse_or(&args, 5, 60.0_f32)?;
            let score = LightingScore::load(&args[2])?;
            render_score_csv(score, &args[3], lights, fps)?;
            println!("saved {}", args[3]);
        }
        "full-preview" => {
            require_args(
                &args,
                4,
                "glowbeat full-preview <input.wav> <output.csv> [lights=64] [fps=60]",
            )?;
            let lights = parse_or(&args, 4, 64usize)?;
            let fps = parse_or(&args, 5, 60.0_f32)?;
            let audio = read_wav(&args[2])?;
            let score = analyze_audio(&audio)?;
            render_score_csv(score, &args[3], lights, fps)?;
            println!("saved {}", args[3]);
        }
        "stream-preview" => {
            require_args(
                &args,
                4,
                "glowbeat stream-preview <input.wav> <output.csv> [lights=64] [fps=60]",
            )?;
            let lights = parse_or(&args, 4, 64usize)?;
            let fps = parse_or(&args, 5, 60.0_f32)?;
            let audio = read_wav(&args[2])?;
            render_stream_csv(&audio, &args[3], lights, fps)?;
            println!("saved {}", args[3]);
        }
        "bench" => {
            require_args(&args, 3, "glowbeat bench <input.wav> [lights=256]")?;
            let lights = parse_or(&args, 3, 256usize)?;
            let audio = read_wav(&args[2])?;
            benchmark(&audio, lights)?;
        }
        "synth" => {
            require_args(
                &args,
                3,
                "glowbeat synth <output.wav> [seconds=24] [bpm=124]",
            )?;
            let seconds = parse_or(&args, 3, 24.0_f32)?;
            let bpm = parse_or(&args, 4, 124.0_f32)?;
            let audio = synthetic_song(48_000, seconds, bpm);
            write_wav_f32(&args[2], audio.sample_rate, audio.channels, &audio.samples)?;
            println!("saved {} ({seconds:.1}s at {bpm:.1} BPM)", args[2]);
        }
        "demo" => {
            let directory = args.get(2).map(String::as_str).unwrap_or("glowbeat-demo");
            std::fs::create_dir_all(directory)?;
            let wav = Path::new(directory).join("demo.wav");
            let score_path = Path::new(directory).join("demo.gbs");
            let full_csv = Path::new(directory).join("full-track.csv");
            let stream_csv = Path::new(directory).join("streaming.csv");
            let audio = synthetic_song(48_000, 24.0, 124.0);
            write_wav_f32(&wav, audio.sample_rate, audio.channels, &audio.samples)?;
            let score = analyze_audio(&audio)?;
            score.save(&score_path)?;
            render_score_csv(score, &full_csv, 64, 60.0)?;
            render_stream_csv(&audio, &stream_csv, 64, 60.0)?;
            println!("created demo files in {directory}");
        }
        "help" | "--help" | "-h" => print_usage(),
        other => return Err(format!("unknown command {other:?}").into()),
    }
    Ok(())
}

fn analyze_audio(audio: &WavData) -> Result<LightingScore, Box<dyn Error>> {
    let mut config = AnalysisConfig::default();
    config.sample_rate = audio.sample_rate;
    config.channels = audio.channels as usize;
    config.max_frequency_hz = config
        .max_frequency_hz
        .min(audio.sample_rate as f32 * 0.5 - 1.0);
    let analyzer = FullTrackAnalyzer::new(config)?;
    Ok(analyzer.analyze_pcm(&audio.samples, audio.sample_rate, audio.channels as usize)?)
}

fn render_score_csv(
    score: LightingScore,
    output_path: impl AsRef<Path>,
    light_count: usize,
    fps: f32,
) -> Result<(), Box<dyn Error>> {
    validate_preview(light_count, fps)?;
    let sample_rate = score.sample_rate;
    let duration = score.duration_samples;
    let mut player = ScorePlayer::new(score, RenderConfig::default(), light_count)?;
    let points = LightPoint::ring(light_count);
    let mut rgba = vec![Rgba::default(); light_count];
    let mut writer = BufWriter::new(File::create(output_path)?);
    writeln!(writer, "sample_index,time_seconds,light_id,r,g,b,a")?;
    let step = sample_rate as f64 / fps as f64;
    let mut frame_number = 0_u64;
    loop {
        let sample = (frame_number as f64 * step).round() as u64;
        if sample > duration {
            break;
        }
        player.render(sample, &points, &mut rgba)?;
        write_rgba_rows(&mut writer, sample_rate, sample, &points, &rgba)?;
        frame_number += 1;
    }
    writer.flush()?;
    Ok(())
}

fn render_stream_csv(
    audio: &WavData,
    output_path: impl AsRef<Path>,
    light_count: usize,
    fps: f32,
) -> Result<(), Box<dyn Error>> {
    validate_preview(light_count, fps)?;
    let mut config = EngineConfig::default();
    config.analysis.sample_rate = audio.sample_rate;
    config.analysis.channels = audio.channels as usize;
    config.analysis.max_frequency_hz = config
        .analysis
        .max_frequency_hz
        .min(audio.sample_rate as f32 * 0.5 - 1.0);
    config.render.frames_per_second = fps;
    let points = LightPoint::ring(light_count);
    let mut engine = StreamingEngine::new(config, points.len())?;
    let mut rgba = vec![Rgba::default(); light_count];
    let mut writer = BufWriter::new(File::create(output_path)?);
    writeln!(writer, "sample_index,time_seconds,light_id,r,g,b,a")?;

    // Advance PCM exactly to each render timestamp. This preserves causality:
    // the streaming preview never lets analysis observe samples that occur
    // after the frame being rendered.
    let channels = audio.channels as usize;
    let total_sample_frames = audio.samples.len() / channels;
    let render_step = audio.sample_rate as f64 / fps as f64;
    let mut consumed_sample_frames = 0usize;
    let mut render_frame = 0_u64;
    loop {
        let render_sample = (render_frame as f64 * render_step).round() as usize;
        if render_sample > total_sample_frames {
            break;
        }
        if render_sample > consumed_sample_frames {
            let start = consumed_sample_frames * channels;
            let end = render_sample * channels;
            engine.push_interleaved(&audio.samples[start..end]);
            consumed_sample_frames = render_sample;
        }
        engine.render_at(render_sample as u64, &points, &mut rgba)?;
        write_rgba_rows(
            &mut writer,
            audio.sample_rate,
            render_sample as u64,
            &points,
            &rgba,
        )?;
        render_frame += 1;
    }
    writer.flush()?;
    Ok(())
}

fn write_rgba_rows<W: Write>(
    writer: &mut W,
    sample_rate: u32,
    sample: u64,
    points: &[LightPoint],
    values: &[Rgba],
) -> Result<(), Box<dyn Error>> {
    let time = sample as f64 / sample_rate as f64;
    for (point, value) in points.iter().zip(values) {
        writeln!(
            writer,
            "{sample},{time:.6},{},{:.6},{:.6},{:.6},{:.6}",
            point.id, value.r, value.g, value.b, value.a
        )?;
    }
    Ok(())
}

fn benchmark(audio: &WavData, light_count: usize) -> Result<(), Box<dyn Error>> {
    if light_count == 0 {
        return Err("light count must be positive".into());
    }
    let analysis_start = Instant::now();
    let score = analyze_audio(audio)?;
    let analysis_elapsed = analysis_start.elapsed();
    let audio_seconds = audio.duration_seconds();
    let analysis_factor = audio_seconds / analysis_elapsed.as_secs_f64().max(1.0e-9);

    let mut player = ScorePlayer::new(score, RenderConfig::default(), light_count)?;
    let points = LightPoint::ring(light_count);
    let mut output = vec![Rgba::default(); light_count];
    let frames = (audio_seconds * 120.0).ceil() as u64;
    let render_start = Instant::now();
    for frame in 0..frames {
        let sample = (frame as f64 * audio.sample_rate as f64 / 120.0) as u64;
        player.render(sample, &points, &mut output)?;
    }
    let render_elapsed = render_start.elapsed();
    let lights_per_second =
        frames as f64 * light_count as f64 / render_elapsed.as_secs_f64().max(1.0e-9);
    println!("audio:              {audio_seconds:.3} s");
    println!(
        "full-track analysis: {:.3} s ({analysis_factor:.2}x realtime)",
        analysis_elapsed.as_secs_f64()
    );
    println!("render:             {:.3} s", render_elapsed.as_secs_f64());
    println!("render throughput:  {lights_per_second:.0} light-frames/s");
    println!(
        "last RGBA:          {:?}",
        output.last().copied().unwrap_or_default()
    );
    Ok(())
}

fn synthetic_song(sample_rate: u32, seconds: f32, bpm: f32) -> WavData {
    let channels = 2_u16;
    let sample_frames = (sample_rate as f32 * seconds.max(1.0)) as usize;
    let mut samples = vec![0.0_f32; sample_frames * channels as usize];
    let beat_period = sample_rate as f32 * 60.0 / bpm.max(30.0);
    let bar_period = beat_period * 4.0;
    let chord_roots = [48_i32, 55, 45, 53]; // C, G, A, F
    for frame in 0..sample_frames {
        let time = frame as f32 / sample_rate as f32;
        let beat_position = frame as f32 / beat_period;
        let beat_index = beat_position.floor() as usize;
        let beat_phase = beat_position.fract();
        let bar_index = (frame as f32 / bar_period).floor() as usize;
        let beat_in_bar = beat_index % 4;
        let section = (time / 6.0).floor() as usize;
        let section_gain = match section % 4 {
            0 => 0.45,
            1 => 0.72,
            2 => 0.54,
            _ => 0.92,
        };
        let kick = if beat_phase < 0.18 {
            let envelope = (-beat_phase / 0.045).exp();
            let frequency = 52.0 + 38.0 * (-beat_phase / 0.04).exp();
            (core::f32::consts::TAU * frequency * time).sin() * envelope
        } else {
            0.0
        } * if beat_in_bar == 0 { 0.85 } else { 0.62 };
        let snare_phase = if beat_in_bar == 1 || beat_in_bar == 3 {
            beat_phase
        } else {
            1.0
        };
        let pseudo_noise = (((frame as u32)
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223))
            >> 8) as f32
            / 16_777_216.0
            * 2.0
            - 1.0;
        let snare = if snare_phase < 0.12 {
            pseudo_noise * (-snare_phase / 0.035).exp() * 0.32
        } else {
            0.0
        };
        let eighth_phase = (beat_position * 2.0).fract();
        let hat = if eighth_phase < 0.06 {
            pseudo_noise * (-eighth_phase / 0.018).exp() * 0.075
        } else {
            0.0
        };
        let root_midi = chord_roots[bar_index % chord_roots.len()];
        let chord = [0_i32, 4, 7]
            .iter()
            .enumerate()
            .map(|(voice, interval)| {
                let frequency = 440.0 * 2.0_f32.powf((root_midi + *interval - 69) as f32 / 12.0);
                (core::f32::consts::TAU * frequency * time + voice as f32 * 0.6).sin()
            })
            .sum::<f32>()
            / 3.0;
        let pad_envelope = 0.65 + 0.35 * (core::f32::consts::TAU * beat_phase).sin().abs();
        let build = if section % 4 == 2 {
            ((time % 6.0) / 6.0).powi(2)
        } else {
            0.0
        };
        let mono = (0.47 * kick
            + 0.32 * snare
            + (0.06 + 0.10 * build) * hat
            + 0.24 * chord * pad_envelope)
            * section_gain;
        let pan = 0.08 * (core::f32::consts::TAU * 0.11 * time).sin();
        samples[frame * 2] = (mono * (1.0 - pan)).clamp(-1.0, 1.0);
        samples[frame * 2 + 1] = (mono * (1.0 + pan)).clamp(-1.0, 1.0);
    }
    WavData {
        sample_rate,
        channels,
        samples,
    }
}

fn print_score_summary(score: &LightingScore) {
    println!("sample rate:  {} Hz", score.sample_rate);
    println!("channels:     {}", score.channels);
    println!(
        "duration:     {:.3} s",
        score.duration_samples as f64 / score.sample_rate as f64
    );
    println!("music frames: {}", score.frames.len());
    println!("sections:     {}", score.sections.len());
    println!("accents:      {}", score.accents.len());
}

fn validate_preview(light_count: usize, fps: f32) -> Result<(), Box<dyn Error>> {
    if light_count == 0 || light_count > 1_000_000 {
        return Err("light count must be in 1..=1,000,000".into());
    }
    if !(1.0..=1_000.0).contains(&fps) {
        return Err("FPS must be in 1..=1000".into());
    }
    Ok(())
}

fn require_args(args: &[String], minimum: usize, usage: &str) -> Result<(), Box<dyn Error>> {
    if args.len() < minimum {
        Err(format!("usage: {usage}").into())
    } else {
        Ok(())
    }
}

fn parse_or<T>(args: &[String], index: usize, default: T) -> Result<T, Box<dyn Error>>
where
    T: std::str::FromStr,
    T::Err: Error + 'static,
{
    match args.get(index) {
        Some(value) => Ok(value.parse::<T>()?),
        None => Ok(default),
    }
}

fn print_usage() {
    println!(
        "GlowBeat — cross-platform music-to-RGBA engine\n\n\
         Commands:\n\
           glowbeat analyze <input.wav> <output.gbs>\n\
           glowbeat inspect <score.gbs>\n\
           glowbeat render <score.gbs> <output.csv> [lights=64] [fps=60]\n\
           glowbeat full-preview <input.wav> <output.csv> [lights=64] [fps=60]\n\
           glowbeat stream-preview <input.wav> <output.csv> [lights=64] [fps=60]\n\
           glowbeat bench <input.wav> [lights=256]\n\
           glowbeat synth <output.wav> [seconds=24] [bpm=124]\n\
           glowbeat demo [output-directory]\n\n\
         The built-in CLI decoder accepts PCM/float WAVE. Applications can feed\n\
         PCM decoded from MP3/AAC/FLAC/etc. directly through the Rust or C API."
    );
}
