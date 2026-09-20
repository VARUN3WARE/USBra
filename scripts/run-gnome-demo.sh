#!/usr/bin/env bash
# Run usbra-host with the GNOME RecordVirtual backend (M6).
# Requires: GNOME Wayland, --features gnome, python3-gi, gstreamer1.0-pipewire.
set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${PORT:-8899}"
STATS="${STATS:-/tmp/usbra-stats.jsonl}"
# 720p RAW over USB adb-reverse stays smooth; 1080p saturates and drops to ~5 FPS.
WIDTH="${WIDTH:-1280}"
HEIGHT="${HEIGHT:-720}"
FPS="${FPS:-30}"

exec cargo run --release --features gnome --bin usbra-host -- \
    --source gnome \
    --port "$PORT" \
    --width "$WIDTH" \
    --height "$HEIGHT" \
    --fps "$FPS" \
    --stats "$STATS" \
    --frame-ack \
    "$@"
