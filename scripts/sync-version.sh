#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VERSION_FILE="${ROOT_DIR}/version.properties"

if [ ! -f "$VERSION_FILE" ]; then
  echo "ERROR: $VERSION_FILE not found" >&2
  exit 1
fi

python3 - "$ROOT_DIR" "$VERSION_FILE" <<'PY'
import os
import re
import sys

root_dir, version_file = sys.argv[1], sys.argv[2]

props = {}
with open(version_file, "r", encoding="utf-8") as f:
    for raw in f:
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        props[k.strip()] = v.strip()

version_name = props["VERSION_NAME"]
version_display = props["VERSION_DISPLAY"]
version_code = props["VERSION_CODE"]
cargo_version = props["CARGO_VERSION"]

if not re.fullmatch(r"\d+", version_code):
    raise SystemExit(f"Invalid VERSION_CODE: {version_code!r}")


def write_if_changed(path: str, new_text: str) -> None:
    old_text = None
    if os.path.exists(path):
        with open(path, "r", encoding="utf-8") as f:
            old_text = f.read()
    if old_text != new_text:
        with open(path, "w", encoding="utf-8") as f:
            f.write(new_text)
        print(f"Synced version -> {os.path.relpath(path, root_dir)}")


def update_prop_file(path: str, updates: dict[str, str]) -> None:
    with open(path, "r", encoding="utf-8") as f:
        lines = f.read().splitlines()
    out = []
    seen = set()
    for line in lines:
        if "=" in line and not line.lstrip().startswith("#"):
            key = line.split("=", 1)[0].strip()
            if key in updates:
                out.append(f"{key}={updates[key]}")
                seen.add(key)
                continue
        out.append(line)
    for key, val in updates.items():
        if key not in seen:
            out.append(f"{key}={val}")
    write_if_changed(path, "\n".join(out) + "\n")


# 1. Cargo.toml ([workspace.package] version)
cargo_toml = os.path.join(root_dir, "Cargo.toml")
with open(cargo_toml, "r", encoding="utf-8") as f:
    cargo_text = f.read()

pattern = re.compile(r'(\[workspace\.package\]\s*\n(?:[^\[]*\n)*?version\s*=\s*)"([^"]+)"')
m = pattern.search(cargo_text)
if not m:
    raise SystemExit("Could not locate [workspace.package] version in Cargo.toml")
new_cargo_text = cargo_text[:m.start(2)] + cargo_version + cargo_text[m.end(2):]
write_if_changed(cargo_toml, new_cargo_text)

# 2. KernelSU module.prop
ksu_prop = os.path.join(root_dir, "android", "ksu-module", "module.prop")
update_prop_file(
    ksu_prop,
    {
        "version": version_name,
        "versionCode": version_code,
        "description": f"寰宇工作台 {version_display} · 局域网安全互联守护进程 (workspaced) 与跨端剪贴板桥接模块",
    },
)

# 3. LSP / LibXposed META-INF/xposed/module.prop
xposed_prop = os.path.join(
    root_dir, "android", "app", "src", "main", "resources", "META-INF", "xposed", "module.prop"
)
update_prop_file(
    xposed_prop,
    {
        "version": version_name,
        "versionCode": version_code,
        "description": f"寰宇工作台 {version_display} · 局域网连接门控剪贴板手势（SystemUI）与 OPPO 分享直传（OShare）模块",
    },
)
PY
