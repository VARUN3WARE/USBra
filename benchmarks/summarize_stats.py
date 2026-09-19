#!/usr/bin/env python3
"""Summarize a usbra-host JSONL stats file — the host half of docs/07-benchmarking.md.

Usage: benchmarks/summarize_stats.py /tmp/usbra-stats.jsonl
Reads the stream produced by `usbra-host --stats FILE` and prints the four
primary metrics (fps, bandwidth, host-side latency budget, ack latency).
"""
from __future__ import annotations

import json
import sys
from collections import Counter


def pct(sorted_vals, q):
    if not sorted_vals:
        return 0
    idx = round((len(sorted_vals) - 1) * q)
    return sorted_vals[min(idx, len(sorted_vals) - 1)]


def fmt_us(v):
    return f"{v / 1000:.2f} ms" if v >= 1000 else f"{v} µs"


def main(path: str) -> int:
    frames = []
    acks: list[int] = []
    others: Counter = Counter()
    t_first = None
    t_last = None

    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                e = json.loads(line)
            except json.JSONDecodeError:
                continue
            ev = e.get("ev")
            if ev == "frame":
                frames.append(e)
                t = e.get("t_ms")
                t_first = t if t_first is None else t_first
                t_last = t
            elif ev == "ack":
                acks.append(int(e.get("latency_us", 0)))
            else:
                others[ev] += 1

    if not frames:
        print(f"no frames found in {path}")
        return 1

    n = len(frames)
    dur_s = ((t_last - t_first) / 1000.0) if t_first is not None and t_last is not None else 0.0
    fps = n / dur_s if dur_s > 0 else float("nan")

    gen = sorted(int(f["gen_us"]) for f in frames)
    send = sorted(int(f["send_us"]) for f in frames)
    sizes = sorted(int(f["bytes"]) for f in frames)
    rects = sorted(int(f["rects"]) for f in frames)

    total_bytes = sum(sizes)
    mean_bytes = total_bytes / n
    bw_mb_s = (total_bytes / (1024 * 1024) / dur_s) if dur_s > 0 else float("nan")

    # size buckets → damage vs full-frame traffic
    buckets = [("≤ 64 KiB", 0), ("64 KiB–512 KiB", 0), ("512 KiB–2 MiB", 0), ("> 2 MiB", 0)]
    for s in sizes:
        if s <= 64 * 1024:
            buckets[0] = (buckets[0][0], buckets[0][1] + 1)
        elif s <= 512 * 1024:
            buckets[1] = (buckets[1][0], buckets[1][1] + 1)
        elif s <= 2 * 1024 * 1024:
            buckets[2] = (buckets[2][0], buckets[2][1] + 1)
        else:
            buckets[3] = (buckets[3][0], buckets[3][1] + 1)

    print(f"USBra stats summary — {path}")
    print(f"  frames            : {n} over {dur_s:.1f} s → {fps:.1f} fps")
    print(f"  bandwidth         : {bw_mb_s:.2f} MB/s ({bw_mb_s * 8:.1f} Mbit/s), "
          f"mean {mean_bytes / 1024:.1f} KiB/frame")
    print(f"  frame sizes       : p50 {sizes[n // 2] / 1024:.1f} KiB  "
          f"p95 {pct(sizes, 0.95) / 1024:.1f} KiB  max {sizes[-1] / 1024:.1f} KiB")
    for label, count in buckets:
        bar = "█" * max(1, round(count * 40 / n))
        print(f"    {label:<16} {count:>7}  {bar}")
    print(f"  damage rects      : p50 {pct(rects, 0.5)}  p95 {pct(rects, 0.95)}  max {rects[-1]}")
    print(f"  host gen (M2)     : p50 {fmt_us(pct(gen, 0.5))}  p95 {fmt_us(pct(gen, 0.95))}  "
          f"max {fmt_us(gen[-1])}")
    print(f"  host send         : p50 {fmt_us(pct(send, 0.5))}  p95 {fmt_us(pct(send, 0.95))}  "
          f"max {fmt_us(send[-1])}")
    if acks:
        a = sorted(acks)
        print(f"  ack latency       : p50 {fmt_us(pct(a, 0.5))}  p95 {fmt_us(pct(a, 0.95))}  "
              f"n={len(a)}  (host send → client ack; add client render for glass-to-glass)")
    if others:
        detail = ", ".join(f"{k}×{v}" for k, v in others.most_common())
        print(f"  other events      : {detail}")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <stats.jsonl>", file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
