#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST="${ROOT_DIR}/dist/linux"

if [ ! -f "${DIST}/me-edfork-workspace" ] || [ ! -f "${DIST}/workspace-share" ] || [ ! -f "${DIST}/libnautilus_workspace.so" ]; then
  echo "Building Linux artifacts first..."
  "${ROOT_DIR}/scripts/build-linux.sh"
fi

mkdir -p "${HOME}/.local/bin"
mkdir -p "${HOME}/.local/share/applications"
mkdir -p "${HOME}/.config/systemd/user"

BIN_PATH="${HOME}/.local/bin/me-edfork-workspace"

install -m 0755 "${DIST}/me-edfork-workspace" "${BIN_PATH}"
install -m 0755 "${DIST}/workspace-share" "${HOME}/.local/bin/workspace-share"

ICON_SRC="${ROOT_DIR}/assets/me.edfork.workspace.png"
rm -f "${HOME}/.local/share/icons/hicolor/scalable/apps/me.edfork.workspace.svg"
for sz in 512 256 128 64 48; do
  ICON_DIR="${HOME}/.local/share/icons/hicolor/${sz}x${sz}/apps"
  mkdir -p "${ICON_DIR}"
  if [ "$sz" = "512" ]; then
    install -m 0644 "${ICON_SRC}" "${ICON_DIR}/me.edfork.workspace.png"
  elif command -v magick >/dev/null 2>&1; then
    magick "${ICON_SRC}" -resize "${sz}x${sz}" "${ICON_DIR}/me.edfork.workspace.png"
    chmod 0644 "${ICON_DIR}/me.edfork.workspace.png"
  elif command -v python3 >/dev/null 2>&1 && python3 -c "import PIL" >/dev/null 2>&1; then
    python3 -c "from PIL import Image; Image.open('${ICON_SRC}').resize((${sz},${sz}), Image.Resampling.LANCZOS).save('${ICON_DIR}/me.edfork.workspace.png', 'PNG')"
    chmod 0644 "${ICON_DIR}/me.edfork.workspace.png"
  else
    install -m 0644 "${ICON_SRC}" "${ICON_DIR}/me.edfork.workspace.png"
  fi
done

install -m 0644 "${ROOT_DIR}/assets/me-edfork-workspace.service" "${HOME}/.config/systemd/user/me-edfork-workspace.service"

# Write .desktop with absolute binary paths because graphical user sessions (Hyprland/systemd)
# may not include ~/.local/bin in PATH.
cat > "${HOME}/.local/share/applications/me.edfork.workspace.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=寰宇工作台
GenericName=LAN Device Bridge
Comment=Secure LAN clipboard sync and file transfer between Linux and Android
TryExec=${BIN_PATH}
Exec=${BIN_PATH}
Icon=me.edfork.workspace
Terminal=false
Categories=Network;Utility;
StartupNotify=false
StartupWMClass=me.edfork.workspace
EOF
chmod 0644 "${HOME}/.local/share/applications/me.edfork.workspace.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "${HOME}/.local/share/applications" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "${HOME}/.local/share/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "Installed binaries & systemd service to ~/.local and ~/.config/systemd/user"
echo ""
echo "To enable background tray daemon at login:"
echo "  systemctl --user daemon-reload && systemctl --user enable --now me-edfork-workspace.service"
echo ""
echo "To install Nautilus 4 right-click extension ('Share with Workspace'):"
echo "  sudo install -m 0755 \"${DIST}/libnautilus_workspace.so\" /usr/lib/nautilus/extensions-4/libnautilus_workspace.so"
echo "  nautilus -q"

if command -v systemctl >/dev/null 2>&1 && systemctl is-active --quiet ufw; then
  echo ""
  echo "Detected active UFW firewall. Allow LAN discovery (45876/udp) and mTLS (45877/tcp):"
  echo "  sudo ufw allow 45876/udp comment 'Workspace LAN Discovery'"
  echo "  sudo ufw allow 45877/tcp comment 'Workspace LAN mTLS'"
fi
