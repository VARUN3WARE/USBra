#!/usr/bin/env bash
# USBra USB tunnel keeper (docs/03-usb-transport.md).
#
# Ensures an Android device is attached and keeps the reverse tunnel
# (phone 127.0.0.1:PORT -> host 127.0.0.1:PORT) installed across replugs.
#
# Usage: scripts/adb-usb-setup.sh [port]     (default port: 8899)
set -euo pipefail

PORT="${1:-8899}"

command -v adb >/dev/null 2>&1 || {
    echo "adb not found. Install it:  sudo apt install adb" >&2
    exit 1
}

echo "[usbra] starting adb server (if needed)…"
adb start-server >/dev/null 2>&1 || true

echo "[usbra] waiting for a USB device (enable USB debugging, plug in, accept the prompt)…"
adb wait-for-device

install_tunnel() {
    adb reverse tcp:"$PORT" tcp:"$PORT" >/dev/null
    echo "[usbra] tunnel up: phone 127.0.0.1:$PORT -> host 127.0.0.1:$PORT"
}

install_tunnel

echo "[usbra] watching for replugs (Ctrl-C to stop; the host re-installs on its own is M9)…"
while true; do
    sleep 2
    state="$(adb get-state 2>/dev/null || echo offline)"
    if [ "$state" != "device" ]; then
        echo "[usbra] device gone ($state); waiting…"
        adb wait-for-device
        install_tunnel
        echo "[usbra] re-tunneled."
    fi
done
