#!/usr/bin/env bash
# Install ProArt for the current user (binaries in ~/.local/bin) and set up i2c access.
#   packaging/linux/install.sh [--system-i2c]   (--system-i2c writes the udev rule + i2c-dev module list with sudo)
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --release -p proart-app -p proart-cli
BIN="${HOME}/.local/bin"
APPS="${HOME}/.local/share/applications"
ICONS="${HOME}/.local/share/icons/hicolor/256x256/apps"
mkdir -p "$BIN" "$APPS" "$ICONS"
install -m755 target/release/proart-app "$BIN/proart-app"
install -m755 target/release/proart "$BIN/proart"
install -m644 packaging/linux/proart.desktop "$APPS/proart.desktop"
install -m644 assets/icons/app-256.png "$ICONS/proart.png"
echo "installed to $BIN (make sure it is on your PATH)"
if [ "${1:-}" = "--system-i2c" ]; then
    sudo install -m644 packaging/linux/60-proart-i2c.rules /etc/udev/rules.d/60-proart-i2c.rules
    echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf >/dev/null
    sudo modprobe i2c-dev
    sudo udevadm control --reload && sudo udevadm trigger
    echo "i2c access configured; log out and in again if /dev/i2c-* is still not writable"
else
    echo "DDC/CI needs the i2c-dev module and write access to /dev/i2c-*: re-run with --system-i2c"
fi
