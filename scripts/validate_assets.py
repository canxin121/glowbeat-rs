#!/usr/bin/env python3
"""Validate deterministic repository assets without third-party packages."""
from __future__ import annotations

import math
import struct
import wave
from pathlib import Path


def read_mono(path: Path) -> tuple[int, list[float]]:
    with wave.open(str(path), "rb") as wav:
        assert wav.getnchannels() == 2, "demo asset must be stereo"
        assert wav.getsampwidth() == 2, "demo asset must be PCM16"
        rate = wav.getframerate()
        raw = wav.readframes(wav.getnframes())
    samples = struct.unpack(f"<{len(raw) // 2}h", raw)
    mono = [(samples[i] + samples[i + 1]) / 65536.0 for i in range(0, len(samples), 2)]
    return rate, mono


def estimate_bpm(rate: int, mono: list[float]) -> float:
    hop = 240  # 200 Hz envelope
    envelope: list[float] = []
    previous = 0.0
    for start in range(0, len(mono) - hop + 1, hop):
        rms = math.sqrt(sum(x * x for x in mono[start : start + hop]) / hop)
        onset = max(0.0, rms - previous)
        envelope.append(onset)
        previous = 0.82 * previous + 0.18 * rms
    frame_rate = rate / hop
    best_lag = 1
    best_score = -1.0
    for bpm in range(80, 181):
        lag = max(1, round(frame_rate * 60.0 / bpm))
        score = sum(envelope[i] * envelope[i - lag] for i in range(lag, len(envelope)))
        score += 0.35 * sum(
            envelope[i] * envelope[i - 2 * lag] for i in range(2 * lag, len(envelope))
        )
        if score > best_score:
            best_score = score
            best_lag = lag
    return 60.0 * frame_rate / best_lag


def main() -> None:
    path = Path(__file__).resolve().parents[1] / "assets" / "demo_124bpm.wav"
    assert path.is_file(), f"missing {path}"
    rate, mono = read_mono(path)
    assert rate == 48_000
    duration = len(mono) / rate
    assert 11.9 <= duration <= 12.1
    peak = max(abs(value) for value in mono)
    assert 0.05 < peak <= 1.0
    bpm = estimate_bpm(rate, mono)
    assert abs(bpm - 124.0) < 5.0, f"BPM estimate {bpm:.2f}"
    print(
        f"asset validation passed: {rate} Hz, {duration:.3f}s, "
        f"peak={peak:.3f}, estimated_bpm={bpm:.2f}"
    )


if __name__ == "__main__":
    main()
