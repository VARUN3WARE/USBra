# USBra

**Turn an Android phone into a real secondary display for Ubuntu — over a USB cable.**

```
Ubuntu PC
 ├── Physical Monitor 1
 └── USBra Virtual Monitor 2
          │ USB cable
          ▼
       Android phone
```

This is not screen mirroring. The Linux desktop gets a **genuine second
display** — drag windows onto the phone, maximize them there, arrange it in
Settings → Displays, keep using Monitor 1 normally.

## Principles

USB-first · extremely low latency · minimal CPU/GPU · damage-region updates ·
no cloud · no account · no telemetry · no subscription · open source (MIT) ·
native (Rust + Kotlin) · no unnecessary video encoding.

## Status (Sep 2026)

| Piece | State |
|---|---|
| Wire protocol v1 (`protocol/`) | ✅ implemented + tested |
| Host: test-pattern source, TCP server, selftest, stats (`host/`) | ✅ implemented + tested |
| Android client: GLES renderer, reconnect (`android/`) | ✅ implemented (build on device per docs) |
| USB transport via `adb reverse` | ✅ (scripts) |
| **M6: GNOME virtual display** (`--features gnome --source gnome`) | ✅ D-Bus + GStreamer grabber wired |
| Native pipewire-rs + damage metadata | 🔜 needs `libpipewire-0.3-dev` |
| Benchmarks on hardware | harness ready (`benchmarks/`) |

See `docs/05-milestones.md` for the full roadmap and acceptance criteria.

## Quickstart

```bash
# Ubuntu (host)
sudo apt install -y cargo adb python3-gi gstreamer1.0-tools gstreamer1.0-pipewire
cargo build --release
./target/release/usbra-host --selftest          # no phone needed

# Phone: enable USB debugging, plug in, then:
scripts/adb-usb-setup.sh                        # terminal 1 (USB tunnel)

# Test pattern (no virtual Display 2 yet):
scripts/run-demo.sh                             # terminal 2

# Real Display 2 on GNOME Wayland (M6):
cargo run --release --features gnome -- \
  --source gnome --frame-ack --stats /tmp/usbra.jsonl
# ...and launch the USBra app on the phone (docs/10-setup-android.md)
```

## How it works (short version)

On GNOME (Ubuntu's default), USBra's host calls mutter's
`org.gnome.Mutter.ScreenCast.RecordVirtual()` D-Bus API — the same mechanism
GNOME's own remote-desktop "extend" mode uses — which creates a virtual
monitor in your live session (`is-platform=true` ⇒ treated like a real
monitor, not screen sharing). Frames arrive over PipeWire with per-frame
damage rectangles (`SPA_META_VideoDamage`); only damaged rects are packed and
sent over the USB cable via `adb reverse` (root-free, driver-free, the
scrcpy-proven transport). The Android app uploads those rects into a GL
texture and presents them. Unplug the cable → the session drops → GNOME
removes Display 2 automatically, like pulling a monitor cable.

Full analysis, alternatives (evdi, vkms, wlroots, X11), citations and
decisions: **`docs/`** — start with `docs/00-feasibility.md`.

## Repository layout

```
usbra/
├── host/        Rust binary: usbra-host (frame sources, TCP server, stats)
├── protocol/    Rust crate: the wire protocol (single source of truth)
├── android/     Kotlin app: protocol reader + GLES renderer
├── docs/        feasibility, architecture, comparisons, protocol spec, …
├── scripts/     adb tunnel, demo runner, GNOME display inventory
├── benchmarks/  methodology + stats summarizer (+ results, M8)
├── tests/       what's tested where + manual acceptance checklists
├── LICENSE      MIT
└── README.md
```

## Non-goals (for now)

Wi-Fi transport (USB-first by principle), touch input back-channel (planned
M11 via mutter's RemoteDesktop D-Bus — Chrome Remote Desktop proves the path),
hardware video codecs (reserved in the protocol; added when M8 shows the need).

## License

MIT — see `LICENSE`. The optional evdi fallback backend relies on DisplayLink's
separately-licensed (GPL-2 kernel module / LGPL-2.1 library) software and is
never bundled with USBra.
