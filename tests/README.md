# tests

## Automated (run on the host, no phone needed)

```bash
cargo test --workspace
```

Covers, per milestone:

| Milestone | What is proven                                    | Where |
|-----------|---------------------------------------------------|-------|
| M0        | Protocol roundtrips, framing, error cases         | `protocol/src/lib.rs` (unit tests) |
| M2        | Frame production: full-frame, damage, packing, cadence | `host/src/testsource.rs` (unit tests) |
| M3+M4     | Full server↔client conversation over real TCP: handshake, frames, PING/PONG, FRAME_ACK, graceful DISCONNECT, RECONNECT resume + reject | `host/tests/loopback.rs` |

## Selftest benchmark (M2, runnable binary)

```bash
cargo run --release --bin usbra-host -- --selftest --frames 1800
```

Prints generate+encode latency percentiles and the bandwidth math that
motivates damage regions (no network, no phone).

## Manual checklists (need phone + cable)

- M3 — USB tunnel: `scripts/adb-usb-setup.sh` prints `tunnel up` and survives
  an unplug/replug cycle.
- M4/M5 — demo: with the tunnel running and the app open, the phone shows the
  animated test pattern at full brightness with no Wi-Fi enabled anywhere;
  the overlay reads ~60 fps, single-digit-ms RTT.
- Reconnect: unplug the cable for 10 s and replug — the pattern resumes after
  a full-frame resync without restarting either side.
- M6 (later): `scripts/m1-verify-displays.sh` before/during a session shows
  the extra connector; a window can be dragged onto the phone and maximized
  there; the physical monitor keeps working.
- M6a (probe, no phone): on a GNOME Wayland session,
  `scripts/m6-probe-virtual-monitor.py --hold 30` or
  `cargo run --features gnome -- --probe-gnome --hold 30` should add a second
  display in Settings → Displays for 30 s, then remove it.
- M6c (gnome source): `scripts/run-gnome-demo.sh` + phone app — Settings shows
  Display 2; a window dragged onto it appears on the phone; unplug removes it.
