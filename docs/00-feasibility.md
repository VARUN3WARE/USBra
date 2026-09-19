# USBra — Technical Feasibility Analysis

Status: **feasible today on stock Ubuntu (GNOME Wayland), with no kernel modules and no root**, for the virtual-display part; USB transport is feasible root-free via `adb reverse` (the scrcpy-proven path). This document is the evidence.

## 1. What "a real second display" means on Linux

A virtual display is *real* (as opposed to a capture/mirror hack) when the
compositor allocates it a logical monitor in its monitor layout. On GNOME that
means it shows up in `org.gnome.Mutter.DisplayConfig` (Settings → Displays),
windows can be dragged/maximized onto it, and the primary monitor keeps working
independently. That requires one of:

1. The compositor natively creating a virtual output (mutter can), or
2. A second DRM device appearing on the system whose connector reports a
   connected status + EDID (evdi/vkms style) — compositors treat it as a
   multi-GPU secondary display, exactly like DisplayLink USB docks.

Everything else (capturing the primary desktop and streaming it) is mirroring,
which the brief explicitly rejects.

## 2. The display stack, briefly

```
apps → compositor (mutter on GNOME Wayland)
        ├─ renders each monitor with the full GPU pipeline
        └─ presents via DRM/KMS to each card's connectors
kernel → DRM devices (card0 = real GPU, cardN = virtual devices)
```

Compositors, not applications, own monitor management. So USBra must convince
**the compositor** to add an output, then extract that output's frames with
damage information, then transport them.

## 3. Key research findings (with sources)

### 3.1 Mutter has a public virtual-monitor API since GNOME 40

- Mutter MR [!1698 "Headless native backend and virtual monitors"](https://gitlab.gnome.org/GNOME/mutter/-/merge_requests/1698)
  (Jonas Ådahl, Feb 2021) added:
  - `org.gnome.Mutter.ScreenCast.Session.RecordVirtual()` — creates a virtual
    monitor in the **running session** and returns a PipeWire stream of it.
    "Virtual monitors are not tied to the headless backend" — they work in a
    normal logged-in GNOME session.
  - A `--virtual-monitor WxH` gnome-shell flag (debug/nested use).
- The D-Bus interface ([XML](https://github.com/jadahl/gnome-remote-desktop/blob/master/src/org.gnome.Mutter.ScreenCast.xml)):
  `RecordVirtual(properties a{sv}) → stream_path o` with properties:
  - `"cursor-mode" (u)`: 0 hidden / 1 embedded / 2 metadata
  - `"is-platform" (b)` (API v3): *"Whether this virtual virtual output should
    be considered part of the platform, meaning it will not be interpreted as
    if the screen is shared, but more transparently as if it was a real
    monitor."* — exactly USBra's semantics.
- Exact usage flow is demonstrated in GTK's own test suite
  ([headless-monitor-tests.py](https://github.com/GNOME/gtk/blob/8873db5d/testsuite/headless/headless-monitor-tests.py)):
  `CreateSession({}) → RecordVirtual({}) → wait for PipeWireStreamAdded(node_id) → Start()`,
  on bus name `org.gnome.Mutter.ScreenCast`, path `/org/gnome/Mutter/ScreenCast`.
  **The virtual monitor's size is negotiated via the PipeWire stream format**
  (client proposes WxH), later refined by GNOME 50 (see below).
- **Chrome Remote Desktop does exactly this on Linux**: the Chromium host calls
  `org.gnome.Mutter.RemoteDesktop.CreateSession`, then `RecordVirtual` (with
  `is-platform`), subscribes to `PipeWireStreamAdded`, and dynamically adds
  more virtual monitors per client request
  ([xdg-desktop-portal discussion #1820](https://github.com/flatpak/xdg-desktop-portal/discussions/1820)).
  This is a production precedent for USBra's host architecture.
- GNOME 49 (Sep 2025, Ubuntu 25.10) shipped RDP "extend desktop with a virtual
  monitor" in the **user session** — g-r-d MR
  [!334](https://gitlab.gnome.org/GNOME/gnome-remote-desktop/-/merge_requests/334),
  [GNOME 49 release notes](https://release.gnome.org/49/).
- GNOME 50 (Mar 2026, Ubuntu 26.04 LTS) added virtual-monitor *mode lists* and
  HiDPI scale to `RecordVirtual()` plus an `org.gnome.scale` PipeWire tag
  ([Phoronix](https://www.phoronix.com/news/GNOME-50-Remote-Desktop-HiDPI),
  [mutter MR !4727](https://gitlab.gnome.org/GNOME/mutter/-/merge_requests/4727)).

**Consequence:** on Ubuntu 22.04 / 24.04 / 26.04 (GNOME 42+), USBra can create
Display 2 with zero kernel modules, using the same machinery GNOME's own remote
desktop uses. This is the primary architecture.

### 3.2 Mutter's PipeWire screencast streams are damage-tracked

- Mutter attaches `SPA_META_VideoDamage` regions to screencast stream buffers
  and maintains accumulated damage between captured frames
  ([mutter commit 7ce731f](https://github.com/GNOME/mutter/commit/7ce731f8010d2723693cc845fb1f5a0577813fe1),
  [commit 4dc0b52](https://github.com/GNOME/mutter/commit/4dc0b52d19ba7df31cce43d8bde61255eaf9c4dd)).
  mutter's own DevKit client consumes it; TigerVNC's `w0vncserver` consumes the
  same metadata from PipeWire
  ([tigervnc commit ca55a89](https://github.com/CendioOssman/tigervnc/commit/ca55a89a13731e2e1d016c3742d56a5424417729)),
  and `xdg-desktop-portal-wlr` produces it — it is the standard damage channel.
- Buffers are dma-buf or memfd (mmap-able) — mutter MR
  [!1086](https://gitlab.gnome.org/GNOME/mutter/-/merge_requests/1086) era work.
- If a target mutter lacks damage metadata, the fallback is "frame only on
  damage event, full-frame content" (mutter records when redraw happens), which
  still preserves frame pacing; M1/M6 probes detect which mode we're in.

### 3.3 evdi — the DisplayLink model (fallback backend, all compositors)

- evdi is DisplayLink's open-source kernel module + LGPL-2.1 `libevdi`
  ([GitHub](https://github.com/DisplayLink/evdi),
  [docs](https://displaylink.github.io/evdi/)). A userspace daemon:
  `evdi_add_device()` → new `/dev/dri/cardX` → `evdi_open` → `evdi_connect(edid)`
  (hot-plugs a "monitor" with our own EDID) → register buffers →
  `evdi_request_update()`/`evdi_handle_events()` → `evdi_grab_pixels()` returns
  **dirty rectangles**. Cursor events (`cursor_set`/`cursor_move`) exist too.
- Kernel support: "Minimum supported kernel 4.15 … verified … up to 6.15"
  ([README](https://github.com/DisplayLink/evdi/blob/main/README.md)) — covers
  Ubuntu 24.04's 6.8.
- GNOME Wayland treats evdi cards as secondary GPUs (the DisplayLink dock path),
  so Display 2 behaves exactly like a USB dock monitor.
- Costs: out-of-tree DKMS module (build per kernel, can break on updates);
  module config needed (`initial_device_count`, `softdep`) per
  [module README](https://github.com/DisplayLink/evdi/blob/main/module/README.md).
  Devices can also be added via `echo 1 > /sys/devices/evdi/add`.

### 3.4 vkms — in-tree, but a display-only dead end (standalone)

- vkms is a software KMS device for testing/headless
  ([kernel docs](https://docs.kernel.org/gpu/vkms.html)). Compositors *do* pick
  up its output as a display (verified on KDE:
  [vkms+krfb experiment](https://discuss.kde.org/t/experiment-virtual-kms-vkms-krfb/44999)),
  but **only the DRM master (the compositor) can touch its buffers** — a third
  process has no userspace data channel, so frames cannot be extracted without
  a separate screencast path. Runtime EDID/hotplug config is still on the
  kernel TODO list.
- Emerging tooling (fauxput, [GitHub](https://github.com/Pesc0/fauxput)) builds
  on a *patched* vkms with configfs EDID support and requires kernel ≥ 7.0 —
  not stock Ubuntu today.

### 3.5 wlroots/Sway — trivial virtual outputs, compositor-specific

- `swaymsg create_output` creates a headless output via
  `wlr_headless_add_output()` on any running Sway session
  ([sway source](https://github.com/swaywm/sway/blob/master/sway/commands/create_output.c));
  named outputs + `destroy_output` landed in Sway ~1.10
  ([PR #8381](https://github.com/swaywm/sway/pull/8381)).
- Frames + damage available via `wlr-screencopy` / PipeWire portal. A clean
  backend for Sway users later; not GNOME.

### 3.6 X11 and boot-time EDID emulation — legacy/awkward

- X11 virtual outputs exist only with the old `intel` driver (`VirtualHeads`);
  Ubuntu's default Xorg stack (modesetting/amdgpu) has none. GNOME-on-X11 is
  legacy.
- `drm.edid_firmware=... video=DP-3:WxH@60e` kernel args force a *physical*
  port to appear connected at boot ([worked example](https://discuss.kde.org/t/how-to-create-a-virtual-monitor-display/2725))
  — static, requires reboot, occupies a real connector. Not suitable.

### 3.7 USB transport without custom drivers

- **`adb reverse tcp:P tcp:P`** makes the phone able to reach a host-side
  localhost TCP port over the USB cable, with no root on either side and no
  custom drivers (userspace adb server + adbd). scrcpy proves the latency/bandwidth
  class over this exact transport: 35–70 ms end-to-end *including* video
  encode/decode, 30–120 fps, USB debugging only
  ([scrcpy README](https://github.com/Genymobile/scrcpy)).
- USB 2.0 bulk practical throughput ≈ 30–40 MB/s (≈ enough for ~60 fps of
  damage-driven desktop updates at 1080p; full raw 1080p60 = 373 MB/s is *not*
  achievable on USB 2 — hence damage + compression are mandatory design
  elements; see `docs/07-benchmarking.md` for measured numbers on real HW).
- USB tethering (RNDIS/CDC-NCM) gives a native `usb0` interface and can be
  enabled from adb on many builds (`svc usb setFunctions rndis`, no root on
  Android 11+; older builds vary — [AOSP `UsbCommand.java`](https://android.googlesource.com/platform/frameworks/base/+/master/cmds/svc/src/com/android/commands/svc/UsbCommand.java),
  [Stack Exchange](https://android.stackexchange.com/questions/29954/can-i-change-some-android-settings-from-the-command-line)).
  Vendor quirks exist (e.g. Nothing phones need a bridge jar —
  [NCM gist](https://gist.github.com/thewh1teagle/bdd3faf8990d1aa15d39bb92a7c9ac2e)).
  Kept as an alternative; `adb reverse` is the MVP (zero device config).
- Direct USB (Android Open Accessory + libusb) is the post-MVP transport: no
  "USB debugging" required at all, host talks bulk endpoints directly.

### 3.8 Android rendering

A GLES texture + `glTexSubImage2D` per damage rect is the entire render path
for MVP; it is well within budget (a 1080p full-frame upload is ~2 ms on
mid-range SoCs, partial rects far less). Hardware video decode (MediaCodec) is
a later add for high-motion content, and the protocol reserves codec IDs for
it. Vulkan/AHardwareBuffer/EGLImage are optimizations, not requirements.

## 4. Feasibility verdict

| Requirement | Verdict | Mechanism |
|---|---|---|
| Real Display 2 on Ubuntu GNOME | ✅ stock, no root, no modules | mutter `RecordVirtual` (GNOME 40+), `is-platform=true` |
| Frames + damage from that display | ✅ | PipeWire stream + `SPA_META_VideoDamage`, memfd/dma-buf |
| Fallback for other compositors | ✅ | evdi (GNOME/KDE/wlroots), Sway `create_output` |
| USB transport, no Wi-Fi, no root | ✅ | `adb reverse` (scrcpy-class), later AOAP |
| 60 FPS desktop workloads | ✅ plausible | damage-only updates; full frames only on resync |
| Clean unplug/replug | ✅ natural | D-Bus session lifetime = monitor lifetime; reconnect via protocol |

No blocking unknowns remain for the MVP. The genuine risks (mutter API
stability, damage metadata availability per version, USB2 bandwidth under
full-screen motion, adbd variance) are tracked in `docs/08-risks.md` with
mitigations.
