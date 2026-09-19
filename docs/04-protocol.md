# USBra Wire Protocol v1

Deliverable #5. Normative. Implementation: `protocol/src/lib.rs` (Rust,
authoritative) and `android/.../Protocol.kt` (byte-exact mirror).

## Design rules

- Binary, **little-endian**, stream-framed over any reliable ordered transport
  (TCP today; raw USB bulk later).
- Small fixed set of messages. Fields only when a consumer exists.
- Every FRAME carries its own damage rects; pixels only for those rects.
- Lossless-first (raw pixels); compression is a per-frame codec choice, so
  hardware video codecs can be added later without protocol surgery.

## Framing

Every message = 16-byte header + `length` payload bytes.

```
offset  size  field
0       4     magic       u32 = 0x5242_5355  ("USBR", bytes on wire: 55 53 42 52)
4       2     version     u16 = 1
6       2     msg_type    u16 (see below)
8       4     length      u32  payload byte count (0..=64 MiB)
12      4     flags       u32  reserved, 0 in v1
```

Limits: payload ≤ 64 MiB; strings ≤ 255 bytes UTF-8 (length-prefixed with u8);
rects per FRAME/DAMAGE_REGION ≤ 255 (header of that message carries u16 count;
senders must merge to ≤ 64 by policy).

Message types:

```
1  HELLO           client → host   session request + capabilities
2  HELLO_OK        host → client   session accepted, negotiated caps
3  CONFIG          host → client   authoritative session parameters
4  DISPLAY_INFO    host → client   the virtual display's geometry/format
5  FRAME           host → client   pixels for a set of damage rects
6  DAMAGE_REGION   host → client   damage metadata without pixels (reserved:
                                  e.g. cursor-only updates when cursor plane
                                  is sent as metadata in a later version)
7  PING            client → host   RTT probe (carries client timestamp)
8  PONG            host → client   echoes PING + host timestamp
9  DISCONNECT      either          graceful close with reason
10 RECONNECT       client → host   resume a known session after link loss
11 FRAME_ACK       client → host   (optional, if CONFIG.flags bit0) frame
                                  received/parsed — enables host-side
                                  frame→ack latency measurement
```

## Common types

```
Rect        = { x: u16, y: u16, w: u16, h: u16 }        // 8 bytes, inclusive
             // origin top-left, pixels [x, x+w) × [y, y+h)

codec        0 = RAW (payload = raw rows, below)
             1 = ZSTD (payload = single zstd frame of the RAW concatenation)

pixel_format 0 = BGRX  — 4 bytes/px, memory order B,G,R,X
                       (DRM_FORMAT_XRGB8888 / PipeWire BGRx, little-endian)
             1 = RGBX  — 4 bytes/px, memory order R,G,B,X
                       (DRM_FORMAT_XBGR8888)
```

Stride rule for RAW payloads: each rect's rows are **tightly packed**
(row stride = `w * 4`); rects are concatenated in rect-list order. The
client's GL upload maps this to `glTexSubImage2D` per rect with
`GL_UNPACK_ROW_LENGTH = 0`. `rect byte range` of rect *i* =
`[Σ_{j<i} w_j*h_j*4, +w_i*h_i*4)`.

## Messages

### HELLO (client → host)

```
u16  proto_version      must be 1
u32  capabilities       bitmask: bit0 RAW, bit1 ZSTD
                          (reserved: bit2 H264, bit3 AV1, bit4 RGB565,
                           bit5 CURSOR_METADATA)
u16  screen_width       phone panel width (informational; host may match the
u16  screen_height      virtual monitor to it in M6)
u16  refresh_hint       preferred max fps
u8   name_len + bytes   device name, UTF-8 (e.g. "Pixel 8a")
```

### HELLO_OK (host → client)

```
u16  proto_version      host's version (client errors out if > its own)
u32  capabilities       negotiated = client ∩ host
u64  session_id         nonzero, random; identifies the session for RECONNECT
u16  max_rects          max rects per FRAME the host will send (≤ 64)
u16  reserved           0
```

### DISPLAY_INFO (host → client)

```
u16  width              virtual display size in pixels
u16  height
u16  refresh            nominal refresh, Hz
u8   pixel_format       0 BGRX / 1 RGBX
u8   flags              bit0: cursor embedded in frames
u8   name_len + bytes   monitor name (e.g. "USBra Test Pattern")
```

### CONFIG (host → client, authoritative)

```
u8   codec              0 RAW / 1 ZSTD
u8   pixel_format       wire pixel format for FRAME payloads
u16  refresh_hint       fps the host targets
u16  flags              bit0: FRAME_ACK requested
```

### FRAME (host → client)

```
u64  frame_id           monotonically increasing per session, starts at 1
u64  timestamp_ns       host clock, UNIX-EPOCH ns (see Timestamps)
u8   codec
u8   pixel_format
u16  rect_count
u16  reserved           0
Rect × rect_count
[payload]               RAW: concatenated tightly-packed rect rows
                        ZSTD: zstd(RAW concatenation), same rect list
```

Semantics: each FRAME applies on top of the client's current texture state;
rects not included are unchanged. A full-frame update is simply a single rect
`{0,0,width,height}`. Clients must apply rects in order. If a client drops
frames (bounded queue overflow), it may show stale pixels until the next
full-frame FRAME — hosts MUST send a periodic full frame (test source: every
2 s; GNOME backend: on resync events, and at least every 5 s) so recovery is
bounded.

### DAMAGE_REGION (host → client)

```
u64  frame_id           id this damage belongs to (usually the next FRAME)
u64  timestamp_ns
u16  rect_count
u16  reserved           0
Rect × rect_count
```

v1 use: advisory/heartbeat (e.g. cursor-plane moves once cursor metadata ships).
Receivers must tolerate it with no pixel payload and no visible change.

### PING / PONG

```
PING: u64 client_ts_ns
PONG: u64 echoed_client_ts_ns, u64 host_ts_ns
```
RTT = client_now − echoed_client_ts_ns (single-clock, skew-free). host_ts_ns
is informational (skewed clocks; never used for math).

### DISCONNECT

```
u16  reason             1 UNKNOWN_SESSION, 2 PROTOCOL, 3 SHUTDOWN, 4 ERROR
u8   msg_len + bytes    human-readable detail
```
Sender closes the transport right after. Client receiving `UNKNOWN_SESSION`
on RECONNECT must fall back to a fresh HELLO next attempt.

### RECONNECT (client → host)

```
u64  session_id         from HELLO_OK
u64  last_frame_id      highest frame_id the client applied
```
Host validates session_id; on match it re-sends DISPLAY_INFO + CONFIG and a
**full-frame FRAME** (v1 always resyncs fully; delta-resume is a v2 candidate
once proven necessary). On mismatch: DISCONNECT(UNKNOWN_SESSION).

### FRAME_ACK (client → host, only if CONFIG.flags bit0)

```
u64  frame_id
u64  client_ts_ns       informational (device clock)
```
Host computes latency = host_now − FRAME.timestamp_ns (both host clock) and
logs it. This sidesteps clock-skew entirely and is the primary latency metric
of the benchmark harness.

## Session state machines

```
client:  DISCONNECTED → CONNECTING → HELLO/RECONNECT → ACTIVE ⇄ (io error →
         CONNECTING with RECONNECT if session_id known)
host:    ACCEPT → HELLO|RECONNECT? → HELLO_OK+DISPLAY_INFO+CONFIG → STREAMING
         → (EOF | DISCONNECT | io error) → session table retains
           (session_id, last_frame_id) for RECONNECT
```

## Timestamps

v1 uses `CLOCK_REALTIME`-equivalent (UNIX epoch) ns from the host. Rationale:
`std::time` only; frame→ack latency is computed host-side against the same
clock, so NTP skew between host and phone never enters the math. v2 may switch
to `CLOCK_MONOTONIC_RAW` + an explicit clock-id field once a cross-device
timestamp consumer exists.

## Versioning & extension policy

- Bump `version` only for wire incompatibility. New messages get new type IDs.
- New capability bits in HELLO are safe: hosts mask unknown bits and must not
  enable features the client didn't advertise.
- New pixel formats/codec IDs must be negotiated via capabilities before use.
- Unknown message types: log and close with DISCONNECT(PROTOCOL) — v1 has no
  skip-forward (no payload-type registry), keeping the parser tiny.

## Example: HELLO from "Pixel 8a", 1080×2340, 60 Hz, RAW

```
55 53 42 52  01 00  01 00  18 00 00 00  00 00 00 00   header (len=24)
01 00        proto_version = 1
01 00 00 00  caps = RAW
38 04        screen_width  = 1080
28 09        screen_height = 2340
3c 00        refresh_hint  = 60
08           name_len = 8
50 69 78 65 6c 20 38 61                        "Pixel 8a"
```
