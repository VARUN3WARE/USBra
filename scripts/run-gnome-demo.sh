#!/usr/bin/env bash
# Run usbra-host with the GNOME RecordVirtual backend (M6).
# Requires: GNOME Wayland, --features gnome, python3-gi, gstreamer1.0-pipewire.
set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${PORT:-8899}"
STATS="${STATS:-/tmp/usbra-stats.jsonl}"

exec cargo run --release --features gnome --bin usbra-host -- \
    --source gnome \
    --port "$PORT" \
    --stats "$STATS" \
    --frame-ack \
    "$@"
