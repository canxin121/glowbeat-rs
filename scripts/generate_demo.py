#!/usr/bin/env python3
"""Generate the deterministic 124 BPM validation track used by tests/docs."""
from __future__ import annotations

import argparse
import math
import random
import struct
import wave
from pathlib import Path


def generate(path: Path, seconds: float = 12.0, sample_rate: int = 48_000, bpm: float = 124.0) -> None:
    rng = random.Random(0x47_4C_4F_57)
    frames = int(round(seconds * sample_rate))
    beat_seconds = 60.0 / bpm
    bar_seconds = beat_seconds * 4.0
    roots = (130.8128, 146.8324, 174.6141, 195.9977)
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(2)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        block = bytearray()
        for index in range(frames):
            t = index / sample_rate
            section = min(3, int(t / max(seconds / 4.0, 1.0)))
            beat_phase = (t / beat_seconds) % 1.0
            beat_index = int(t / beat_seconds)
            bar_phase = (t / bar_seconds) % 1.0

            kick = 0.0
            if beat_phase < 0.16:
                kick = math.exp(-beat_phase * 30.0) * math.sin(
                    math.tau * (48.0 + 45.0 * (1.0 - beat_phase)) * t
                )

            snare_phase = ((t - beat_seconds) / (beat_seconds * 2.0)) % 1.0
            snare = 0.0
            if snare_phase < 0.075:
                snare = rng.uniform(-1.0, 1.0) * math.exp(-snare_phase * 52.0)

            hat_phase = (t / (beat_seconds * 0.5)) % 1.0
            hat = 0.0
            if hat_phase < 0.03:
                hat = rng.uniform(-1.0, 1.0) * math.exp(-hat_phase * 120.0)

            root = roots[int(t / bar_seconds) % len(roots)]
            chord = (
                0.46 * math.sin(math.tau * root * t)
                + 0.27 * math.sin(math.tau * root * 1.5 * t)
                + 0.18 * math.sin(math.tau * root * 2.0 * t)
            )
            section_gain = (0.22, 0.36, 0.56, 0.75)[section]
            buildup = bar_phase if section == 2 else 0.0
            drop_boost = (
                0.48 * math.exp(-beat_phase * 18.0)
                if section == 3 and beat_index % 4 == 0 and beat_phase < 0.20
                else 0.0
            )
            mono = section_gain * (0.52 * kick + 0.25 * snare + 0.08 * hat + 0.22 * chord)
            mono += 0.10 * buildup * hat + drop_boost
            pan = 0.12 * math.sin(math.tau * t / 8.0)
            left = max(-1.0, min(1.0, mono * (1.0 - pan)))
            right = max(-1.0, min(1.0, mono * (1.0 + pan)))
            block.extend(struct.pack("<hh", round(left * 32767), round(right * 32767)))
            if len(block) >= 1 << 20:
                wav.writeframesraw(block)
                block.clear()
        if block:
            wav.writeframesraw(block)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", nargs="?", default="assets/demo_124bpm.wav")
    parser.add_argument("--seconds", type=float, default=12.0)
    args = parser.parse_args()
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    generate(output, seconds=args.seconds)
    print(output)


if __name__ == "__main__":
    main()
