# USBra — Known Technical Risks

Deliverable #13. Each risk: likelihood × impact, mitigation, owner milestone.

## Display side

1. **mutter `ScreenCast` API drift** (it's a mutter API, not a freedesktop
   standard; properties/behaviors shift across GNOME versions — e.g. GNOME 50
   reworked virtual-monitor sizing). L:medium, I:medium. **Mitigation:** probe
   API version at connect; pin behavior per version; the API has been stable
   in shape since GNOME 40 and is used by GNOME's own remote desktop + Chrome
   Remote Desktop, so breakage would be loudly upstreamed. (M6, M9)
2. **`RecordVirtual` monitor ergonomics on target GNOME** — e.g. it may appear
   as a non-arrangeable "shared" display if `is-platform` is unsupported on an
   old API version, or scale oddly. L:low, I:high. **Mitigation:** M1/M6 probe
   script + acceptance test on 24.04 before building further; evdi backend as
   the escape hatch. (M6)
3. **`SPA_META_VideoDamage` availability** — damage metadata is verified in
   current mutter but older builds may only send full frames on damage events.
   L:medium, I:medium (bandwidth, not correctness). **Mitigation:** detect
   metadata at stream setup; fall back to full-frame-on-change; measure; if
   needed, host-side rect diffing on a dirty-page-hint budget. (M7)
4. **evdi kernel-module breakage** (out-of-tree vs new kernels). L:medium,
   I:low (fallback-only path). **Mitigation:** evdi is not the primary path;
   pin known-good releases per kernel; DKMS makes rebuilds automatic when
   headers are present. (M6b)
5. **Fractional scaling / HiDPI mismatch** between virtual monitor scale and
   phone panel. L:medium, I:low. **Mitigation:** GNOME 50's `org.gnome.scale`
   tag + mode lists; client letterboxes correctly regardless. (M9)
6. **Cursor handling** — hardware cursor is a separate plane; with
   `cursor-mode=embedded` mutter burns it into frames (slightly more damage on
   pointer moves, zero protocol work). L:low, I:low. **Mitigation:** embedded
   for MVP; `cursor-mode=metadata` + overlay sprite later (protocol reserves
   DAMAGE_REGION + capability bit). (M7/M9)

## Transport side

7. **USB 2 bandwidth ceiling under full-screen motion** (8.3 MB/frame raw at
   1080p). L:certain, I:medium. **Mitigation:** damage-only; zstd on large
   payloads; adaptive fps when congestion detected (send time growth); USB 3
   phones are already common. (M7, M8)
8. **adbd throughput/latency variance across vendors** — some phone adbd
   builds throttle or add jitter. L:medium, I:medium. **Mitigation:** M8
   measures per-device; document "known good" devices; AOAP/libusb path (M11)
   removes adbd entirely where needed.
9. **adb UX (USB debugging + RSA authorization)** — friction for non-dev
   users; corporate policies sometimes disable adb. L:certain, I:low (MVP).
   **Mitigation:** excellent setup docs; AOAP transport removes the
   requirement (M11).
10. **Tunnel robustness** — `adb reverse` mappings can vanish on phone
    reboot/USB re-enumeration. L:medium, I:low. **Mitigation:**
    `scripts/adb-usb-setup.sh` re-establishes automatically; client reconnect
    loop with backoff. (M3 ✅, M9)

## Client side

11. **JVM buffer copies on the hot path** — `ByteArray` → direct `ByteBuffer`
    per rect is one copy too many at high bandwidth. L:medium, I:low.
    **Mitigation:** pooled direct buffers reading the socket directly (parse
    into native memory); AHardwareBuffer + EGLImage later. (M8 → optimize if
    it shows)
12. **GLSurfaceView vsync coupling** — RENDERMODE_WHEN_DIRTY + requestRender
    coalesces correctly, but pathological frame bursts could queue > 1 render.
    L:low, I:low. **Mitigation:** bounded queue drops to latest + periodic
    full-frame resync (protocol guarantee). (M5 ✅ design)
13. **Phone thermal/battery** — sustained 60 fps uploads on mid-range phones.
    L:medium, I:low. **Mitigation:** damage-driven idle ≈ 0 cost; measure
    thermals in M8; fps cap config.

## Correctness

14. **Pixel-format byte-order bugs** (BGRX vs RGBX) — classic off-by-swizzle.
    L:medium, I:low (cosmetic). **Mitigation:** format is explicit per
    message; shader swizzle switch; test pattern includes pure R/G/B areas to
    catch it instantly. (M5)
15. **Clock skew in latency math** — solved by design: all latency math is
    host-clock (FRAME_ACK) or client-clock (RTT), never mixed. (✅)
16. **TCP head-of-line blocking** on lossless USB link — negligible over adb
    (no real packet loss; drops present as adbd stalls). AOAP framing (M11)
    would revisit.

## Security posture

17. Host binds **127.0.0.1 only**; nothing reachable from the network. adb's
    trust model (RSA-authorized debugging) is the only exposure and is
    user-controlled; no cloud, no telemetry, no accounts by construction.
    Post-MVP: AOAP transport drops adb from the trust chain.
