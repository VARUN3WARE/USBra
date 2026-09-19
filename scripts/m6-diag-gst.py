#!/usr/bin/env python3
"""One-shot: open RecordVirtual, try gst pipelines, try m6-pw-grab.py."""
import subprocess
import sys
import time

import gi

gi.require_version("Gio", "2.0")
gi.require_version("GLib", "2.0")
from gi.repository import Gio, GLib

BUS = "org.gnome.Mutter.ScreenCast"


def call(proxy, method, args):
    return proxy.call_sync(method, args, Gio.DBusCallFlags.NONE, 10000, None)


def main() -> int:
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    root = Gio.DBusProxy.new_sync(
        bus, Gio.DBusProxyFlags.NONE, None, BUS, "/org/gnome/Mutter/ScreenCast", BUS, None
    )
    session_path = call(root, "CreateSession", GLib.Variant("(a{sv})", ({},))).unpack()[0]
    session = Gio.DBusProxy.new_sync(
        bus, Gio.DBusProxyFlags.NONE, None, BUS, session_path, BUS + ".Session", None
    )
    props = {
        "cursor-mode": GLib.Variant("u", 1),
        "is-platform": GLib.Variant("b", True),
    }
    stream_path = call(
        session, "RecordVirtual", GLib.Variant("(a{sv})", (props,))
    ).unpack()[0]

    node = [None]
    loop = GLib.MainLoop()

    def on_sig(*a):
        node[0] = int(a[5].unpack()[0])
        loop.quit()

    sid = bus.signal_subscribe(
        BUS, BUS + ".Stream", "PipeWireStreamAdded", stream_path, None, 0, on_sig, None
    )
    call(session, "Start", None)
    GLib.timeout_add(8000, lambda: (loop.quit(), False)[1])
    loop.run()
    bus.signal_unsubscribe(sid)
    print("node", node[0], flush=True)
    if not node[0]:
        return 1
    n = node[0]

    tests = [
        (
            "probe",
            [
                "gst-launch-1.0",
                "-q",
                "pipewiresrc",
                f"path={n}",
                "do-timestamp=true",
                "!",
                "video/x-raw,width=1280,height=720,framerate=30/1",
                "!",
                "videoconvert",
                "!",
                "fakesink",
                "sync=false",
            ],
        ),
        (
            "maxfr-bgrx",
            [
                "gst-launch-1.0",
                "-q",
                "pipewiresrc",
                f"path={n}",
                "do-timestamp=true",
                "!",
                "video/x-raw,width=1280,height=720,max-framerate=30/1",
                "!",
                "videoconvert",
                "!",
                "video/x-raw,format=BGRx",
                "!",
                "fakesink",
                "sync=false",
            ],
        ),
        (
            "loose",
            [
                "gst-launch-1.0",
                "-q",
                "pipewiresrc",
                f"path={n}",
                "do-timestamp=true",
                "!",
                "videoconvert",
                "!",
                "video/x-raw,format=BGRx",
                "!",
                "fakesink",
                "sync=false",
            ],
        ),
    ]

    ok_label = None
    for label, cmd in tests:
        print("TRY", label, flush=True)
        p = subprocess.Popen(cmd, stderr=subprocess.PIPE, stdout=subprocess.DEVNULL)
        time.sleep(2.5)
        rc = p.poll()
        if rc is None:
            print("OK", label, flush=True)
            p.terminate()
            try:
                p.wait(timeout=3)
            except subprocess.TimeoutExpired:
                p.kill()
            ok_label = label
            break
        err = (p.stderr.read() if p.stderr else b"").decode(errors="replace")[-500:]
        print("FAIL", label, rc, err, flush=True)

    print("TRY grabber", flush=True)
    g = subprocess.Popen(
        [
            "python3",
            "scripts/m6-pw-grab.py",
            "--node-id",
            str(n),
            "--width",
            "1280",
            "--height",
            "720",
            "--fps",
            "30",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    time.sleep(6)
    rc = g.poll()
    if rc is None:
        hdr = g.stdout.read(32)
        print("grabber_header_len", len(hdr), "magic", hdr[:4], flush=True)
        g.terminate()
        try:
            g.wait(timeout=3)
        except subprocess.TimeoutExpired:
            g.kill()
        print("grabber_stderr:", g.stderr.read().decode(errors="replace")[-600:], flush=True)
    else:
        print(
            "grabber_exited",
            rc,
            g.stderr.read().decode(errors="replace")[-800:],
            flush=True,
        )

    call(session, "Stop", None)
    print("done ok_pipeline=", ok_label, flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
