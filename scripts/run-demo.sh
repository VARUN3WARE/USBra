#!/usr/bin/env bash
# Build (if needed) and run the USBra host with the test-pattern source (M2–M5 demo).
#
# Usage: scripts/run-demo.sh [-- extra usbra-host args…]
#   PORT=9000 scripts/run-demo.sh
#   scripts/run-demo.sh -- --width 1920 --height 1080
set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${PORT:-8899}"
STATS="${STATS:-/tmp/usbra-stats.jsonl}"

exec cargo run --release --bin usbra-host -- \
    --source test \
    --port "$PORT" \
    --stats "$STATS" \
    --frame-ack \
    "$@"
