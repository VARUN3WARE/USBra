# benchmarks

Tooling for the measurement methodology defined in
[`docs/07-benchmarking.md`](../docs/07-benchmarking.md).

## summarize_stats.py

Summarizes a host JSONL event log into the four primary metrics
(fps, bandwidth, host latency budget, ack latency):

```bash
scripts/run-demo.sh                    # writes /tmp/usbra-stats.jsonl
# … let it run for a minute while you watch the phone …
python3 benchmarks/summarize_stats.py /tmp/usbra-stats.jsonl
```

Typical output:

```text
USBra stats summary — /tmp/usbra-stats.jsonl
  frames            : 3120 over 52.3 s → 59.7 fps
  bandwidth         : 0.94 MB/s (7.5 Mbit/s), mean 10.1 KiB/frame
  frame sizes       : p50 8.2 KiB  p95 320.5 KiB  max 5626.0 KiB
    ≤ 64 KiB           2891  ████████████████████████████████████████
    64 KiB–512 KiB       222  ███
    512 KiB–2 MiB          5
    > 2 MiB                2
  damage rects      : p50 3  p95 4  max 6
  host gen (M2)     : p50 212 µs  p95 480 µs  max 3.10 ms
  host send         : p50 96 µs  p95 240 µs  max 2.20 ms
  ack latency       : p50 8.90 ms  p95 14.00 ms  n=3120
```

The client-side overlay (fps / RTT / MB/s) is the other half of the picture;
the optional ground-truth method (high-speed camera) is described in the doc.
