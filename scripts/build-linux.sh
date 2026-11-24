#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

bash "${ROOT_DIR}/scripts/sync-version.sh"

CARGO_EXTRA_ARGS=()
if [ "${WORKSPACE_OFFLINE:-0}" = "1" ] || [ ! -w "${CARGO_HOME:-$HOME/.cargo}" ]; then
  CARGO_EXTRA_ARGS+=(--offline)
fi

cargo clippy --workspace --all-targets "${CARGO_EXTRA_ARGS[@]}" -- -D warnings
cargo test --workspace "${CARGO_EXTRA_ARGS[@]}"
cargo build --release -p workspace-linux -p workspace-share -p workspace-nautilus "${CARGO_EXTRA_ARGS[@]}"

OUT_DIR="${ROOT_DIR}/dist/linux"
mkdir -p "$OUT_DIR"

cp "${ROOT_DIR}/target/release/me-edfork-workspace" "${OUT_DIR}/me-edfork-workspace"
cp "${ROOT_DIR}/target/release/workspace-share" "${OUT_DIR}/workspace-share"
cp "${ROOT_DIR}/target/release/libnautilus_workspace.so" "${OUT_DIR}/libnautilus_workspace.so"

chmod 0755 "${OUT_DIR}/me-edfork-workspace" "${OUT_DIR}/workspace-share" "${OUT_DIR}/libnautilus_workspace.so"

cp "${ROOT_DIR}/LICENSE" "${OUT_DIR}/LICENSE"
cp "${ROOT_DIR}/THIRD_PARTY_NOTICES.md" "${OUT_DIR}/THIRD_PARTY_NOTICES.md"
cp "${ROOT_DIR}/crates/workspace-nautilus/LICENSE" "${OUT_DIR}/LICENSE-nautilus-extension-GPL-3.0.txt"
chmod 0644 "${OUT_DIR}/LICENSE" "${OUT_DIR}/THIRD_PARTY_NOTICES.md" "${OUT_DIR}/LICENSE-nautilus-extension-GPL-3.0.txt"

if ldd "${OUT_DIR}/me-edfork-workspace" | grep -qi gtk-3; then
  echo "ERROR: me-edfork-workspace unexpectedly links libgtk-3" >&2
  exit 1
fi
if ldd "${OUT_DIR}/workspace-share" | grep -qi gtk-3; then
  echo "ERROR: workspace-share unexpectedly links libgtk-3" >&2
  exit 1
fi

nm -D "${OUT_DIR}/libnautilus_workspace.so" | grep -E " nautilus_module_(initialize|list_types|shutdown)$"

echo "Linux release artifacts ready under ${OUT_DIR}"

if [ "${SKIP_LINUX_PACKAGES:-0}" != "1" ]; then
  bash "${ROOT_DIR}/scripts/package-linux.sh"
fi
