#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="${ROOT_DIR}/dist/linux"

if [ ! -f "${DIST_DIR}/me-edfork-workspace" ] || [ ! -f "${DIST_DIR}/workspace-share" ] || [ ! -f "${DIST_DIR}/libnautilus_workspace.so" ]; then
  echo "Linux release binaries not found; building first..."
  SKIP_LINUX_PACKAGES=1 bash "${ROOT_DIR}/scripts/build-linux.sh"
fi

python3 "${ROOT_DIR}/scripts/packaging/build_packages.py"
