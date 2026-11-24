#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-${ANDROID_SDK:-}}}"
if [ -z "$SDK" ]; then
  for candidate in "${HOME}/Android/Sdk" "${HOME}/Library/Android/sdk" "/opt/android-sdk"; do
    if [ -d "$candidate" ]; then
      SDK="$candidate"
      break
    fi
  done
fi

if [ -z "$SDK" ] || [ ! -d "$SDK" ]; then
  echo "ERROR: Android SDK not found. Set ANDROID_HOME (or ANDROID_SDK_ROOT)." >&2
  exit 1
fi

PLATFORM_JAR="${SDK}/platforms/android-37.0/android.jar"
if [ ! -f "$PLATFORM_JAR" ]; then
  PLATFORM_JAR="$(find "${SDK}/platforms" -name "android.jar" | sort -V | tail -n 1)"
fi
D8_BIN="$(find "${SDK}/build-tools" -name "d8" | sort -V | tail -n 1)"

if [ -z "$PLATFORM_JAR" ] || [ -z "$D8_BIN" ]; then
  echo "ERROR: Android SDK android.jar or d8 missing under $SDK" >&2
  exit 1
fi

BUILD_DIR="${ROOT_DIR}/target/bridge-dex"
OUT_DIR="${ROOT_DIR}/dist/android"
rm -rf "$BUILD_DIR"
mkdir -p "${BUILD_DIR}/classes" "$OUT_DIR"

javac --release 17 \
  -cp "$PLATFORM_JAR" \
  -d "${BUILD_DIR}/classes" \
  "${ROOT_DIR}/android/bridge-dex/java/me/edfork/workspace/bridge/ClipboardBridge.java"

"$D8_BIN" \
  --min-api 29 \
  --lib "$PLATFORM_JAR" \
  --output "${OUT_DIR}/workspace-bridge.jar" \
  $(find "${BUILD_DIR}/classes" -name "*.class")

chmod 0644 "${OUT_DIR}/workspace-bridge.jar"
echo "Built ${OUT_DIR}/workspace-bridge.jar"
