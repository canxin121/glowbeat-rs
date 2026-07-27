#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
archive_root="glowbeat-rs"
out="${GLOWBEAT_PACKAGE_OUT:-$(dirname "$root")/glowbeat-rs-complete.zip}"
checksum_out="${out}.sha256"

command -v zip >/dev/null 2>&1 || { echo "zip is required" >&2; exit 1; }

if find "$root" -type l -print -quit | grep -q .; then
  echo "refusing to package repository symlinks" >&2
  find "$root" -type l -print >&2
  exit 1
fi

rm -f "$out" "$checksum_out"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/$archive_root"

(
  cd "$root"
  find . \
    -path './.git' -prune -o \
    -path './target' -prune -o \
    -path './scripts/__pycache__' -prune -o \
    -path './adapters/cpal/target' -prune -o \
    -path './adapters/wasm/target' -prune -o \
    -name '*.pyc' -prune -o \
    -name '*.pyo' -prune -o \
    -name 'check-demo.wav' -prune -o \
    -name 'check-demo.gbs' -prune -o \
    -name 'check-demo.csv' -prune -o \
    -type f -print0
) | (
  cd "$root"
  while IFS= read -r -d '' file; do
    destination="$tmp/$archive_root/${file#./}"
    mkdir -p "$(dirname "$destination")"
    cp -p "$file" "$destination"
  done
)

(
  cd "$tmp/$archive_root"
  find . -type f ! -name MANIFEST.sha256 -print0 \
    | sort -z \
    | xargs -0 sha256sum > MANIFEST.sha256
)

(
  cd "$tmp"
  zip -q -9 -r "$out" "$archive_root"
)
sha256sum "$out" | tee "$checksum_out"
printf '%s\n' "$out"
