# USBra — Benchmarking Methodology

Deliverable #12. Two rules: (1) every number comes from the built-in
instrumentation, not eyeballing; (2) every run is reproducible from a script.

## Instrumentation (already in the code)

- **Host JSONL** (`usbra-host --stats stats.jsonl`): one line per event —
  `frame {id, ts_ns, rects, bytes, gen_us, send_us}`, `ping`, `ack {latency_us}`,
  `session`, `disconnect`.
- **Client overlay** (on-screen): render fps, PING RTT, MB/s received, frame
  count.
- **FRAME_ACK** (`--frame-ack`): host-side frame→ack latency (both timestamps
  on the host clock — immune to phone/host clock skew). Measures transport +
  phone parse, not vsync present; see "glass-to-glass" below.

## Primary metrics & how to read them

| Metric | Definition | Source | Target (MVP) |
|---|---|---|---|
| Frame production latency | host gen+encode time per frame | `gen_us` | p95 < 2 ms (damage), p95 < 10 ms (full frame) |
| Send latency | syscall write time per frame | `send_us` | p95 < 2 ms |
| Frame→ack latency | host ts of FRAME → host ts of its ACK | `ack.latency_us` | p50 < 15 ms, p95 < 30 ms |
| Transport RTT | PING→PONG, client clock | client overlay | p50 < 5 ms over USB |
| Fps (produced) | frames/s in JSONL | summarize script | ≥ 60 (steady), ≥ 45 (window drag) |
| Fps (rendered) | client frame counter | overlay | ≥ 55 with zero dropped frames > 2 s |
| Bandwidth | bytes/s payload | both sides | < 35 MB/s on USB 2 (fits), report actual |
| Host CPU | `pidstat -p $(pgrep usbra-host) 1` + `perf stat` | OS | < 10 % of one core @ damage-60fps |
| Host GPU | `intel_gpu_top` / `radeontop` / `nvtop` | OS | record, no target yet |
| Phone CPU/GPU | `adb shell dumpsys gfxinfo org.usbra.client`, `simpleperf` | OS | record; renderer thread < 30 % |

## Workloads (scripted, fixed seeds)

1. **Idle desktop** — nothing moving; expect ~0 fps, ~0 B/s (damage-driven).
2. **Text editing** — terminal typing at ~5 chars/s.
3. **Scrolling** — browser/file list continuous scroll (worst common case).
4. **Window drag** — scripted `xdotool`/`ydotool` drag of a 800×600 window.
5. **Full-motion video** — 1080p60 clip on the virtual display (stress; MVP
   expected to degrade — that's M11's motivation).
6. **Resync storm** — unplug/replug loop ×20 (reconnect robustness + recovery
   time).

Runs: 60 s each, 3 repetitions, report p50/p95/p99 + min/max. Hardware
recorded in the results file (CPU, GPU, kernel, GNOME/mutter version, phone
model, Android version, cable/USB generation — check `lsusb -t`).

## Glass-to-glass (optional ground truth)

Instrumented timestamps stop at "ack". For true end-to-end validation use a
high-speed camera (≥ 240 fps) photographing both monitors while the test
pattern flashes a full-screen color toggle on a keystroke; count frame
differential. Do this once per milestone, not per run.

## Commands (host)

```bash
# produce
scripts/run-demo.sh &                       # host + tunnel
adb shell am start -n org.usbra.client/.MainActivity

# measure
usbra-host --source test --stats /tmp/s.jsonl --frame-ack
pidstat -p $(pgrep usbra-host) -h 1 60 > /tmp/cpu.txt
intel_gpu_top -l 60 > /tmp/gpu.txt &        # or radeontop/nvtop

# reduce
python3 benchmarks/summarize_stats.py /tmp/s.jsonl
adb shell dumpsys gfxinfo org.usbra.client  # phone render health
```

## Reporting

Results go to `benchmarks/results/<date>-<hw>.md` with: environment table,
per-workload metric table, anomalies (frame gaps > 100 ms, reconnect events),
and a one-paragraph conclusion. Compare M6 (GNOME backend) vs M0 (test
pattern) to isolate compositor cost from transport cost.
