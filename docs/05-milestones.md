# USBra — Milestones

Deliverable #7. Every milestone ends with something runnable and a yes/no
acceptance test. Status column is current as of this commit.

## M0 — Repo scaffold + protocol + test pattern + transport + renderer  ✅ (this commit)

What: monorepo, `usbra-protocol` crate (full v1 + tests), `usbra-host` with
TestPattern source/selftest/TCP server, Kotlin client with GLES renderer and
reconnect, adb scripts, benchmark harness skeleton.
Acceptance: `cargo test` green (protocol roundtrip + host↔client loopback
integration test); host selftest prints frame-production percentiles.
This deliberately front-loads Milestones 2–5 of the brief using the test
source so transport/rendering are proven *before* compositor integration.

## M1 — Prove GNOME can show a virtual Display 2  📋 ready to run on target HW

What: `scripts/m1-verify-displays.sh` inventories what GNOME currently sees
(`org.gnome.Mutter.DisplayConfig.GetResources`). The decisive run happens in
M6 when `usbra-host` itself calls `RecordVirtual`; the API's existence and
user-session semantics are already source-verified (see `docs/00-feasibility.md`).
Acceptance: with the host session active, GetResources lists a second monitor;
Settings → Displays shows it; it can be arranged. Evdi probe (optional M6b
path) documented in `docs/09-setup-ubuntu.md`.

## M2 — Render the virtual display locally, measure production latency  ✅

What: `usbra-host --selftest` generates N frames with true damage rects,
measures generate+encode time percentiles, bytes, effective fps — no network,
no phone needed.
Acceptance: p95 frame production < 2 ms at 1600×900 (damage frames).

## M3 — USB link to Android  ✅ (code ready; needs on-device run)

What: `scripts/adb-usb-setup.sh` + client connect/HELO/PING/PONG/reconnect.
Acceptance: phone connects via USB within ~1 s of plug; `adb reverse --list`
shows the tunnel; unplug/replug auto-reconnects (client logs).

## M4 — Transmit a framebuffer over USB  ✅ (code ready)

What: FRAME messages with damage rects from TestPattern source.
Acceptance: client-side frame counter increments at ~60 fps with typical
damage payloads < 400 KB/frame; host JSONL stats show send_us p95 < 2 ms.

## M5 — Render efficiently on Android  ✅ (code ready)

What: GLSurfaceView + GLES2/3, per-rect `glTexSubImage2D`, shader swizzle for
BGRX/RGBX, letterbox, stats overlay.
Acceptance: smooth animated pattern on the phone; renderer keeps up at 60 fps
(drain queue stays ≤ 2 deep); `dumpsys gfxinfo` shows no jank frames > 16 ms
in steady state.

## M6 — Real virtual display: mutter `RecordVirtual` backend  🔜 next

What: host gains `GnomeScreenCast` source: zbus D-Bus
(`CreateSession`/`RecordVirtual(cursor-mode=embedded, is-platform=true)`/
`PipeWireStreamAdded`/`Start`), PipeWire stream consumer (memfd buffers,
negotiate WxH from the phone), damage extraction from `SPA_META_VideoDamage`,
frames → existing pipeline. Auto teardown on USB unplug (session lifetime).
Steps: (a) D-Bus probe tool printing API version + created monitor; (b) raw
PipeWire consumer dumping frames to PPM for eyeball check; (c) wire into
server; (d) drag-a-window test.
Acceptance (the brief's MVP list): phone connects over USB; Ubuntu shows
Display 2; Android renders it; a window dragged from Monitor 1 lands on the
phone; Monitor 1 unaffected; USB unplug removes Display 2 within ~1 s; replug
restores it; 60 FPS for normal desktop work; no Wi-Fi at any point.

## M7 — Damage-region updates end-to-end + compression

What: verify GNOME backend emits partial rects (not full frames) on typical
desktop activity; add zstd codec (negotiated; applied when payload > threshold);
merge/clip rect lists; embedded-cursor verification.
Acceptance: editing text produces < 100 KB/s average; 60 fps sustained while
scrolling a page; full-screen video detection can degrade gracefully (M11
adds hardware codec).

## M8 — Benchmarks

What: run `docs/07-benchmarking.md` methodology on real hardware; record
results in `benchmarks/results/`; set default tuning from data.
Acceptance: report with latency p50/p95, fps, MB/s, host CPU %, phone CPU %.

## M9 — Reconnect, configuration, UX polish

What: robust RECONNECT (link flaps, host restart), config file + CLI
(resolution presets, fps cap, codec policy), Android settings screen, host
status CLI (`usbra-host status`), systemd user unit + adb udev triggering.

## M10 — Packaging

What: deb + optional PPA, prebuilt APK (or Play/F-Droid), `install.sh` that
sets up adb rules + udev + systemd unit; docs polish.

## M11+ — Future (explicit non-goals for MVP)

- Direct USB transport (AOAP/libusb) if adb overhead measurably matters.
- Touch/pen input back-channel (mutter `org.gnome.Mutter.RemoteDesktop` D-Bus
  or libei — Chrome Remote Desktop already does EIS this way on GNOME).
- Hardware video codec for high-motion content (codec IDs reserved).
- evdi backend (M6b) and Sway `create_output` backend for other compositors.
- Multi-virtual-monitor (protocol already has no single-display assumption).
