# USBra — Virtual Display Approaches: Comparison

Deliverable #3 of the project brief. Every approach was verified against
current sources (Sep 2026); citations inline.

## Scorecard

| Approach | Real monitor on stock Ubuntu GNOME? | Frame+damage access for a 3rd-party process | Kernel work | Compositor coverage | MVP role |
|---|---|---|---|---|---|
| **Mutter virtual monitor** (`RecordVirtual` D-Bus + PipeWire) | ✅ GNOME 40+ (Ubuntu 21.04+) | ✅ PipeWire stream, memfd/dma-buf, `SPA_META_VideoDamage` | none | GNOME/mutter (niri also implements the API) | **Primary** |
| **evdi** (DisplayLink virtual DRM card) | ✅ any KMS compositor (DisplayLink-dock path) | ✅ libevdi: mmap buffers + dirty rects + cursor events | DKMS out-of-tree module | GNOME, KDE, wlroots, X11 | **Fallback backend** (M6b) |
| **wlroots headless output** (`swaymsg create_output`) | ✅ in Sway (runtime) | ✅ wlr-screencopy / PipeWire portal | none | wlroots compositors only | Backend for Sway users (later) |
| **vkms** (in-tree virtual KMS) | ⚠️ appears as secondary-GPU display (verified on KDE; mutter similar) but fixed modes, no runtime EDID | ❌ none — only the DRM master (compositor) can touch buffers; needs a *separate* screencast path to extract frames | none (in-tree) | any KMS | Not viable standalone; CI/testing only |
| **PipeWire (alone)** | ❌ capture-only — cannot create displays | ✅ (it *is* the frame transport) | none | via portals | Forbidden as primary by brief; used only as the *extraction* half of the mutter path |
| **X11 VIRTUAL outputs** (intel `VirtualHeads`) | ⚠️ only with legacy intel ddx; not modesetting/amdgpu | XShm capture | none | X11 only | Legacy, rejected |
| **Boot-time EDID emulation** (`drm.edid_firmware`, `video=…e`) | ⚠️ occupies a *physical* connector, needs reboot, static | via screencast | none | any | Rejected (not hot-pluggable) |

## Per-approach analysis

### 1. Mutter virtual monitors — primary

**What:** `org.gnome.Mutter.ScreenCast` session method
`RecordVirtual(properties a{sv}) → stream_path` ([interface XML](https://github.com/jadahl/gnome-remote-desktop/blob/master/src/org.gnome.Mutter.ScreenCast.xml)),
landed via mutter [MR !1698](https://gitlab.gnome.org/GNOME/mutter/-/merge_requests/1698)
(GNOME 40, Apr 2021). The monitor exists in the live session — the MR states
virtual monitors are "not tied to the headless backend". Size is negotiated in
the PipeWire stream format. `is-platform=true` makes mutter treat it as a real
monitor rather than screen-sharing. GNOME 50 adds mode-lists + HiDPI scale
([Phoronix](https://www.phoronix.com/news/GNOME-50-Remote-Desktop-HiDPI)).

**Frames:** PipeWire stream (`PipeWireStreamAdded` signal → node id), buffers
memfd (mmap) or dma-buf; damage via `SPA_META_VideoDamage`
([mutter commit](https://github.com/GNOME/mutter/commit/7ce731f8010d2723693cc845fb1f5a0577813fe1)).
Consumed identically by mutter's DevKit, TigerVNC `w0vncserver`
([commit](https://github.com/CendioOssman/tigervnc/commit/ca55a89a13731e2e1d016c3742d56a5424417729)),
and Chrome Remote Desktop
([portal discussion](https://github.com/flatpak/xdg-desktop-portal/discussions/1820) —
their Linux host is the best-read production example of this exact stack).

**Strengths:** zero kernel modules, zero root, stock Ubuntu 22.04/24.04/26.04;
the same machinery GNOME 49's own "extend desktop with virtual monitor" RDP
feature uses ([g-r-d !334](https://gitlab.gnome.org/GNOME/gnome-remote-desktop/-/merge_requests/334),
[GNOME 49 notes](https://release.gnome.org/49/)); monitor lifetime tied to our
session (clean unplug); official API, actively invested in.

**Weaknesses:** mutter/GNOME-specific (fine for MVP target); `ScreenCast` is a
mutter API, not a freedesktop standard (XDG portal work is ongoing but doesn't
yet allow dynamic virtual monitors); needs PipeWire + D-Bus client code in the
host; exact property/behavior details vary slightly across versions → feature
detection in M6.

### 2. evdi — fallback backend, best cross-compositor coverage

**What:** GPL-2 kernel module creating virtual DRM cards + LGPL-2.1 `libevdi`
userspace library ([GitHub](https://github.com/DisplayLink/evdi),
[API docs](https://displaylink.github.io/evdi/)). Flow: `evdi_add_device()` →
`evdi_open()` → `evdi_connect(edid,…)` (hotplug; our EDID names the monitor
"USBra <phone>") → `evdi_register_buffer()` → event loop on
`evdi_get_event_ready()` fd (`mode_changed`, `update_ready`, `dpms`,
`cursor_set`, `cursor_move`) → `evdi_request_update()` +
`evdi_grab_pixels(&rects)` returns dirty rects. The compositor renders into the
evdi card as a secondary GPU — precisely the DisplayLink dock path GNOME has
supported for years. Kernel support verified to 6.15
([README](https://github.com/DisplayLink/evdi/blob/main/README.md)); module
options required for compositor friendliness
([module README](https://github.com/DisplayLink/evdi/blob/main/module/README.md));
devices can also be added with `echo 1 > /sys/devices/evdi/add`. Nice detail:
`evdi_open_attached_to("usb:2-2.1")` links the virtual card to the phone's
USB device in sysfs.

**Strengths:** compositor-agnostic; damage + cursor events built-in; EDID
fully under our control; proven at scale (every DisplayLink dock).

**Weaknesses:** out-of-tree DKMS — install friction and kernel-update
breakage risk; double copy (compositor→dumb buffer, grab→our buffer); the
ecosystem's userspace manager (DisplayLink's) is proprietary — we write our
own manager role (libevdi is LGPL, or bind the ioctls directly).

### 3. wlroots headless outputs

`swaymsg create_output [name]` → `wlr_headless_add_output(backend, 1920, 1080)`
([source](https://github.com/swaywm/sway/blob/master/sway/commands/create_output.c)),
works on DRM sessions since the secondary-headless-backend PR
([sway #5216](https://github.com/swaywm/sway/pull/5216)); `destroy_output`
since ~1.10 ([#8381](https://github.com/swaywm/sway/pull/8381)). Extract frames
with wlr-screencopy (damage included) or the portal. The community pattern is
proven with wayvnc
([worked example](https://b.r0.at/posts/wayvnc_remote_desktop/)). A USBra
backend here is ~the mutter backend's structure with different plumbing.

### 4. vkms

In-tree software KMS ([kernel docs](https://docs.kernel.org/gpu/vkms.html)).
Confirmed to appear as a configurable display in a running session (KDE
experiment: [discuss.kde.org](https://discuss.kde.org/t/experiment-virtual-kms-vkms-krfb/44999)),
with caveats (no redraw pressure when it's the only device; needs
`KWIN_DRM_DEVICES` ordering). The killer problem: **no userspace frame/damage
channel** — vkms is a KMS *sink*; only the DRM master (= the compositor) can
access its buffers, and nothing notifies a third party of updates. Runtime
EDID/hotplug is still on the kernel TODO list. Emerging projects (fauxput,
[GitHub](https://github.com/Pesc0/fauxput)) require kernel ≥ 7.0 **plus a
patched vkms** with configfs EDID, then still capture via a portal — not stock
Ubuntu. Verdict: keep for CI/headless testing; not a USBra backend today.

### 5. PipeWire (alone)

PipeWire is a frame *transport*, not a display creator — it can only stream
outputs that already exist. Using it against the primary monitor is precisely
the mirroring the brief forbids. Its legitimate role is as the **extraction
half** of the mutter path (a virtual monitor is a real monitor, so
screencasting *it* is not mirroring). Documented here to close the loop.

### 6. X11 / boot-time EDID

X11 `VIRTUAL` outputs require the legacy intel ddx driver; Ubuntu's default
Xorg uses modesetting/amdgpu (no virtual outputs) and GNOME-on-X11 is legacy —
rejected. `drm.edid_firmware` + `video=<conn>:WxH@60e` boot args fake a
connector at boot
([example](https://discuss.kde.org/t/how-to-create-a-virtual-monitor-display/2725)) —
static, reboot-bound, occupies a physical port — rejected for a hot-pluggable
USB display.

## Decision

- **Ubuntu GNOME (the brief's first host): mutter `RecordVirtual`.** Smallest
  install, official API, damage-tracked, clean lifecycle, and a production
  precedent (Chrome Remote Desktop).
- **Cross-compositor reach: evdi backend (M6b)** for GNOME<40, KDE, X11, and
  anyone who wants the DisplayLink-style model.
- **Sway: `create_output` backend (later).**
- **vkms: testing only. X11/boot-EDID: rejected.**
