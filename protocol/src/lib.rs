//! USBra wire protocol v1.
//!
//! Binary, little-endian, framed messages over any reliable ordered stream.
//! Normative spec: `docs/04-protocol.md`. This crate is the authoritative
//! implementation; the Kotlin reader in `android/` mirrors it byte-for-byte.
//!
//! Design invariants:
//! * [`unsafe`] free, `std` only.
//! * Every message roundtrips through [`Message::to_bytes`] /
//!   [`Message::decode`] — enforced by tests.
//! * Unknown/invalid input is an [`Error`], never a panic (except the
//!   programmer-error assertion on oversize strings).

use std::fmt;
use std::io::{self, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// `u32` that appears as ASCII bytes `55 53 42 52` ("USBR") on the wire.
/// (Value: 0x5242_5355 — i.e. the "USBR" bytes read little-endian.)
pub const MAGIC: u32 = u32::from_le_bytes(*b"USBR");
/// Current protocol version.
pub const VERSION: u16 = 1;
/// Framing header size in bytes.
pub const HEADER_LEN: usize = 16;
/// Maximum payload size we accept (guards against corrupt length fields).
pub const MAX_PAYLOAD: usize = 64 * 1024 * 1024;
/// Maximum string length (length prefix is `u8`).
pub const MAX_STRING: usize = 255;
/// Rects-per-message policy limit enforced by senders.
pub const MAX_RECTS: u16 = 64;

// ---------------------------------------------------------------------------
// Message type ids
// ---------------------------------------------------------------------------

pub mod msg {
    pub const HELLO: u16 = 1;
    pub const HELLO_OK: u16 = 2;
    pub const CONFIG: u16 = 3;
    pub const DISPLAY_INFO: u16 = 4;
    pub const FRAME: u16 = 5;
    pub const DAMAGE_REGION: u16 = 6;
    pub const PING: u16 = 7;
    pub const PONG: u16 = 8;
    pub const DISCONNECT: u16 = 9;
    pub const RECONNECT: u16 = 10;
    pub const FRAME_ACK: u16 = 11;

    pub fn name(t: u16) -> &'static str {
        match t {
            HELLO => "HELLO",
            HELLO_OK => "HELLO_OK",
            CONFIG => "CONFIG",
            DISPLAY_INFO => "DISPLAY_INFO",
            FRAME => "FRAME",
            DAMAGE_REGION => "DAMAGE_REGION",
            PING => "PING",
            PONG => "PONG",
            DISCONNECT => "DISCONNECT",
            RECONNECT => "RECONNECT",
            FRAME_ACK => "FRAME_ACK",
            _ => "UNKNOWN",
        }
    }
}

// ---------------------------------------------------------------------------
// Capability / codec / format / reason registries
// ---------------------------------------------------------------------------

/// Session capabilities (HELLO bitfield).
pub mod caps {
    /// Raw tightly-packed rect pixels (always supported by v1 peers).
    pub const RAW: u32 = 1 << 0;
    /// Zstd-compressed frame payloads.
    pub const ZSTD: u32 = 1 << 1;
    // Reserved: H264 = 1<<2, AV1 = 1<<3, RGB565 = 1<<4, CURSOR_METADATA = 1<<5
}

/// FRAME payload codecs.
pub mod codec {
    /// Payload = concatenated tightly-packed rect rows.
    pub const RAW: u8 = 0;
    /// Payload = single zstd frame of the RAW concatenation.
    pub const ZSTD: u8 = 1;

    pub fn valid(c: u8) -> bool {
        c <= 1
    }
    pub fn name(c: u8) -> &'static str {
        match c {
            RAW => "raw",
            ZSTD => "zstd",
            _ => "?",
        }
    }
}

/// Pixel formats. Value names the **memory byte order**, 4 bytes per pixel.
pub mod pixel_format {
    /// DRM_FORMAT_XRGB8888 little-endian (bytes B, G, R, X) — the format
    /// mutter/evdi framebuffers use.
    pub const BGRX: u8 = 0;
    /// DRM_FORMAT_XBGR8888 little-endian (bytes R, G, B, X).
    pub const RGBX: u8 = 1;

    pub fn valid(f: u8) -> bool {
        f <= 1
    }
    pub fn name(f: u8) -> &'static str {
        match f {
            BGRX => "BGRX",
            RGBX => "RGBX",
            _ => "?",
        }
    }
}

/// DISCONNECT reason codes.
pub mod reason {
    pub const UNKNOWN_SESSION: u16 = 1;
    pub const PROTOCOL: u16 = 2;
    pub const SHUTDOWN: u16 = 3;
    pub const ERROR: u16 = 4;
}

/// Host wall-clock nanoseconds since UNIX epoch (v1 timestamp basis; see
/// `docs/04-protocol.md` § Timestamps for why skew never matters).
pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    BadMagic(u32),
    UnsupportedVersion(u16),
    UnknownMessageType(u16),
    Truncated { need: usize, have: usize },
    Trailing(usize),
    BadString,
    BadPixelFormat(u8),
    BadCodec(u8),
    PayloadTooLarge(u32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadMagic(m) => write!(f, "bad magic 0x{m:08x} (not USBra)"),
            Error::UnsupportedVersion(v) => write!(f, "unsupported protocol version {v}"),
            Error::UnknownMessageType(t) => write!(f, "unknown message type {t}"),
            Error::Truncated { need, have } => {
                write!(f, "truncated payload: need {need} bytes, have {have}")
            }
            Error::Trailing(n) => write!(f, "{n} trailing bytes after message"),
            Error::BadString => write!(f, "invalid string (not UTF-8 or oversize length)"),
            Error::BadPixelFormat(v) => write!(f, "bad pixel format {v}"),
            Error::BadCodec(v) => write!(f, "bad codec {v}"),
            Error::PayloadTooLarge(n) => write!(f, "payload too large: {n} bytes"),
        }
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub magic: u32,
    pub version: u16,
    pub msg_type: u16,
    pub length: u32,
    pub flags: u32,
}

impl Header {
    pub fn new(msg_type: u16, length: u32) -> Self {
        Header { magic: MAGIC, version: VERSION, msg_type, length, flags: 0 }
    }

    pub fn to_bytes(self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&self.magic.to_le_bytes());
        b[4..6].copy_from_slice(&self.version.to_le_bytes());
        b[6..8].copy_from_slice(&self.msg_type.to_le_bytes());
        b[8..12].copy_from_slice(&self.length.to_le_bytes());
        b[12..16].copy_from_slice(&self.flags.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Header, Error> {
        if b.len() < HEADER_LEN {
            return Err(Error::Truncated { need: HEADER_LEN, have: b.len() });
        }
        let magic = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if magic != MAGIC {
            return Err(Error::BadMagic(magic));
        }
        let version = u16::from_le_bytes([b[4], b[5]]);
        if version > VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        Ok(Header {
            magic,
            version,
            msg_type: u16::from_le_bytes([b[6], b[7]]),
            length: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            flags: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        })
    }
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Damage rectangle. Origin top-left; covers pixels `[x, x+w) × [y, y+h)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Rect { x, y, w, h }
    }
    pub const fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }
    /// Bytes of tightly-packed rows for this rect at 4 bytes/pixel.
    pub const fn byte_len(&self) -> u64 {
        self.area() * 4
    }
    /// Intersection with another rect, if non-empty.
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x1 = self.x.max(o.x);
        let y1 = self.y.max(o.y);
        let x2 = (self.x + self.w).min(o.x + o.w);
        let y2 = (self.y + self.h).min(o.y + o.h);
        if x2 > x1 && y2 > y1 {
            Some(Rect { x: x1, y: y1, w: x2 - x1, h: y2 - y1 })
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Payload structs
// ---------------------------------------------------------------------------

/// Client → host: session request and capability negotiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub proto_version: u16,
    pub capabilities: u32,
    pub screen_w: u16,
    pub screen_h: u16,
    pub refresh_hint: u16,
    pub name: String,
}

/// Host → client: session accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelloOk {
    pub proto_version: u16,
    pub capabilities: u32,
    pub session_id: u64,
    pub max_rects: u16,
}

/// Host → client: the virtual display's geometry and native format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    pub width: u16,
    pub height: u16,
    pub refresh: u16,
    pub pixel_format: u8,
    pub flags: u8,
    pub name: String,
}

impl DisplayInfo {
    /// Bit 0: cursor is embedded in frames.
    pub const FLAG_CURSOR_EMBEDDED: u8 = 1 << 0;
}

/// Host → client: authoritative session parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub codec: u8,
    pub pixel_format: u8,
    pub refresh_hint: u16,
    pub flags: u16,
}

impl Config {
    /// Bit 0: host wants FRAME_ACK for every frame.
    pub const FLAG_FRAME_ACK: u16 = 1 << 0;
}

/// Host → client: pixels for a set of damage rects, applied incrementally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_id: u64,
    pub timestamp_ns: u64,
    pub codec: u8,
    pub pixel_format: u8,
    pub rects: Vec<Rect>,
    /// RAW: concatenated tightly-packed rect rows (rect order); ZSTD: a
    /// single zstd frame of that concatenation.
    pub payload: Vec<u8>,
}

impl Frame {
    /// Expected RAW payload length (sum of rect areas × 4).
    pub fn raw_payload_len(&self) -> u64 {
        self.rects.iter().map(|r| r.byte_len()).sum()
    }

    /// Byte range of rect `i` inside the RAW payload (tightly packed,
    /// concatenated in rect order). Returns `None` for bad indices or the
    /// ZSTD codec (whose payload is a single opaque blob).
    pub fn rect_byte_range(&self, i: usize) -> Option<(usize, usize)> {
        if self.codec != codec::RAW || i >= self.rects.len() {
            return None;
        }
        let mut off: u64 = 0;
        for r in &self.rects[..i] {
            off += r.byte_len();
        }
        let len = self.rects[i].byte_len();
        Some((off as usize, (off + len) as usize))
    }

    /// True if this frame's rects cover the whole `w×h` display.
    pub fn covers(&self, w: u16, h: u16) -> bool {
        self.rects.len() == 1
            && self.rects[0].x == 0
            && self.rects[0].y == 0
            && self.rects[0].w == w
            && self.rects[0].h == h
    }
}

/// Host → client: damage metadata without pixels (reserved; e.g. cursor-only
/// updates in a later version).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DamageRegion {
    pub frame_id: u64,
    pub timestamp_ns: u64,
    pub rects: Vec<Rect>,
}

/// Client → host: RTT probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ping {
    pub client_ts_ns: u64,
}

/// Host → client: RTT reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pong {
    pub echoed_client_ts_ns: u64,
    pub host_ts_ns: u64,
}

/// Either side: graceful close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disconnect {
    pub reason: u16,
    pub message: String,
}

/// Client → host: resume a known session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reconnect {
    pub session_id: u64,
    pub last_frame_id: u64,
}

/// Client → host (optional): frame received/parsed acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameAck {
    pub frame_id: u64,
    pub client_ts_ns: u64,
}

// ---------------------------------------------------------------------------
// Message enum + encode/decode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Hello(Hello),
    HelloOk(HelloOk),
    Config(Config),
    DisplayInfo(DisplayInfo),
    Frame(Frame),
    DamageRegion(DamageRegion),
    Ping(Ping),
    Pong(Pong),
    Disconnect(Disconnect),
    Reconnect(Reconnect),
    FrameAck(FrameAck),
}

impl Message {
    pub fn msg_type(&self) -> u16 {
        match self {
            Message::Hello(_) => msg::HELLO,
            Message::HelloOk(_) => msg::HELLO_OK,
            Message::Config(_) => msg::CONFIG,
            Message::DisplayInfo(_) => msg::DISPLAY_INFO,
            Message::Frame(_) => msg::FRAME,
            Message::DamageRegion(_) => msg::DAMAGE_REGION,
            Message::Ping(_) => msg::PING,
            Message::Pong(_) => msg::PONG,
            Message::Disconnect(_) => msg::DISCONNECT,
            Message::Reconnect(_) => msg::RECONNECT,
            Message::FrameAck(_) => msg::FRAME_ACK,
        }
    }

    pub fn type_name(&self) -> &'static str {
        msg::name(self.msg_type())
    }

    /// Header + payload as wire bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(64);
        match self {
            Message::Hello(m) => {
                put_u16(&mut payload, m.proto_version);
                put_u32(&mut payload, m.capabilities);
                put_u16(&mut payload, m.screen_w);
                put_u16(&mut payload, m.screen_h);
                put_u16(&mut payload, m.refresh_hint);
                put_str(&mut payload, &m.name);
            }
            Message::HelloOk(m) => {
                put_u16(&mut payload, m.proto_version);
                put_u32(&mut payload, m.capabilities);
                put_u64(&mut payload, m.session_id);
                put_u16(&mut payload, m.max_rects);
                put_u16(&mut payload, 0);
            }
            Message::Config(m) => {
                payload.push(m.codec);
                payload.push(m.pixel_format);
                put_u16(&mut payload, m.refresh_hint);
                put_u16(&mut payload, m.flags);
            }
            Message::DisplayInfo(m) => {
                put_u16(&mut payload, m.width);
                put_u16(&mut payload, m.height);
                put_u16(&mut payload, m.refresh);
                payload.push(m.pixel_format);
                payload.push(m.flags);
                put_str(&mut payload, &m.name);
            }
            Message::Frame(m) => {
                put_u64(&mut payload, m.frame_id);
                put_u64(&mut payload, m.timestamp_ns);
                payload.push(m.codec);
                payload.push(m.pixel_format);
                put_u16(&mut payload, m.rects.len() as u16);
                put_u16(&mut payload, 0);
                for r in &m.rects {
                    put_rect(&mut payload, r);
                }
                payload.extend_from_slice(&m.payload);
            }
            Message::DamageRegion(m) => {
                put_u64(&mut payload, m.frame_id);
                put_u64(&mut payload, m.timestamp_ns);
                put_u16(&mut payload, m.rects.len() as u16);
                put_u16(&mut payload, 0);
                for r in &m.rects {
                    put_rect(&mut payload, r);
                }
            }
            Message::Ping(m) => put_u64(&mut payload, m.client_ts_ns),
            Message::Pong(m) => {
                put_u64(&mut payload, m.echoed_client_ts_ns);
                put_u64(&mut payload, m.host_ts_ns);
            }
            Message::Disconnect(m) => {
                put_u16(&mut payload, m.reason);
                put_str(&mut payload, &m.message);
            }
            Message::Reconnect(m) => {
                put_u64(&mut payload, m.session_id);
                put_u64(&mut payload, m.last_frame_id);
            }
            Message::FrameAck(m) => {
                put_u64(&mut payload, m.frame_id);
                put_u64(&mut payload, m.client_ts_ns);
            }
        }
        assert!(
            payload.len() <= MAX_PAYLOAD,
            "payload exceeds {MAX_PAYLOAD} bytes (bug)"
        );
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
        out.extend_from_slice(&Header::new(self.msg_type(), payload.len() as u32).to_bytes());
        out.append(&mut payload);
        out
    }

    pub fn decode(msg_type: u16, payload: &[u8]) -> Result<Message, Error> {
        let mut c = Cur { buf: payload, pos: 0 };
        let m = match msg_type {
            msg::HELLO => Message::Hello(Hello {
                proto_version: c.u16()?,
                capabilities: c.u32()?,
                screen_w: c.u16()?,
                screen_h: c.u16()?,
                refresh_hint: c.u16()?,
                name: c.string()?,
            }),
            msg::HELLO_OK => {
                let proto_version = c.u16()?;
                let capabilities = c.u32()?;
                let session_id = c.u64()?;
                let max_rects = c.u16()?;
                let _reserved = c.u16()?;
                Message::HelloOk(HelloOk { proto_version, capabilities, session_id, max_rects })
            }
            msg::CONFIG => Message::Config(Config {
                codec: c.u8()?,
                pixel_format: c.u8()?,
                refresh_hint: c.u16()?,
                flags: c.u16()?,
            }),
            msg::DISPLAY_INFO => Message::DisplayInfo(DisplayInfo {
                width: c.u16()?,
                height: c.u16()?,
                refresh: c.u16()?,
                pixel_format: c.u8()?,
                flags: c.u8()?,
                name: c.string()?,
            }),
            msg::FRAME => {
                let frame_id = c.u64()?;
                let timestamp_ns = c.u64()?;
                let codec_v = c.u8()?;
                let pf = c.u8()?;
                let count = c.u16()? as usize;
                let _reserved = c.u16()?;
                if count > MAX_RECTS as usize {
                    return Err(Error::Truncated { need: count * 8, have: c.buf.len() - c.pos });
                }
                let mut rects = Vec::with_capacity(count);
                for _ in 0..count {
                    rects.push(c.rect()?);
                }
                let payload = c.rest().to_vec();
                // RAW payloads must exactly match the packed rect areas —
                // protects peers from out-of-bounds uploads (the Android
                // renderer relies on this invariant).
                if codec_v == codec::RAW {
                    let expect: u64 = rects.iter().map(|r| r.byte_len()).sum();
                    if payload.len() as u64 != expect {
                        return Err(Error::Truncated {
                            need: expect as usize,
                            have: payload.len(),
                        });
                    }
                }
                Message::Frame(Frame {
                    frame_id,
                    timestamp_ns,
                    codec: codec_v,
                    pixel_format: pf,
                    rects,
                    payload,
                })
            }
            msg::DAMAGE_REGION => {
                let frame_id = c.u64()?;
                let timestamp_ns = c.u64()?;
                let count = c.u16()? as usize;
                let _reserved = c.u16()?;
                if count > MAX_RECTS as usize {
                    return Err(Error::Truncated { need: count * 8, have: c.buf.len() - c.pos });
                }
                let mut rects = Vec::with_capacity(count);
                for _ in 0..count {
                    rects.push(c.rect()?);
                }
                Message::DamageRegion(DamageRegion { frame_id, timestamp_ns, rects })
            }
            msg::PING => Message::Ping(Ping { client_ts_ns: c.u64()? }),
            msg::PONG => Message::Pong(Pong {
                echoed_client_ts_ns: c.u64()?,
                host_ts_ns: c.u64()?,
            }),
            msg::DISCONNECT => Message::Disconnect(Disconnect {
                reason: c.u16()?,
                message: c.string()?,
            }),
            msg::RECONNECT => Message::Reconnect(Reconnect {
                session_id: c.u64()?,
                last_frame_id: c.u64()?,
            }),
            msg::FRAME_ACK => Message::FrameAck(FrameAck {
                frame_id: c.u64()?,
                client_ts_ns: c.u64()?,
            }),
            other => return Err(Error::UnknownMessageType(other)),
        };
        c.done()?;
        // Semantic validation for fields with registries.
        if let Message::Config(Config { codec: k, .. }) | Message::Frame(Frame { codec: k, .. }) = &m
        {
            if !codec::valid(*k) {
                return Err(Error::BadCodec(*k));
            }
        }
        if let Message::Config(Config { pixel_format: f, .. })
        | Message::Frame(Frame { pixel_format: f, .. })
        | Message::DisplayInfo(DisplayInfo { pixel_format: f, .. }) = &m
        {
            if !pixel_format::valid(*f) {
                return Err(Error::BadPixelFormat(*f));
            }
        }
        Ok(m)
    }
}

impl Default for HelloOk {
    fn default() -> Self {
        HelloOk { proto_version: 0, capabilities: 0, session_id: 0, max_rects: 0 }
    }
}
// (Default kept for API convenience; decode sets every field explicitly.)

// ---------------------------------------------------------------------------
// Stream I/O
// ---------------------------------------------------------------------------

/// Write a full framed message.
pub fn write_message<W: Write + ?Sized>(w: &mut W, m: &Message) -> io::Result<()> {
    w.write_all(&m.to_bytes())
}

/// Read one framed message. `Ok(None)` = peer closed cleanly between
/// messages. Decode failures are `io::ErrorKind::InvalidData`.
pub fn read_message<R: Read + ?Sized>(r: &mut R) -> io::Result<Option<Message>> {
    let mut hbuf = [0u8; HEADER_LEN];
    let mut filled = 0usize;
    while filled < HEADER_LEN {
        let n = r.read(&mut hbuf[filled..])?;
        if n == 0 {
            if filled == 0 {
                return Ok(None); // clean EOF between messages
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed mid-header",
            ));
        }
        filled += n;
    }
    let header = Header::from_bytes(&hbuf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if header.length as usize > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            Error::PayloadTooLarge(header.length),
        ));
    }
    let mut payload = vec![0u8; header.length as usize];
    if !payload.is_empty() {
        r.read_exact(&mut payload)?;
    }
    let m = Message::decode(header.msg_type, &payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(m))
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_rect(out: &mut Vec<u8>, r: &Rect) {
    out.extend_from_slice(&r.x.to_le_bytes());
    out.extend_from_slice(&r.y.to_le_bytes());
    out.extend_from_slice(&r.w.to_le_bytes());
    out.extend_from_slice(&r.h.to_le_bytes());
}

/// Strings are `u8` length + UTF-8 bytes; oversize input is a programmer bug.
fn put_str(out: &mut Vec<u8>, s: &str) {
    assert!(
        s.len() <= MAX_STRING,
        "protocol string exceeds {MAX_STRING} bytes (bug): {s:?}"
    );
    out.push(s.len() as u8);
    out.extend_from_slice(s.as_bytes());
}

struct Cur<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.pos + n > self.buf.len() {
            return Err(Error::Truncated { need: self.pos + n, have: self.buf.len() });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
    fn rect(&mut self) -> Result<Rect, Error> {
        let b = self.take(8)?;
        Ok(Rect {
            x: u16::from_le_bytes([b[0], b[1]]),
            y: u16::from_le_bytes([b[2], b[3]]),
            w: u16::from_le_bytes([b[4], b[5]]),
            h: u16::from_le_bytes([b[6], b[7]]),
        })
    }
    fn string(&mut self) -> Result<String, Error> {
        let len = self.u8()? as usize;
        let b = self.take(len)?;
        String::from_utf8(b.to_vec()).map_err(|_| Error::BadString)
    }
    fn rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.pos..];
        self.pos = self.buf.len();
        s
    }
    fn done(&self) -> Result<(), Error> {
        if self.pos != self.buf.len() {
            Err(Error::Trailing(self.buf.len() - self.pos))
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(m: Message) {
        let wire = m.to_bytes();
        let h = Header::from_bytes(&wire[..HEADER_LEN]).unwrap();
        assert_eq!(h.magic, MAGIC);
        assert_eq!(h.version, VERSION);
        assert_eq!(h.msg_type, m.msg_type());
        assert_eq!(h.length as usize + HEADER_LEN, wire.len());
        let back = Message::decode(h.msg_type, &wire[HEADER_LEN..]).unwrap();
        assert_eq!(back, m, "roundtrip mismatch for {}", m.type_name());
    }

    fn sample_frame() -> Frame {
        Frame {
            frame_id: 42,
            timestamp_ns: 1_700_000_000_000_000_000,
            codec: codec::RAW,
            pixel_format: pixel_format::BGRX,
            rects: vec![
                Rect::new(0, 0, 1600, 900),
                Rect::new(10, 20, 30, 40),
                Rect::new(100, 200, 300, 400),
            ],
            payload: vec![7u8; (1600 * 900 + 30 * 40 + 300 * 400) * 4],
        }
    }

    #[test]
    fn header_roundtrip() {
        let h = Header::new(msg::FRAME, 12345);
        assert_eq!(Header::from_bytes(&h.to_bytes()).unwrap(), h);
        let b = h.to_bytes();
        assert_eq!(&b[0..4], &[0x55, 0x53, 0x42, 0x52]); // "USBR" on the wire
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut b = Header::new(msg::FRAME, 0).to_bytes();
        b[0] = 0x00; // magic bytes become 00 53 42 52
        assert_eq!(Header::from_bytes(&b), Err(Error::BadMagic(0x5242_5300)));
    }

    #[test]
    fn header_rejects_newer_version() {
        let mut b = Header::new(msg::FRAME, 0).to_bytes();
        b[4] = 0x02;
        assert!(matches!(Header::from_bytes(&b), Err(Error::UnsupportedVersion(2))));
    }

    #[test]
    fn all_messages_roundtrip() {
        roundtrip(Message::Hello(Hello {
            proto_version: VERSION,
            capabilities: caps::RAW | caps::ZSTD,
            screen_w: 1080,
            screen_h: 2340,
            refresh_hint: 60,
            name: "Pixel 8a".into(),
        }));
        roundtrip(Message::HelloOk(HelloOk {
            proto_version: VERSION,
            capabilities: caps::RAW,
            session_id: 0xDEAD_BEEF_CAFE,
            max_rects: MAX_RECTS,
        }));
        roundtrip(Message::Config(Config {
            codec: codec::RAW,
            pixel_format: pixel_format::BGRX,
            refresh_hint: 60,
            flags: Config::FLAG_FRAME_ACK,
        }));
        roundtrip(Message::DisplayInfo(DisplayInfo {
            width: 1600,
            height: 900,
            refresh: 60,
            pixel_format: pixel_format::BGRX,
            flags: DisplayInfo::FLAG_CURSOR_EMBEDDED,
            name: "USBra Test Pattern".into(),
        }));
        roundtrip(Message::Frame(sample_frame()));
        roundtrip(Message::DamageRegion(DamageRegion {
            frame_id: 43,
            timestamp_ns: 7,
            rects: vec![Rect::new(1, 2, 3, 4)],
        }));
        roundtrip(Message::Ping(Ping { client_ts_ns: 1234567890 }));
        roundtrip(Message::Pong(Pong { echoed_client_ts_ns: 1234567890, host_ts_ns: 987654321 }));
        roundtrip(Message::Disconnect(Disconnect {
            reason: reason::UNKNOWN_SESSION,
            message: "unknown session".into(),
        }));
        roundtrip(Message::Reconnect(Reconnect { session_id: 99, last_frame_id: 41 }));
        roundtrip(Message::FrameAck(FrameAck { frame_id: 42, client_ts_ns: 555 }));
    }

    #[test]
    fn frame_rect_ranges_match_tight_packing() {
        let f = sample_frame();
        // rect 0: full 1600x900
        let (s0, e0) = f.rect_byte_range(0).unwrap();
        assert_eq!(e0 - s0, 1600 * 900 * 4);
        // rect 1 starts right after rect 0
        let (s1, e1) = f.rect_byte_range(1).unwrap();
        assert_eq!(s1, e0);
        assert_eq!(e1 - s1, 30 * 40 * 4);
        // rect 2 after rect 1
        let (s2, e2) = f.rect_byte_range(2).unwrap();
        assert_eq!(s2, e1);
        assert_eq!(e2, f.payload.len());
        // raw payload length matches
        assert_eq!(f.raw_payload_len() as usize, f.payload.len());
        // full-frame predicate: 3 rects, so not a single full rect
        assert!(!f.covers(1600, 900));
        let full = Frame {
            frame_id: 1,
            timestamp_ns: 2,
            codec: codec::RAW,
            pixel_format: pixel_format::BGRX,
            rects: vec![Rect::new(0, 0, 1600, 900)],
            payload: vec![0; 1600 * 900 * 4],
        };
        assert!(full.covers(1600, 900));
    }

    #[test]
    fn truncated_frame_payload_errors() {
        let mut f = sample_frame();
        f.payload.truncate(10); // drop pixel bytes
        let wire = Message::Frame(f).to_bytes();
        let err = Message::decode(msg::FRAME, &wire[HEADER_LEN..]).unwrap_err();
        // RAW packing check: rects promise 6,244,800 bytes, payload has 10.
        assert_eq!(
            err,
            Error::Truncated {
                need: (1600 * 900 + 30 * 40 + 300 * 400) * 4,
                have: 10
            }
        );
    }

    #[test]
    fn truncated_header_section_errors() {
        let f = Message::Ping(Ping { client_ts_ns: 1 }).to_bytes();
        let err = Message::decode(msg::PING, &f[HEADER_LEN..HEADER_LEN + 4]).unwrap_err();
        assert_eq!(err, Error::Truncated { need: 8, have: 4 });
    }

    #[test]
    fn trailing_bytes_rejected() {
        let mut wire = Message::Config(Config {
            codec: 0,
            pixel_format: 0,
            refresh_hint: 60,
            flags: 0,
        })
        .to_bytes();
        wire.push(0xFF); // one byte too many
        assert_eq!(
            Message::decode(msg::CONFIG, &wire[HEADER_LEN..]),
            Err(Error::Trailing(1))
        );
    }

    #[test]
    fn unknown_type_rejected() {
        assert_eq!(
            Message::decode(999, &[]),
            Err(Error::UnknownMessageType(999))
        );
    }

    #[test]
    fn bad_codec_and_format_rejected() {
        let wire = Message::Config(Config { codec: 9, pixel_format: 0, refresh_hint: 0, flags: 0 })
            .to_bytes();
        assert_eq!(Message::decode(msg::CONFIG, &wire[HEADER_LEN..]), Err(Error::BadCodec(9)));
        let wire = Message::Config(Config { codec: 0, pixel_format: 9, refresh_hint: 0, flags: 0 })
            .to_bytes();
        assert_eq!(
            Message::decode(msg::CONFIG, &wire[HEADER_LEN..]),
            Err(Error::BadPixelFormat(9))
        );
    }

    #[test]
    fn oversize_string_is_programmer_error() {
        let m = Hello {
            proto_version: VERSION,
            capabilities: 0,
            screen_w: 0,
            screen_h: 0,
            refresh_hint: 0,
            name: "x".repeat(256),
        };
        let result = std::panic::catch_unwind(|| Message::Hello(m).to_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn invalid_utf8_string_rejected() {
        let hello_payload = {
            let mut p = Vec::new();
            p.extend_from_slice(&1u16.to_le_bytes()); // proto_version
            p.extend_from_slice(&0u32.to_le_bytes()); // caps
            p.extend_from_slice(&0u16.to_le_bytes()); // screen_w
            p.extend_from_slice(&0u16.to_le_bytes()); // screen_h
            p.extend_from_slice(&0u16.to_le_bytes()); // refresh
            p.push(1); // name_len
            p.push(0xFF); // invalid UTF-8 byte
            p
        };
        assert_eq!(Message::decode(msg::HELLO, &hello_payload), Err(Error::BadString));
    }

    #[test]
    fn stream_write_read_roundtrip() {
        let mut buf: Vec<u8> = Vec::new();
        let msgs = vec![
            Message::Hello(Hello {
                proto_version: VERSION,
                capabilities: caps::RAW,
                screen_w: 1080,
                screen_h: 2340,
                refresh_hint: 60,
                name: "test".into(),
            }),
            Message::Frame(sample_frame()),
            Message::Ping(Ping { client_ts_ns: 42 }),
        ];
        for m in &msgs {
            write_message(&mut buf, m).unwrap();
        }
        let mut cur = std::io::Cursor::new(&buf);
        for m in &msgs {
            let back = read_message(&mut cur).unwrap().unwrap();
            assert_eq!(back, *m);
        }
        // clean EOF → None
        assert!(read_message(&mut cur).unwrap().is_none());
    }

    #[test]
    fn read_rejects_mid_stream_garbage() {
        let mut buf: Vec<u8> = vec![0u8; HEADER_LEN];
        buf[0..4].copy_from_slice(&MAGIC.to_le_bytes()); // valid magic
        buf[8..12].copy_from_slice(&1_000_000_000u32.to_le_bytes()); // absurd length
        let mut cur = std::io::Cursor::new(buf);
        let err = read_message(&mut cur).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rect_intersect_works() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(50, 50, 100, 100);
        assert_eq!(a.intersect(&b), Some(Rect::new(50, 50, 50, 50)));
        let c = Rect::new(200, 200, 10, 10);
        assert_eq!(a.intersect(&c), None);
    }

    #[test]
    fn now_ns_is_sane() {
        let a = now_ns();
        let b = now_ns();
        assert!(b >= a);
        assert!(a > 1_600_000_000_000_000_000); // after Sep 2020
    }
}
