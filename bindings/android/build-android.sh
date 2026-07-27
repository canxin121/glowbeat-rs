#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
APP_ROOT="$(cd "$ROOT/../android-app" && pwd)"
ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"
if [[ -z "$ANDROID_SDK_ROOT" ]]; then
  printf 'ANDROID_SDK_ROOT or ANDROID_HOME is required\n' >&2
  exit 1
fi
if [[ -n "${ANDROID_NDK_HOME:-}" ]]; then
  NDK="$ANDROID_NDK_HOME"
else
  NDK="$(find "$ANDROID_SDK_ROOT/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -n 1)"
fi
if [[ ! -d "$NDK/toolchains/llvm/prebuilt" ]]; then
  printf 'Android NDK not found under %s\n' "$ANDROID_SDK_ROOT" >&2
  exit 1
fi
HOST_TAG="$(find "$NDK/toolchains/llvm/prebuilt" -mindepth 1 -maxdepth 1 -type d -exec basename {} \; | head -n 1)"
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin"
API=23
TARGETS=(aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android)
for target in "${TARGETS[@]}"; do
  rustup target add "$target"
  case "$target" in
    aarch64-linux-android) abi=arm64-v8a; clang=aarch64-linux-android${API}-clang ;;
    armv7-linux-androideabi) abi=armeabi-v7a; clang=armv7a-linux-androideabi${API}-clang ;;
    x86_64-linux-android) abi=x86_64; clang=x86_64-linux-android${API}-clang ;;
    i686-linux-android) abi=x86; clang=i686-linux-android${API}-clang ;;
  esac
  linker_key="CARGO_TARGET_$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_')_LINKER"
  env "$linker_key=$TOOLCHAIN/$clang" \
    cargo build --manifest-path "$ROOT/Cargo.toml" --release --target "$target" \
      --features ffi,android-jni
  destination="$APP_ROOT/androidApp/src/main/jniLibs/$abi"
  mkdir -p "$destination"
  cp "$ROOT/target/$target/release/libglowbeat.so" "$destination/"
done
printf 'Android libraries written to android-app/androidApp/src/main/jniLibs\n'
