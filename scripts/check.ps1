$ErrorActionPreference = "Stop"
Set-Location (Resolve-Path (Join-Path $PSScriptRoot ".."))

$python = Get-Command python3 -ErrorAction SilentlyContinue
if (-not $python) { $python = Get-Command python -ErrorAction SilentlyContinue }
if ($python) {
    & $python.Source scripts/static_sanity.py
    & $python.Source scripts/validate_assets.py
    & $python.Source scripts/validate_protocol.py
    & $python.Source scripts/validate_links.py
    & $python.Source -m py_compile scripts/*.py
}

$node = Get-Command node -ErrorAction SilentlyContinue
if ($node) { & $node.Source --check adapters/wasm/web/example.js }

$cc = Get-Command clang -ErrorAction SilentlyContinue
if (-not $cc) { $cc = Get-Command gcc -ErrorAction SilentlyContinue }
if ($cc) {
    & $cc.Source -std=c11 -Wall -Wextra -Werror -Iinclude -fsyntax-only examples/c_api_smoke.c
}

$cxx = Get-Command clang++ -ErrorAction SilentlyContinue
if (-not $cxx) { $cxx = Get-Command g++ -ErrorAction SilentlyContinue }
if ($cxx) {
    $ffiTest = Join-Path $env:TEMP "glowbeat-ffi-layout.exe"
    & $cxx.Source -std=c++17 -Wall -Wextra -Werror -Iinclude tests/ffi_layout.cpp -o $ffiTest
    & $ffiTest
    Remove-Item $ffiTest -ErrorAction SilentlyContinue
}

$swift = Get-Command swiftc -ErrorAction SilentlyContinue
if ($swift) { & $swift.Source -typecheck -Xcc -fmodule-map-file=bindings/swift/module.modulemap bindings/swift/GlowBeat.swift }

cargo fmt --all -- --check
cargo clippy --lib --bins --examples --tests --all-features -- -D warnings
cargo test --lib --bins --examples --tests --all-features
cargo test --release --lib --bins --examples --tests --all-features
cargo build --release --all-features
cargo run --release -- synth assets/check-demo.wav 6 124
cargo run --release -- analyze assets/check-demo.wav assets/check-demo.gbs
cargo run --release -- render assets/check-demo.gbs assets/check-demo.csv 8 120
Remove-Item assets/check-demo.wav, assets/check-demo.gbs, assets/check-demo.csv -ErrorAction SilentlyContinue
