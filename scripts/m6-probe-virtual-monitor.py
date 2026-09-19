#!/usr/bin/env python3
"""M6a probe: create a real GNOME virtual Display 2 via mutter ScreenCast.

Flow (same as GTK's headless-monitor-tests.py and Chrome Remote Desktop):

  org.gnome.Mutter.ScreenCast
    CreateSession({})
      → Session.RecordVirtual({cursor-mode=1, is-platform=True})
      → Stream.PipeWireStreamAdded(node_id)
      → Session.Start()

A tiny GStreamer PipeWire consumer proposes WxH so mutter actually materializes
the monitor. Hold for --hold seconds, then Stop() — Display 2 disappears.

Requires: GNOME Wayland session, python3-gi, gstreamer1.0-tools,
          gstreamer1.0-pipewire (or pipewire GStreamer plugin).

Usage:
  scripts/m6-probe-virtual-monitor.py
  scripts/m6-probe-virtual-monitor.py --width 1080 --height 1920 --hold 30
  scripts/m6-probe-virtual-monitor.py --no-gst   # D-Bus only (may not show a monitor)

See docs/05-milestones.md § M6 and docs/00-feasibility.md § 3.1.
"""
from __future__ import annotations

import argparse
import shutil
import signal
import subprocess
import sys
import time

try:
    import gi

    gi.require_version("Gio", "2.0")
    gi.require_version("GLib", "2.0")
    from gi.repository import Gio, GLib
except (ImportError, ValueError) as e:
    print(f"error: need python3-gi (Gio/GLib): {e}", file=sys.stderr)
    print("  sudo apt install python3-gi", file=sys.stderr)
    sys.exit(1)

BUS_NAME = "org.gnome.Mutter.ScreenCast"
ROOT_PATH = "/org/gnome/Mutter/ScreenCast"
IFACE_ROOT = "org.gnome.Mutter.ScreenCast"
IFACE_SESSION = "org.gnome.Mutter.ScreenCast.Session"
IFACE_STREAM = "org.gnome.Mutter.ScreenCast.Stream"

# Cursor modes (mutter ScreenCast / portal): 0 hidden, 1 embedded, 2 metadata.
CURSOR_EMBEDDED = 1


def asv(**kwargs) -> dict:
    """Python dict of GLib.Variants suitable for a{sv} method args."""
    out = {}
    for k, val in kwargs.items():
        if isinstance(val, bool):
            out[k] = GLib.Variant("b", val)
        elif isinstance(val, int):
            out[k] = GLib.Variant("u", val)
        else:
            raise TypeError((k, val))
    return out


def call(proxy: Gio.DBusProxy, method: str, args: GLib.Variant | None, timeout_ms: int = 10000):
    return proxy.call_sync(method, args, Gio.DBusCallFlags.NONE, timeout_ms, None)


def list_connectors() -> list[str]:
    """Best-effort inventory via DisplayConfig.GetResources (same as m1-verify)."""
    try:
        bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        proxy = Gio.DBusProxy.new_sync(
            bus,
            Gio.DBusProxyFlags.NONE,
            None,
            "org.gnome.Mutter.DisplayConfig",
            "/org/gnome/Mutter/DisplayConfig",
            "org.gnome.Mutter.DisplayConfig",
            None,
        )
        result = call(proxy, "GetResources", None)
        # GetResources → (u serial, a(uxiiiiiuaua{sv}) crtcs, a(uxiiiiiuaua{sv}) outputs, ...)
        # Walk outputs' connector names from the packed tuple when possible.
        raw = result.unpack()
        connectors = []
        # outputs is typically index 2; each output tuple has connector at a known slot.
        # Format varies slightly by mutter version — fall back to string scan.
        text = str(raw)
        import re

        connectors = sorted(set(re.findall(r"'([A-Za-z0-9]+-[0-9]+)'", text)))
        return connectors
    except Exception as e:
        print(f"  (could not list DisplayConfig connectors: {e})")
        return []


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--width", type=int, default=1600)
    ap.add_argument("--height", type=int, default=900)
    ap.add_argument("--fps", type=int, default=60)
    ap.add_argument("--hold", type=float, default=20.0, help="seconds to keep Display 2 alive")
    ap.add_argument("--no-gst", action="store_true", help="skip GStreamer consumer (D-Bus only)")
    ap.add_argument("--no-platform", action="store_true", help="omit is-platform=true (share semantics)")
    args = ap.parse_args()

    print("USBra M6a — mutter RecordVirtual probe")
    print(f"  target size {args.width}x{args.height}@{args.fps}  hold={args.hold}s")

    before = list_connectors()
    print(f"  connectors before: {before or '(none listed)'}")

    try:
        bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    except GLib.Error as e:
        print(f"error: cannot connect to session bus: {e}", file=sys.stderr)
        return 1

    try:
        root = Gio.DBusProxy.new_sync(
            bus, Gio.DBusProxyFlags.NONE, None, BUS_NAME, ROOT_PATH, IFACE_ROOT, None
        )
    except GLib.Error as e:
        print(f"error: {BUS_NAME} not on the bus — need GNOME Wayland (GNOME 40+): {e}", file=sys.stderr)
        return 1

    # CreateSession({})
    session_path = call(root, "CreateSession", GLib.Variant("(a{sv})", ({},))).unpack()[0]
    print(f"  session: {session_path}")
    session = Gio.DBusProxy.new_sync(
        bus, Gio.DBusProxyFlags.NONE, None, BUS_NAME, session_path, IFACE_SESSION, None
    )

    # RecordVirtual({cursor-mode=embedded, is-platform=true})
    prop_kwargs: dict = {"cursor-mode": CURSOR_EMBEDDED}
    if not args.no_platform:
        prop_kwargs["is-platform"] = True
    stream_path = call(
        session, "RecordVirtual", GLib.Variant("(a{sv})", (asv(**prop_kwargs),))
    ).unpack()[0]
    print(f"  stream:  {stream_path}  props={prop_kwargs}")

    node_id_box: list[int | None] = [None]
    loop = GLib.MainLoop()

    def on_signal(_connection, _sender, _object_path, _iface, signal_name, parameters, *_user):
        if signal_name != "PipeWireStreamAdded":
            return
        node_id = int(parameters.unpack()[0])
        node_id_box[0] = node_id
        print(f"  PipeWireStreamAdded: node_id={node_id}")
        loop.quit()

    signal_id = bus.signal_subscribe(
        BUS_NAME,
        IFACE_STREAM,
        "PipeWireStreamAdded",
        stream_path,
        None,
        Gio.DBusSignalFlags.NONE,
        on_signal,
        None,
    )

    # Start() — mutter emits PipeWireStreamAdded around this call.
    call(session, "Start", None)
    print("  session.Start() ok — waiting for PipeWireStreamAdded…")

    def timeout_quit():
        if node_id_box[0] is None:
            print("  timeout waiting for PipeWireStreamAdded", file=sys.stderr)
        loop.quit()
        return False

    GLib.timeout_add(8000, timeout_quit)
    if node_id_box[0] is None:
        loop.run()

    bus.signal_unsubscribe(signal_id)

    node_id = node_id_box[0]
    gst = None
    if node_id is None:
        print("error: no PipeWire node — virtual monitor may not appear", file=sys.stderr)
    elif args.no_gst:
        print("  --no-gst: not attaching a consumer (monitor size may stay unset)")
    else:
        if not shutil.which("gst-launch-1.0"):
            print(
                "error: gst-launch-1.0 not found — install gstreamer1.0-tools "
                "and gstreamer1.0-pipewire",
                file=sys.stderr,
            )
            call(session, "Stop", None)
            return 1
        # Propose WxH via caps so mutter sizes the virtual CRTC.
        print(
            f"  gst: pipewiresrc path={node_id} → "
            f"{args.width}x{args.height}@{args.fps} → fakesink"
        )
        gst = subprocess.Popen(
            [
                "gst-launch-1.0",
                "-q",
                "pipewiresrc",
                f"path={node_id}",
                "do-timestamp=true",
                "!",
                f"video/x-raw,width={args.width},height={args.height},framerate={args.fps}/1",
                "!",
                "videoconvert",
                "!",
                "fakesink",
                "sync=false",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
        time.sleep(1.5)
        if gst.poll() is not None:
            err = (gst.stderr.read() if gst.stderr else b"").decode(errors="replace")
            print(f"error: gstreamer exited early:\n{err}", file=sys.stderr)
            call(session, "Stop", None)
            return 1

    after = list_connectors()
    print(f"  connectors after:  {after or '(none listed)'}")
    new = [c for c in after if c not in before]
    if new:
        print(f"  NEW connector(s): {new}  ← Display 2 (open Settings → Displays)")
    else:
        print(
            "  note: no new connector string detected — check Settings → Displays "
            "anyway (naming varies by mutter version)"
        )

    print(f"  holding for {args.hold}s — Ctrl-C to stop early…")

    stopping = False

    def stop(_signum=None, _frame=None):
        nonlocal stopping
        if stopping:
            return
        stopping = True
        print("\n  stopping session…")
        if gst and gst.poll() is None:
            gst.terminate()
            try:
                gst.wait(timeout=2)
            except subprocess.TimeoutExpired:
                gst.kill()
        try:
            call(session, "Stop", None)
        except Exception as e:
            print(f"  Stop() failed: {e}")
        final = list_connectors()
        print(f"  connectors after Stop: {final or '(none listed)'}")
        print("  done.")

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)

    end = time.monotonic() + args.hold
    while not stopping and time.monotonic() < end:
        time.sleep(0.25)
        if gst and gst.poll() is not None:
            print("  gstreamer exited; stopping session")
            break
    stop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
