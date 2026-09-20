//! TCP protocol server: handshake, frame producer, PING/PONG, reconnect.
//!
//! Threading per connection:
//!
//! ```text
//! handle_conn (reader) ── PING→Pong, DISCONNECT→Bye, FRAME_ACK→stats ──┐
//! producer thread ── paced FrameSource frames ─────────────────────────┼──► writer thread ──► socket
//!                                                    stop flag ◄──────┘
//! ```

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use usbra_protocol as proto;

use crate::source::FrameSource;
use crate::stats::Stats;
use crate::testsource::TestSource;

#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub bind: String,
    pub port: u16,
    pub width: u16,
    pub height: u16,
    pub fps: u32,
    pub full_every_secs: u64,
    pub stats_path: Option<String>,
    pub frame_ack: bool,
    /// `test` or `gnome` (gnome requires `--features gnome`).
    pub source: String,
}

impl ServeConfig {
    fn print(&self, addr: &SocketAddr) {
        eprintln!(
            "usbra-host: listening on {addr} (source={} {}x{}@{}fps, \
             full-frame every {}s{})",
            self.source,
            self.width,
            self.height,
            self.fps,
            self.full_every_secs,
            if self.bind.starts_with("127.") || self.bind.starts_with("localhost") {
                " — loopback only, pair with: adb reverse tcp:PORT tcp:PORT"
            } else {
                " — WARNING: bound beyond loopback!"
            }
        );
    }
}

/// Sessions the host will resume via RECONNECT, plus a bounded window of
/// recent frame timestamps for host-side FRAME_ACK latency computation.
#[derive(Default)]
struct SessionTable {
    sessions: HashMap<u64, u64>,   // session_id → last_frame_id
    frames: VecDeque<(u64, u64)>,  // (frame_id, timestamp_ns)
}

impl SessionTable {
    fn note_session(&mut self, id: u64) {
        self.sessions.entry(id).or_insert(0);
    }
    fn has_session(&self, id: u64) -> bool {
        self.sessions.contains_key(&id)
    }
    fn note_frame(&mut self, session_id: u64, frame_id: u64, ts: u64) {
        self.sessions.insert(session_id, frame_id);
        self.frames.push_back((frame_id, ts));
        while self.frames.len() > 512 {
            self.frames.pop_front();
        }
    }
    fn frame_ts(&self, frame_id: u64) -> Option<u64> {
        self.frames.iter().rev().find(|(id, _)| *id == frame_id).map(|(_, ts)| *ts)
    }
}

enum OutMsg {
    Frame { frame: proto::Frame, gen_us: u64 },
    Pong(proto::Pong),
    Bye(proto::Disconnect),
}

/// Run the accept loop on the current thread (used by `main`).
pub fn serve(cfg: &ServeConfig) -> io::Result<()> {
    let listener = TcpListener::bind((cfg.bind.as_str(), cfg.port))?;
    let addr = listener.local_addr()?;
    cfg.print(&addr);
    let stats = Arc::new(Stats::new(cfg.stats_path.as_deref())?);
    let table = Arc::new(Mutex::new(SessionTable::default()));
    accept_loop(listener, cfg, &stats, &table)
}

/// Start the server on an ephemeral port (used by integration tests).
pub fn spawn(cfg: &ServeConfig) -> io::Result<(SocketAddr, JoinHandle<io::Result<()>>)> {
    let listener = TcpListener::bind((cfg.bind.as_str(), 0))?;
    let addr = listener.local_addr()?;
    let stats = Arc::new(Stats::new(cfg.stats_path.as_deref())?);
    let table = Arc::new(Mutex::new(SessionTable::default()));
    let cfg = cfg.clone();
    let h = thread::Builder::new()
        .name("usbra-accept".into())
        .spawn(move || accept_loop(listener, &cfg, &stats, &table))?;
    Ok((addr, h))
}

fn accept_loop(
    listener: TcpListener,
    cfg: &ServeConfig,
    stats: &Arc<Stats>,
    table: &Arc<Mutex<SessionTable>>,
) -> io::Result<()> {
    for conn in listener.incoming() {
        let stream = conn?;
        let cfg = cfg.clone();
        let stats = stats.clone();
        let table = table.clone();
        thread::Builder::new()
            .name("usbra-conn".into())
            .spawn(move || {
                if let Err(e) = handle_conn(stream, &cfg, &stats, &table) {
                    stats.event("conn_end", &[("error", e.to_string())]);
                }
            })?;
    }
    Ok(())
}

fn handle_conn(
    stream: TcpStream,
    cfg: &ServeConfig,
    stats: &Arc<Stats>,
    table: &Arc<Mutex<SessionTable>>,
) -> io::Result<()> {
    let peer = stream.peer_addr()?;
    stream.set_nodelay(true)?;
    let mut reader = stream.try_clone()?;
    let mut writer = stream.try_clone()?;

    stats.event("conn_open", &[("peer", peer.to_string())]);

    // ---- handshake (10 s budget) ----
    reader.set_read_timeout(Some(Duration::from_secs(10)))?;
    let session_id: u64;
    let resumed: bool;
    match proto::read_message(&mut reader)? {
        None => {
            stats.event("conn_end", &[("why", "eof-before-hello".into())]);
            return Ok(());
        }
        Some(proto::Message::Hello(h)) => {
            if h.proto_version != proto::VERSION {
                let _ = proto::write_message(
                    &mut writer,
                    &proto::Message::Disconnect(proto::Disconnect {
                        reason: proto::reason::PROTOCOL,
                        message: format!(
                            "host speaks v{}, client announced v{}",
                            proto::VERSION,
                            h.proto_version
                        ),
                    }),
                );
                return Ok(());
            }
            session_id = new_session_id();
            resumed = false;
            stats.event(
                "hello",
                &[
                    ("name", h.name.clone()),
                    ("caps", h.capabilities.to_string()),
                    ("screen", format!("{}x{}", h.screen_w, h.screen_h)),
                    ("refresh", h.refresh_hint.to_string()),
                ],
            );
        }
        Some(proto::Message::Reconnect(r)) => {
            if table.lock().unwrap().has_session(r.session_id) {
                session_id = r.session_id;
                resumed = true;
                stats.event(
                    "reconnect",
                    &[
                        ("session", session_id.to_string()),
                        ("last_frame", r.last_frame_id.to_string()),
                    ],
                );
            } else {
                let _ = proto::write_message(
                    &mut writer,
                    &proto::Message::Disconnect(proto::Disconnect {
                        reason: proto::reason::UNKNOWN_SESSION,
                        message: "unknown session; send a fresh HELLO".into(),
                    }),
                );
                return Ok(());
            }
        }
        Some(other) => {
            stats.event("protocol_violation", &[("got", other.type_name().into())]);
            let _ = proto::write_message(
                &mut writer,
                &proto::Message::Disconnect(proto::Disconnect {
                    reason: proto::reason::PROTOCOL,
                    message: format!("expected HELLO or RECONNECT, got {}", other.type_name()),
                }),
            );
            return Ok(());
        }
    }
    table.lock().unwrap().note_session(session_id);

    // ---- reply: HELLO_OK + DISPLAY_INFO + CONFIG ----
    proto::write_message(
        &mut writer,
        &proto::Message::HelloOk(proto::HelloOk {
            proto_version: proto::VERSION,
            capabilities: proto::caps::RAW, // test source: raw only for now
            session_id,
            max_rects: proto::MAX_RECTS,
        }),
    )?;
    proto::write_message(
        &mut writer,
        &proto::Message::DisplayInfo(proto::DisplayInfo {
            width: cfg.width,
            height: cfg.height,
            refresh: cfg.fps as u16,
            pixel_format: proto::pixel_format::BGRX,
            flags: proto::DisplayInfo::FLAG_CURSOR_EMBEDDED, // pattern contains everything
            name: if cfg.source == "gnome" {
                "USBra Display".into()
            } else {
                "USBra Test Pattern".into()
            },
        }),
    )?;
    proto::write_message(
        &mut writer,
        &proto::Message::Config(proto::Config {
            codec: proto::codec::RAW,
            pixel_format: proto::pixel_format::BGRX,
            refresh_hint: cfg.fps as u16,
            flags: if cfg.frame_ack { proto::Config::FLAG_FRAME_ACK } else { 0 },
        }),
    )?;
    reader.set_read_timeout(None)?;

    // ---- pipeline ----
    // Bounded queue: if USB/writer is busy, the producer drops frames instead of
    // building multi-second latency. Capacity 2 ≈ one in-flight + one ready.
    let (tx, rx) = mpsc::sync_channel::<OutMsg>(2);
    let stop = Arc::new(AtomicBool::new(false));

    let producer = {
        let cfg = cfg.clone();
        let tx = tx.clone();
        let stop = stop.clone();
        thread::Builder::new().name("usbra-producer".into()).spawn(move || {
            produce_frames(cfg, tx, stop);
        })?
    };
    let writer_h = {
        let stats = stats.clone();
        let table = table.clone();
        thread::Builder::new().name("usbra-writer".into()).spawn(move || {
            writer_loop(writer, rx, session_id, &stats, &table);
        })?
    };

    // ---- reader loop (this thread) ----
    loop {
        match proto::read_message(&mut reader) {
            Ok(Some(proto::Message::Ping(p))) => {
                stats.event("ping", &[("client_ts_ns", p.client_ts_ns.to_string())]);
                let _ = tx.try_send(OutMsg::Pong(proto::Pong {
                    echoed_client_ts_ns: p.client_ts_ns,
                    host_ts_ns: proto::now_ns(),
                }));
            }
            Ok(Some(proto::Message::FrameAck(a))) => {
                let latency_us = table
                    .lock()
                    .unwrap()
                    .frame_ts(a.frame_id)
                    .map(|ts| proto::now_ns().saturating_sub(ts) / 1000)
                    .unwrap_or(0);
                stats.event(
                    "ack",
                    &[
                        ("frame", a.frame_id.to_string()),
                        ("latency_us", latency_us.to_string()),
                    ],
                );
            }
            Ok(Some(proto::Message::Disconnect(d))) => {
                stats.event(
                    "client_disconnect",
                    &[("reason", d.reason.to_string()), ("msg", d.message)],
                );
                let _ = tx.try_send(OutMsg::Bye(proto::Disconnect {
                    reason: proto::reason::SHUTDOWN,
                    message: "bye".into(),
                }));
                break;
            }
            Ok(Some(other)) => {
                stats.event("protocol_violation", &[("got", other.type_name().into())]);
                let _ = tx.try_send(OutMsg::Bye(proto::Disconnect {
                    reason: proto::reason::PROTOCOL,
                    message: format!("unexpected {}", other.type_name()),
                }));
                break;
            }
            Ok(None) => break, // clean EOF
            Err(e) => {
                stats.event("conn_end", &[("why", format!("read: {e}"))]);
                break;
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    drop(tx);
    let _ = producer.join();
    let _ = writer_h.join();
    stats.event(
        "conn_end",
        &[
            ("session", session_id.to_string()),
            ("resumed", resumed.to_string()),
        ],
    );
    Ok(())
}

fn produce_frames(cfg: ServeConfig, tx: mpsc::SyncSender<OutMsg>, stop: Arc<AtomicBool>) {
    let mut src: Box<dyn FrameSource> = match open_source(&cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("usbra-host: failed to open source '{}': {e}", cfg.source);
            return;
        }
    };
    eprintln!("usbra-host: producer using source={}", src.name());
    let period = Duration::from_nanos(1_000_000_000 / cfg.fps.clamp(1, 240) as u64);
    let mut next = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if !src.paces_itself() {
            next += period;
            let now = Instant::now();
            if next > now {
                thread::sleep(next - now);
            } else {
                next = now; // fell behind; reset cadence instead of bursting
            }
        }
        let t0 = Instant::now();
        let frame = src.next_frame(proto::now_ns());
        let gen_us = t0.elapsed().as_micros() as u64;
        match tx.try_send(OutMsg::Frame { frame, gen_us }) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                // Writer still flushing prior frame(s) — stay realtime, skip.
            }
            Err(mpsc::TrySendError::Disconnected(_)) => break,
        }
    }
    src.shutdown();
}

fn open_source(cfg: &ServeConfig) -> io::Result<Box<dyn FrameSource>> {
    match cfg.source.as_str() {
        "test" => Ok(Box::new(TestSource::new(
            cfg.width,
            cfg.height,
            cfg.fps,
            cfg.full_every_secs,
        ))),
        "gnome" => {
            #[cfg(feature = "gnome")]
            {
                Ok(Box::new(crate::gnome::GnomeSource::open(
                    cfg.width,
                    cfg.height,
                    cfg.fps,
                    cfg.full_every_secs,
                )?))
            }
            #[cfg(not(feature = "gnome"))]
            {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "source 'gnome' requires: cargo run --features gnome -- --source gnome",
                ))
            }
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown source '{other}'"),
        )),
    }
}

fn writer_loop(
    mut w: TcpStream,
    rx: mpsc::Receiver<OutMsg>,
    session_id: u64,
    stats: &Stats,
    table: &Arc<Mutex<SessionTable>>,
) {
    while let Ok(m) = rx.recv() {
        match m {
            OutMsg::Frame { frame, gen_us } => {
                let id = frame.frame_id;
                let ts = frame.timestamp_ns;
                let bytes = frame.payload.len();
                let rects = frame.rects.len();
                let t0 = Instant::now();
                if proto::write_message(&mut w, &proto::Message::Frame(frame)).is_err() {
                    break;
                }
                let send_us = t0.elapsed().as_micros() as u64;
                table.lock().unwrap().note_frame(session_id, id, ts);
                stats.frame(id, rects as u64, bytes as u64, gen_us, send_us);
            }
            OutMsg::Pong(p) => {
                if proto::write_message(&mut w, &proto::Message::Pong(p)).is_err() {
                    break;
                }
            }
            OutMsg::Bye(d) => {
                let _ = proto::write_message(&mut w, &proto::Message::Disconnect(d));
                break;
            }
        }
    }
    let _ = w.shutdown(Shutdown::Both);
}

/// Non-zero, hard-to-guess session ids from the clock and a golden-ratio
/// counter step (adequate for loopback-local session tokens).
fn new_session_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let c = COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    let id = proto::now_ns() ^ c.rotate_left(32);
    if id == 0 {
        1
    } else {
        id
    }
}
