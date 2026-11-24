#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
bash "${ROOT_DIR}/scripts/sync-version.sh"

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
  echo "ERROR: Android SDK not found. Set ANDROID_HOME (or ANDROID_SDK_ROOT) to your SDK directory." >&2
  exit 1
fi

export ANDROID_HOME="$SDK"
export ANDROID_SDK="$SDK"
export ANDROID_SDK_ROOT="$SDK"

NDK_DIR="${ANDROID_NDK_HOME:-}"
if [ -z "$NDK_DIR" ] && [ -d "${SDK}/ndk" ]; then
  NDK_DIR="$(find "${SDK}/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -n 1)"
fi
if [ -z "$NDK_DIR" ]; then
  echo "ERROR: Android NDK missing under ${SDK}/ndk (set ANDROID_NDK_HOME or install NDK via sdkmanager)" >&2
  exit 1
fi

HOST_TAG="$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64"
TOOLCHAIN="${NDK_DIR}/toolchains/llvm/prebuilt/${HOST_TAG}/bin"
if [ ! -d "$TOOLCHAIN" ]; then
  TOOLCHAIN="$(find "${NDK_DIR}/toolchains/llvm/prebuilt" -maxdepth 2 -name bin -type d | head -n 1)"
fi

CC_BIN="$(find "$TOOLCHAIN" -maxdepth 1 -name 'aarch64-linux-android*-clang' | sort -V | grep -E 'android(2[4-9]|3[0-9])-clang$' | tail -n 1)"
if [ -z "$CC_BIN" ]; then
  CC_BIN="$(find "$TOOLCHAIN" -maxdepth 1 -name 'aarch64-linux-android*-clang' | sort -V | tail -n 1)"
fi
AR_BIN="${TOOLCHAIN}/llvm-ar"

if [ -z "$CC_BIN" ] || [ ! -x "$AR_BIN" ]; then
  echo "ERROR: NDK aarch64 clang / llvm-ar not found under $TOOLCHAIN" >&2
  exit 1
fi

CARGO_EXTRA_ARGS=()
if [ "${WORKSPACE_OFFLINE:-0}" = "1" ] || [ ! -w "${CARGO_HOME:-$HOME/.cargo}" ]; then
  CARGO_EXTRA_ARGS+=(--offline)
fi

GRADLE_EXTRA_ARGS=()
if [ "${WORKSPACE_OFFLINE:-0}" = "1" ]; then
  GRADLE_EXTRA_ARGS+=(--offline)
fi

OUT_DIR="${ROOT_DIR}/dist/android"
mkdir -p "$OUT_DIR"

echo "=== [1/4] Building workspace-bridge.jar (UID-1000 DEX) ==="
bash "${ROOT_DIR}/android/bridge-dex/build.sh"

echo "=== [2/4] Cross-compiling workspaced for aarch64-linux-android ==="
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_BIN"
export CC_aarch64_linux_android="$CC_BIN"
export AR_aarch64_linux_android="$AR_BIN"
export CARGO_PROFILE_RELEASE_LTO=thin
export CARGO_PROFILE_RELEASE_STRIP=symbols

(
  cd "$ROOT_DIR"
  cargo build --release --target aarch64-linux-android -p workspace-android "${CARGO_EXTRA_ARGS[@]}"
)
cp "${ROOT_DIR}/target/aarch64-linux-android/release/workspaced" "${OUT_DIR}/workspaced"
chmod 0755 "${OUT_DIR}/workspaced"
file "${OUT_DIR}/workspaced"

echo "=== [3/4] Running Gradle unit tests and building me.edfork.workspace APK ==="
(
  cd "${ROOT_DIR}/android"
  ./gradlew "${GRADLE_EXTRA_ARGS[@]}" :app:testDebugUnitTest :app:assembleDebug
)
APK_SRC="${ROOT_DIR}/android/app/build/outputs/apk/debug/app-debug.apk"
cp "$APK_SRC" "${OUT_DIR}/me.edfork.workspace.apk"
chmod 0644 "${OUT_DIR}/me.edfork.workspace.apk"

cp "${ROOT_DIR}/LICENSE" "${OUT_DIR}/LICENSE"
cp "${ROOT_DIR}/THIRD_PARTY_NOTICES.md" "${OUT_DIR}/THIRD_PARTY_NOTICES.md"
chmod 0644 "${OUT_DIR}/LICENSE" "${OUT_DIR}/THIRD_PARTY_NOTICES.md"

echo "=== [4/4] Packaging KernelSU module zip ==="
STAGE_DIR="${ROOT_DIR}/target/ksu-stage"
rm -rf "$STAGE_DIR"
mkdir -p "${STAGE_DIR}/bin" "${STAGE_DIR}/dex"
cp "${ROOT_DIR}/android/ksu-module/module.prop" "${STAGE_DIR}/module.prop"
cp "${ROOT_DIR}/android/ksu-module/customize.sh" "${STAGE_DIR}/customize.sh"
cp "${ROOT_DIR}/android/ksu-module/service.sh" "${STAGE_DIR}/service.sh"
cp "${OUT_DIR}/workspaced" "${STAGE_DIR}/bin/workspaced"
cp "${OUT_DIR}/workspace-bridge.jar" "${STAGE_DIR}/dex/workspace-bridge.jar"
cp "${ROOT_DIR}/LICENSE" "${STAGE_DIR}/LICENSE"
cp "${ROOT_DIR}/THIRD_PARTY_NOTICES.md" "${STAGE_DIR}/THIRD_PARTY_NOTICES.md"

python3 - "${STAGE_DIR}" "${OUT_DIR}/me_edfork_workspace-ksu.zip" <<'PY'
import os, stat, sys, zipfile

stage_dir, out_zip = sys.argv[1], sys.argv[2]
exec_names = {"workspaced", "customize.sh", "service.sh"}

with zipfile.ZipFile(out_zip, "w", compression=zipfile.ZIP_DEFLATED) as zf:
    for root, dirs, files in os.walk(stage_dir):
        dirs.sort()
        files.sort()
        for fname in files:
            full = os.path.join(root, fname)
            rel = os.path.relpath(full, stage_dir).replace(os.sep, "/")
            info = zipfile.ZipInfo(rel)
            info.compress_type = zipfile.ZIP_DEFLATED
            mode = 0o755 if fname in exec_names else 0o644
            info.external_attr = (stat.S_IFREG | mode) << 16
            with open(full, "rb") as f:
                zf.writestr(info, f.read())
print(f"Packaged {out_zip}")
PY

ls -lh "$OUT_DIR"
