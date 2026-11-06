#!/bin/sh
set -e

# Game Tunnel Tooling (GTT) Linux Uninstaller

if [ "$(id -u)" -eq 0 ]; then
    BIN_DIR="/usr/local/bin"
    APP_DIR="/usr/local/share/applications"
    ICON_DIR="/usr/local/share/icons/hicolor/scalable/apps"
    SYSTEMD_DIR="/etc/systemd/system"
    systemctl stop gtt.service 2>/dev/null || true
    systemctl disable gtt.service 2>/dev/null || true
else
    BIN_DIR="${HOME}/.local/bin"
    APP_DIR="${HOME}/.local/share/applications"
    ICON_DIR="${HOME}/.local/share/icons/hicolor/scalable/apps"
    SYSTEMD_DIR="${HOME}/.config/systemd/user"
    systemctl --user stop gtt.service 2>/dev/null || true
    systemctl --user disable gtt.service 2>/dev/null || true
fi

echo "==> Removing GTT files..."
rm -f "${BIN_DIR}/gtt"
rm -f "${APP_DIR}/gtt.desktop"
rm -f "${ICON_DIR}/gtt.svg"
rm -f "${SYSTEMD_DIR}/gtt.service"

if [ "$1" = "--remove-user-data" ]; then
    echo "==> Removing user server data and configurations..."
    rm -rf "${HOME}/.local/share/GTT"
fi

echo "==> GTT has been uninstalled."
