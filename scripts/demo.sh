#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
OUT="${1:-target/glowbeat-demo}"
mkdir -p "$OUT"
cargo run --release -- synth "$OUT/demo.wav" 12 124
cargo run --release -- analyze "$OUT/demo.wav" "$OUT/demo.gbs"
cargo run --release -- render "$OUT/demo.gbs" "$OUT/demo.csv" 32 120
printf 'Demo artifacts written to %s\n' "$OUT"
