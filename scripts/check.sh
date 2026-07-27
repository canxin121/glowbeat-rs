#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PYTHON=""
if command -v python3 >/dev/null 2>&1; then
  PYTHON=python3
elif command -v python >/dev/null 2>&1; then
  PYTHON=python
fi
if [[ -n "$PYTHON" ]]; then
  "$PYTHON" scripts/static_sanity.py
  "$PYTHON" scripts/validate_assets.py
  "$PYTHON" scripts/validate_protocol.py
  "$PYTHON" scripts/validate_links.py
  "$PYTHON" -m py_compile scripts/*.py
fi

if command -v gcc >/dev/null 2>&1; then
  gcc -std=c11 -Wall -Wextra -Werror -Iinclude -fsyntax-only examples/c_api_smoke.c
fi
if command -v g++ >/dev/null 2>&1; then
  g++ -std=c++17 -Wall -Wextra -Werror -Iinclude tests/ffi_layout.cpp -o /tmp/glowbeat-ffi-layout
  /tmp/glowbeat-ffi-layout
fi
if command -v swiftc >/dev/null 2>&1; then
  swiftc -typecheck -Xcc -fmodule-map-file=bindings/swift/module.modulemap bindings/swift/GlowBeat.swift
fi
if command -v node >/dev/null 2>&1; then
  node --check adapters/wasm/web/example.js
fi

cargo fmt --all -- --check
cargo clippy --lib --bins --examples --tests --all-features -- -D warnings
cargo test --lib --bins --examples --tests --all-features
cargo test --release --lib --bins --examples --tests --all-features
cargo build --release --all-features
cargo run --release -- synth assets/check-demo.wav 6 124
cargo run --release -- analyze assets/check-demo.wav assets/check-demo.gbs
cargo run --release -- render assets/check-demo.gbs assets/check-demo.csv 8 120
rm -f assets/check-demo.wav assets/check-demo.gbs assets/check-demo.csv
