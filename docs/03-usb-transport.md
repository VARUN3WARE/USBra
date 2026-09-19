# USBra — USB Transport Strategy

Deliverable #4. Requirement: USB cable, no Wi-Fi, no custom kernel drivers,
no root, low latency, minimal overhead.

## Options considered

### 1. `adb reverse` TCP-over-USB — **chosen for MVP**

```
phone app ── TCP 127.0.0.1:8899 ── adbd (phone)
     ── USB bulk transfers ── adb server (PC) ── TCP 127.0.0.1:8899 ── usbra-host
```

- One command on the PC: `adb reverse tcp:8899 tcp:8899`. Connections the phone
  makes to its own `localhost:8899` are tunneled over the USB cable to the
  PC's port 8899. No root anywhere, no drivers, no device-side network config.
- Reliability/latency class is proven by scrcpy over the same transport:
  **35–70 ms end-to-end including H.264 encode+decode, 30–120 fps**
  ([scrcpy README](https://github.com/Genymobile/scrcpy)); scrcpy's maintainer
  notes USB 2.0 bandwidth is already sufficient for a compressed 1080p video
  stream ([issue #1537](https://github.com/Genymobile/scrcpy/issues/1537)).
  USBra's raw+damage traffic for desktop workloads is *smaller* than that
  (full raw 1080p60 = 373 MB/s is the one thing USB 2 cannot carry — hence
  damage regions + optional zstd are protocol-level requirements, not options).
- Costs: requires USB debugging enabled + RSA authorization (acceptable for
  MVP; the post-MVP AOAP path removes it); adbd adds one userspace hop
  (~0.5–2 ms + ~10% bandwidth overhead); `adb` daemon must run on the host.
- The tunnel survives phone reboots poorly — `scripts/adb-usb-setup.sh` keeps
  it re-established automatically.

### 2. USB tethering (RNDIS / CDC-NCM) — alternative, documented

- Android's USB tethering exposes a native `usb0` network interface on Linux
  (phone gateway 192.168.42.129 by default); we could run plain TCP/UDP over
  it with zero adb involvement on the data path.
- Can be enabled from the PC via `adb shell svc usb setFunctions rndis` (or
  `ncm` on gadget HAL ≥ 1.2) — no root on many Android 11+ builds
  ([AOSP `UsbCommand.java`](https://android.googlesource.com/platform/frameworks/base/+/master/cmds/svc/src/com/android/commands/svc/UsbCommand.java),
  [Android SE](https://android.stackexchange.com/questions/29954/can-i-change-some-android-settings-from-the-command-line));
  older builds and some vendors need root or behave oddly (Nothing phones need
  a bridge jar — [NCM tethering gist](https://gist.github.com/thewh1teagle/bdd3faf8990d1aa15d39bb92a7c9ac2e)).
- Verdict: same USB wire, similar throughput, *worse* automation story than
  adb reverse on stock phones. Supported later via a `--transport tether`
  option if measurement (M8) shows a meaningful win.

### 3. Android Open Accessory (AOAP) + libusb — **post-MVP upgrade path**

- Host-side libusb talks bulk endpoints directly to the phone after switching
  it into accessory mode; the app receives a `UsbAccessory` via `UsbManager`.
- **No USB debugging, no adb, no authorization dialogs** — the cleanest
  end-user UX, and removes the adbd hop (~1–2 ms, ~10% overhead).
- Costs: host must ship udev rules + libusb; accessory mode re-enumeration
  quirks per vendor; more code (raw USB framing instead of a kernel TCP stack).
- Planned as M11 — only if M8 measurements show adb is a measurable bottleneck
  or UX demands it. The protocol is transport-agnostic (length-prefixed
  messages over any reliable stream; a USB bulk framing layer can carry the
  same bytes).

### 4. Rejected

- **Wi-Fi / network transports** — excluded by project principles (debug-only
  fallback, off by default, never for the primary path).
- **usbip / gadget-side hacks** — phones don't expose usbip servers; CDC
  functionfs custom gadget requires root and breaks MTP/charging UX.
- **Bluetooth** — bandwidth class is hopeless for pixels.

## Bandwidth budget (design driver)

| Content at 1080p (1920×1080×4 B) | Per change | @60 fps | USB 2 (~35 MB/s) | USB 3 (~300 MB/s) |
|---|---|---|---|---|
| Full frame, raw | 8.3 MB | 498 MB/s | ❌ (≈4 fps) | ⚠️ borderline |
| Full frame, zstd (UI ~4–8×) | 1–2 MB | 60–120 MB/s | ⚠️ (30–60 fps) | ✅ |
| Typical desktop damage (1–5 %) | 80–400 KB | 5–24 MB/s | ✅ | ✅ |
| Window drag 800×600, raw | 1.9 MB | 115 MB/s | ❌ raw / ✅ zstd | ✅ |

Conclusions baked into the protocol: damage rects always; full frames only on
resync; zstd capability negotiated per-session (bit in HELLO caps, codec field
per FRAME); RGB565/future codecs reserved. M8 measures real numbers per phone
+ cable (harness in `benchmarks/`).

## Why TCP framing on top of adb is fine

`adb reverse` gives the phone a reliable, ordered byte stream. That lets the
protocol stay a simple binary message stream (no retransmission, no ordering
fields, no checksums) — all of which we'd have to reinvent on raw USB bulk
endpoints later. The AOAP upgrade keeps the same message bytes and adds a
length-framed bulk transport underneath.
