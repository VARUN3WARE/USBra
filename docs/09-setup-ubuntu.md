# USBra — Ubuntu Host Setup & Run (deliverable #10)

Tested against: Ubuntu 24.04 LTS (GNOME 46, kernel 6.8) — also expected to
work on 22.04 (GNOME 42+) and 26.04 (GNOME 49/50).

## 1. Build the host (zero third-party crates)

```bash
sudo apt install -y cargo adb        # or use rustup for a current toolchain
cd usbra/                            # repo root
cargo build --release                # builds protocol + host
# binary at target/release/usbra-host
```

## 2. Sanity checks (no phone needed)

```bash
# M2: measure frame-production latency of the test-pattern source
./target/release/usbra-host --selftest

# What does GNOME currently see? (M1 inventory tool)
scripts/m1-verify-displays.sh
```

`--selftest` prints generate+encode percentiles and payload sizes; expected
p95 well under 2 ms for damage frames at 1600×900 on any recent x86 core.

## 3. Run the demo (host + USB + phone)

Terminal 1 — USB tunnel keeper:

```bash
scripts/adb-usb-setup.sh             # waits for device, installs reverse,
                                     # re-installs automatically on replug
```

Terminal 2 — host:

```bash
scripts/run-demo.sh                  # = usbra-host --source test (8899)
# explicit:
./target/release/usbra-host --source test --port 8899 --width 1600 --height 900 \
    --fps 60 --stats /tmp/usbra.jsonl --frame-ack
```

Then launch **USBra** on the phone (see `docs/10-setup-android.md`). You
should see the animated test pattern within a second, with the stats overlay
counting fps/RTT.

### Real Display 2 (M6, GNOME Wayland)

```bash
sudo apt install -y python3-gi gstreamer1.0-tools gstreamer1.0-pipewire

# Prove mutter can create Display 2 (no phone):
cargo run --features gnome -- --probe-gnome --hold 20
# or: scripts/m6-probe-virtual-monitor.py --hold 20

# Full path — phone renders the virtual monitor:
cargo run --release --features gnome -- \
  --source gnome --port 8899 --width 1600 --height 900 \
  --fps 60 --stats /tmp/usbra.jsonl --frame-ack
```

On connect, GNOME Settings → Displays should show a second monitor. Drag a
window onto it; the phone should show that desktop region. Unplug USB (or
quit the host) and Display 2 disappears.

Other useful flags: `--full-frame-every 0` (damage-only on test source), `--fps 30`,
`--bind 127.0.0.1` (default; never expose this to the network).

## 4. Benchmarking

See `docs/07-benchmarking.md` — then:

```bash
python3 benchmarks/summarize_stats.py /tmp/usbra.jsonl
```

## 5. M1 verification once M6 lands (what "Display 2 exists" looks like)

```bash
scripts/m1-verify-displays.sh
# While usbra-host's GNOME backend session is ACTIVE you should see a second
# logical monitor (e.g. connector/virtual output listed beyond your eDP-1 /
# DP-1). GNOME Settings → Displays will show it and let you arrange it.
```

## 6. Optional: evdi fallback backend prerequisites (M6b)

Only needed for non-GNOME compositors or GNOME < 40 — **not required for the
MVP path**:

```bash
sudo apt install -y build-essential cmake dkms linux-headers-$(uname -r) libdrm-dev
git clone https://github.com/DisplayLink/evdi.git /tmp/evdi && cd /tmp/evdi
cmake . && make && sudo make install && sudo depmod
sudo modprobe evdi
# /sys/devices/evdi/add creates card nodes; our host (or DisplayLink's manager)
# then "plugs in" the virtual monitor via evdi_connect(EDID).
```

Module options for compositor friendliness are documented upstream
(`/etc/modprobe.d/evdi.conf`: `options evdi initial_device_count=1`,
`softdep evdi pre: drm_kms_helper i915`).

## 7. Troubleshooting

| Symptom | Fix |
|---|---|
| `adb devices` shows `unauthorized` | Unlock phone, accept the RSA dialog |
| App says "reconnecting…" forever | Check `adb reverse --list` shows `tcp:8899`; re-run `scripts/adb-usb-setup.sh` |
| "connection refused" on phone | Host not running / wrong port; host binds 127.0.0.1 — that's correct for adb reverse |
| Colors wrong on phone (red/blue swapped) | That's a pixel-format bug — file an issue with the overlay's format line |
| Choppy during full-screen motion | Expected on USB 2 raw; M7 adds zstd; try `--fps 30` meanwhile |
| `cargo` too old | Install rustup: `curl https://sh.rustup.rs | sh` |
