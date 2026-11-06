#!/bin/sh
set -e

# Game Tunnel Tooling (GTT) Linux Installer
# Installs system-wide if root, or per-user in ~/.local if unprivileged.

if [ "$(id -u)" -eq 0 ]; then
    BIN_DIR="/usr/local/bin"
    APP_DIR="/usr/local/share/applications"
    ICON_DIR="/usr/local/share/icons/hicolor/scalable/apps"
    SYSTEMD_DIR="/etc/systemd/system"
    IS_SYSTEM=1
else
    BIN_DIR="${HOME}/.local/bin"
    APP_DIR="${HOME}/.local/share/applications"
    ICON_DIR="${HOME}/.local/share/icons/hicolor/scalable/apps"
    SYSTEMD_DIR="${HOME}/.config/systemd/user"
    IS_SYSTEM=0
fi

SCRIPT_DIR="$(dirname "$(readlink -f "$0")")"

echo "==> Installing Game Tunnel Tooling (GTT)..."
mkdir -p "${BIN_DIR}" "${APP_DIR}" "${ICON_DIR}" "${SYSTEMD_DIR}"

# 1. Install Binary
if [ -f "${SCRIPT_DIR}/bin/gtt" ]; then
    install -m 755 "${SCRIPT_DIR}/bin/gtt" "${BIN_DIR}/gtt"
elif [ -f "${SCRIPT_DIR}/gtt" ]; then
    install -m 755 "${SCRIPT_DIR}/gtt" "${BIN_DIR}/gtt"
fi

# 2. Install Desktop Entry & Icon
if [ -f "${SCRIPT_DIR}/share/applications/gtt.desktop" ]; then
    install -m 644 "${SCRIPT_DIR}/share/applications/gtt.desktop" "${APP_DIR}/gtt.desktop"
elif [ -f "${SCRIPT_DIR}/gtt.desktop" ]; then
    install -m 644 "${SCRIPT_DIR}/gtt.desktop" "${APP_DIR}/gtt.desktop"
fi

if [ -f "${SCRIPT_DIR}/share/icons/hicolor/scalable/apps/gtt.svg" ]; then
    install -m 644 "${SCRIPT_DIR}/share/icons/hicolor/scalable/apps/gtt.svg" "${ICON_DIR}/gtt.svg"
elif [ -f "${SCRIPT_DIR}/gtt.svg" ]; then
    install -m 644 "${SCRIPT_DIR}/gtt.svg" "${ICON_DIR}/gtt.svg"
fi

# 3. Install Systemd Unit
if [ -f "${SCRIPT_DIR}/systemd/gtt.service" ]; then
    install -m 644 "${SCRIPT_DIR}/systemd/gtt.service" "${SYSTEMD_DIR}/gtt.service"
elif [ -f "${SCRIPT_DIR}/gtt.service" ]; then
    install -m 644 "${SCRIPT_DIR}/gtt.service" "${SYSTEMD_DIR}/gtt.service"
fi

echo "==> Installation complete!"
echo "Binary: ${BIN_DIR}/gtt"
if [ "${IS_SYSTEM}" -eq 1 ]; then
    echo "To enable headless service: systemctl enable --now gtt.service"
else
    echo "To enable headless user service: systemctl --user enable --now gtt.service"
    echo "Make sure ${BIN_DIR} is in your PATH."
fi
