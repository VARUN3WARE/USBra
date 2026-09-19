#!/usr/bin/env python3
"""M6 PipeWire → stdout frame grabber (GStreamer appsink).

Single pipeline (retrying multiple pipelines on one node poisons negotiation).
Matches GTK headless-monitor-tests + our --probe-gnome gst-launch line.

Wire format on stdout (little-endian), one message per frame:

  magic u32=0x31464255 ("UBF1") | frame_id u64 | timestamp_ns u64
  | width u16 | height u16 | stride u32 | payload_len u32 | BGRx payload
"""
from __future__ import annotations

import argparse
import signal
import struct
import sys
import time

MAGIC = 0x31464255


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--node-id", type=int, required=True)
    ap.add_argument("--width", type=int, required=True)
    ap.add_argument("--height", type=int, required=True)
    ap.add_argument("--fps", type=int, default=60)
    args = ap.parse_args()

    try:
        import gi

        gi.require_version("Gst", "1.0")
        from gi.repository import Gst
    except (ImportError, ValueError) as e:
        print(f"error: need python3-gi + GStreamer: {e}", file=sys.stderr)
        return 1

    Gst.init(None)
    n, w, h, fps = args.node_id, args.width, args.height, args.fps

    # GTK uses max-framerate; forcing exact framerate often yields not-negotiated.
    desc = (
        f"pipewiresrc path={n} do-timestamp=true ! "
        f"video/x-raw,width={w},height={h},max-framerate={fps}/1 ! "
        f"videoconvert ! video/x-raw,format=BGRx ! "
        f"appsink name=sink emit-signals=false sync=false max-buffers=2 drop=true"
    )
    print(f"m6-pw-grab: {desc}", file=sys.stderr, flush=True)

    try:
        pipeline = Gst.parse_launch(desc)
    except Exception as e:
        print(f"error: parse: {e}", file=sys.stderr)
        return 1

    sink = pipeline.get_by_name("sink")
    pipeline.set_state(Gst.State.PLAYING)
    ret, state, _ = pipeline.get_state(10 * Gst.SECOND)
    if state != Gst.State.PLAYING:
        bus = pipeline.get_bus()
        while True:
            msg = bus.pop_filtered(Gst.MessageType.ERROR | Gst.MessageType.WARNING)
            if msg is None:
                break
            if msg.type == Gst.MessageType.ERROR:
                err, dbg = msg.parse_error()
                print(f"error: {err} | {dbg}", file=sys.stderr)
            else:
                err, dbg = msg.parse_warning()
                print(f"warning: {err} | {dbg}", file=sys.stderr)
        print(f"error: play failed ret={ret} state={state}", file=sys.stderr)
        # Fallback: no size/fps caps on pipewiresrc (accept native buffer size).
        pipeline.set_state(Gst.State.NULL)
        desc2 = (
            f"pipewiresrc path={n} do-timestamp=true ! "
            f"videoconvert ! video/x-raw,format=BGRx ! "
            f"appsink name=sink emit-signals=false sync=false max-buffers=2 drop=true"
        )
        print(f"m6-pw-grab: fallback {desc2}", file=sys.stderr, flush=True)
        pipeline = Gst.parse_launch(desc2)
        sink = pipeline.get_by_name("sink")
        pipeline.set_state(Gst.State.PLAYING)
        ret, state, _ = pipeline.get_state(10 * Gst.SECOND)
        if state != Gst.State.PLAYING:
            bus = pipeline.get_bus()
            msg = bus.pop_filtered(Gst.MessageType.ERROR)
            if msg:
                err, dbg = msg.parse_error()
                print(f"error: {err} | {dbg}", file=sys.stderr)
            print(f"error: fallback play failed ret={ret} state={state}", file=sys.stderr)
            pipeline.set_state(Gst.State.NULL)
            return 1

    stopping = False

    def on_stop(*_a):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGINT, on_stop)
    signal.signal(signal.SIGTERM, on_stop)
    print(f"m6-pw-grab: streaming node={n}", file=sys.stderr, flush=True)

    frame_id = 0
    out = sys.stdout.buffer
    try:
        while not stopping:
            sample = sink.emit("try-pull-sample", 500 * Gst.MSECOND)
            if sample is None:
                bus = pipeline.get_bus()
                msg = bus.pop_filtered(Gst.MessageType.ERROR | Gst.MessageType.EOS)
                if msg is not None:
                    if msg.type == Gst.MessageType.ERROR:
                        err, dbg = msg.parse_error()
                        print(f"error: {err} ({dbg})", file=sys.stderr)
                        return 2
                    break
                continue

            buf = sample.get_buffer()
            ok, mapinfo = buf.map(Gst.MapFlags.READ)
            if not ok:
                continue
            try:
                data = bytes(mapinfo.data)
            finally:
                buf.unmap(mapinfo)

            caps = sample.get_caps().get_structure(0)
            ok_w, cw = caps.get_int("width")
            ok_h, ch = caps.get_int("height")
            if not (ok_w and ok_h) or cw <= 0 or ch <= 0:
                continue
            stride = len(data) // ch
            row = cw * 4
            if stride < row:
                continue
            if stride == row:
                payload = data[: row * ch]
            else:
                packed = bytearray(row * ch)
                for y in range(ch):
                    packed[y * row : (y + 1) * row] = data[y * stride : y * stride + row]
                payload = bytes(packed)

            frame_id += 1
            header = struct.pack(
                "<IQQHHII", MAGIC, frame_id, time.time_ns(), cw, ch, row, len(payload)
            )
            try:
                out.write(header)
                out.write(payload)
                out.flush()
            except BrokenPipeError:
                break
            if frame_id == 1:
                print(f"m6-pw-grab: first frame {cw}x{ch}", file=sys.stderr, flush=True)
    finally:
        pipeline.set_state(Gst.State.NULL)
        print(f"m6-pw-grab: exit after {frame_id} frames", file=sys.stderr)

    return 0


if __name__ == "__main__":
    sys.exit(main())
