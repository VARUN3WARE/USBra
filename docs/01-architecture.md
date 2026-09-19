# USBra — Recommended Architecture

## 1. System diagram

```
┌────────────────────────── Ubuntu PC (GNOME Wayland) ──────────────────────────┐
│                                                                               │
│  apps (windows)                                                               │
│      │                                                                        │
│  mutter ──── Monitor 1 (physical, eDP/HDMI/DP on card0)                       │
│      │                                                                        │
│      └──── Monitor 2  ←── created via org.gnome.Mutter.ScreenCast             │
│              │                .Session.RecordVirtual(is-platform=true)        │
│              │                (falls back to evdi card on other compositors)  │
│              ▼                                                                │
│  PipeWire stream (memfd/dma-buf + SPA_META_VideoDamage)                       │
│              │                                                                │
│  usbra-host (Rust)                                                            │
│    ├─ source backends: TestPattern | GnomeScreenCast | Evdi (planned)          │
│    ├─ damage tracker & rect packer                                            │
│    ├─ optional zstd (per-frame, UI content compresses ~3–10×)                 │
│    └─ TCP server on 127.0.0.1 (loopback only)                                 │
│              │                                                                │
│  adb server ── adb reverse tcp:8899 tcp:8899  (userspace USB bulk)            │
└──────────────┼────────────────────────────────────────────────────────────────┘
               │ USB cable (no Wi-Fi, no drivers, no root)
┌──────────────▼─────────── Android phone ──────────────────────────────────────┐
│  adbd ── local TCP 127.0.0.1:8899                                              │
│  usbra-android (Kotlin)                                                        │
│    ├─ protocol reader (byte-exact mirror of protocol/ crate)                   │
│    ├─ GLSurfaceView + GLES: texture upload per damage rect, letterboxed draw   │
│    ├─ PING/PONG RTT + FRAME_ACK latency instrumentation                        │
│    └─ auto-reconnect (RECONNECT carries session_id + last_frame_id)            │
└────────────────────────────────────────────────────────────────────────────────┘
```

## 2. Component responsibilities

### `usbra-host` (Rust, `host/`)

- **Source backends** behind one interface (`FrameSource`):
  - `TestPattern` — synthetic animated desktop with real damage rects. Lets us
    build and measure transport/rendering *before* any compositor integration.
    Ships today.
  - `GnomeScreenCast` (M6) — D-Bus `org.gnome.Mutter.ScreenCast`:
    `CreateSession` → `RecordVirtual(cursor-mode=embedded, is-platform=true)`
    → `PipeWireStreamAdded(node_id)` → `session.Start()`; consume the PipeWire
    stream, read `SPA_META_VideoDamage`, emit `FRAME` messages. Virtual monitor
    size chosen by our proposed PipeWire format (WxH from the phone).
  - `Evdi` (M6b, optional backend) — `evdi_add_device` + custom EDID +
    `evdi_grab_pixels` dirty rects; works on GNOME/KDE/wlroots, costs a DKMS
    module. `SwayHeadless` (`swaymsg create_output` + screencopy) later.
- **Protocol server** — loopback-only TCP, `TCP_NODELAY`, one writer thread,
  one reader thread per connection; producer thread paced to the display's
  refresh. Handles HELLO/RECONNECT handshake, PING→PONG, FRAME_ACK stats.
- **Stats** — JSONL event log (`--stats file`) + 1 Hz stderr summary; the
  benchmarking harness consumes this (`benchmarks/`).

### `usbra-protocol` (Rust crate, `protocol/`)

- The single source of truth for the wire format (see `docs/04-protocol.md`).
  No I/O opinions, no_std-friendly layout, exhaustively roundtrip-tested.
  The Kotlin reader in the Android app is a byte-exact manual mirror (kept
  small enough that this is safe; codegen is a later option if it grows).

### `usbra-android` (Kotlin, `android/`)

- Socket reader thread → frame queue (bounded, latest-wins with periodic
  full-frame resync as the safety net) → GL thread uploads damage rects into a
  persistent texture via `glTexSubImage2D`, draws a letterboxed fullscreen quad.
- No video codec in MVP. `MediaCodec` (H.264/AV1) is additive later: the
  protocol's FRAME message already carries codec/pixel-format fields.

## 3. Control & lifecycle (the part that makes it feel like a dock)

```
USB plugged → adb sees device → scripts/adb-usb-setup.sh installs reverse tunnel
phone app starts → connects 127.0.0.1:8899 → HELLO (reports its panel size)
host: HELLO_OK(session_id) + DISPLAY_INFO + CONFIG
host: creates/attaches virtual display sized to the phone (M6: RecordVirtual)
GNOME: Display 2 appears; user arranges it in Settings → Displays
frames flow; PING/PONG keeps RTT visible; FRAME_ACK keeps latency visible
USB unplugged → socket EOF → host drops the ScreenCast session
           → mutter removes Display 2 automatically (clean removal, criterion 6)
           → windows migrate back to Monitor 1
USB replugged → tunnel re-established by script → app sends
           RECONNECT(session_id, last_frame_id) → host resyncs with a full frame
```

The virtual monitor's lifetime being tied to the streaming session is a
feature: it is exactly the semantics of a physical cable.

## 4. Data flow of one frame (M6, GNOME backend)

```
mutter paints Monitor 2 (damage Δ)
 → mutter records into PipeWire buffer (GPU readback or dma-buf share), attaches Δ
 → usbra-host dequeues, mmaps, extracts Δ rects (≤ 64/frame, merged)
 → pack rect pixels (rows, tightly packed) → FRAME{frame_id, ts, codec, rects, payload}
 → TCP loopback → adb server → USB bulk → adbd → app socket
 → GL thread: glTexSubImage2D per rect → next vsync presents
```

Latency budget at 1080p (to be measured, `docs/07-benchmarking.md`):
mutter record ~1–3 ms; mmap+extract ~0.3 ms; loopback+adb+USB ~1–4 ms;
app parse+upload ~0.5–2 ms; vsync ≤ 16.7 ms → **≈ 20–40 ms** typical,
comparable to scrcpy's 35–70 ms *without* paying video encode/decode on static
content.

## 5. Why this satisfies the core principles

1. **USB-first** — adb reverse rides USB bulk; no network stack beyond
   loopback. 2. **Low latency** — damage-only transmission, no per-frame
   encode, TCP_NODELAY, bounded queues. 3. **Minimal CPU** — one mmap + one
   memcpy of dirty rows on the hot path (host), one texture upload (phone);
   no pixel format conversions (we ship the compositor's native XRGB8888).
4–7. **No cloud/account/telemetry/subscription** — host binds 127.0.0.1 only;
   nothing leaves the machine. 8. **Open source** — MIT (this repo); evdi
   backend (GPL-2 kernel module + LGPL-2.1 lib) loaded, not linked, at the
   user's option. 9. **Native** — Rust + Kotlin, no Electron. 10. **No
   unnecessary encoding** — raw pixels + damage; zstd only when it pays.
11. **Damage regions** — first-class in protocol and both implementations.
12. **GPU where it matters** — mutter's full GPU pipeline renders Monitor 2;
   phone GPU presents; hot paths avoid CPU pixel churn. 13/14. **Android +
   Ubuntu first** — all APIs chosen are Ubuntu/GNOME-native.

## 6. Compositor strategy (explicit)

- **Ubuntu GNOME (default target):** mutter-native `RecordVirtual`. No kernel
  modules. Compositor-specific — stated clearly — but it is the *primary*
  platform per the brief.
- **Other compositors:** the host's source-backend interface isolates the
  compositor-specific code. evdi backend covers GNOME<40/X11/KDE/wlroots;
  Sway gets a `create_output` backend. Same protocol, same app, same USB.
