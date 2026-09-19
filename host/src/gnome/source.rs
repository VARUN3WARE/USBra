//! `GnomeSource`: mutter RecordVirtual + GStreamer PipeWire grabber → `Frame`s.
//!
//! Creates a real Display 2 via [`super::ScreenCastSession`], then spawns
//! `scripts/m6-pw-grab.py` to consume the PipeWire node (BGRx full frames).
//! Damage-region extraction lands when we switch to a native pipewire-rs
//! consumer (`libpipewire-0.3-dev`).

use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use usbra_protocol::{codec, pixel_format, Frame, Rect};

use crate::source::FrameSource;

use super::{cursor, ScreenCastSession, SessionConfig};

const UBF1_MAGIC: u32 = 0x3146_4255; // "UBF1"
const HEADER_LEN: usize = 4 + 8 + 8 + 2 + 2 + 4 + 4; // 32

/// Live GNOME virtual-monitor frame source.
pub struct GnomeSource {
    session: Option<ScreenCastSession>,
    grabber: Option<Child>,
    stdout: Option<ChildStdout>,
    width: u16,
    height: u16,
    stop: Arc<AtomicBool>,
    frame_id: u64,
    /// First frame consumed during `open` (proves the pipeline works).
    pending: Option<Frame>,
}

impl GnomeSource {
    /// Create the virtual monitor and start the grabber. Blocks until the
    /// ScreenCast session is up **and** the first frame has been captured.
    pub fn open(width: u16, height: u16, fps: u32) -> io::Result<GnomeSource> {
        let cfg = SessionConfig {
            width,
            height,
            fps,
            is_platform: true,
            cursor_mode: cursor::EMBEDDED,
        };
        eprintln!(
            "usbra-host: gnome source — RecordVirtual {width}x{height}@{fps} (is-platform)…"
        );
        let session = ScreenCastSession::start(cfg).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("ScreenCast session failed: {e}"),
            )
        })?;
        eprintln!(
            "usbra-host: gnome source — session ok, PipeWire node_id={}",
            session.node_id
        );

        let script = grabber_script_path()?;
        let mut child = Command::new("python3")
            .arg(&script)
            .arg("--node-id")
            .arg(session.node_id.to_string())
            .arg("--width")
            .arg(width.to_string())
            .arg("--height")
            .arg(height.to_string())
            .arg("--fps")
            .arg(fps.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("failed to spawn {}: {e}", script.display()),
                )
            })?;

        let mut stdout = child.stdout.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "grabber stdout missing")
        })?;

        // Block until the first frame (or grabber death / EOF).
        let first = match read_ubf1_frame_once(&mut stdout) {
            Ok(f) => f,
            Err(e) => {
                let status = child.try_wait().ok().flatten();
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "waiting for first gnome frame failed: {e} (grabber status={status:?})"
                    ),
                ));
            }
        };
        let (w, h) = (first.rects[0].w, first.rects[0].h);
        let frame_id = first.frame_id;
        eprintln!(
            "usbra-host: gnome source — first frame {w}x{h} id={frame_id} ({} KiB)",
            first.payload.len() / 1024
        );

        Ok(GnomeSource {
            session: Some(session),
            grabber: Some(child),
            stdout: Some(stdout),
            width: w,
            height: h,
            stop: Arc::new(AtomicBool::new(false)),
            frame_id,
            pending: Some(first),
        })
    }
}

impl FrameSource for GnomeSource {
    fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    fn name(&self) -> &'static str {
        "gnome"
    }

    fn paces_itself(&self) -> bool {
        true
    }

    fn next_frame(&mut self, _timestamp_ns: u64) -> Frame {
        if let Some(f) = self.pending.take() {
            return f;
        }
        let stdout = self
            .stdout
            .as_mut()
            .expect("gnome source stdout after open");
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return blank_frame(self.frame_id.saturating_add(1), self.width, self.height);
            }
            match read_ubf1_frame_once(stdout) {
                Ok(f) => {
                    if f.rects[0].w != self.width || f.rects[0].h != self.height {
                        eprintln!(
                            "usbra-host: gnome size change {}x{} (was {}x{})",
                            f.rects[0].w,
                            f.rects[0].h,
                            self.width,
                            self.height
                        );
                        self.width = f.rects[0].w;
                        self.height = f.rects[0].h;
                    }
                    self.frame_id = f.frame_id;
                    return f;
                }
                Err(e) => {
                    if let Some(child) = self.grabber.as_mut() {
                        if let Ok(Some(status)) = child.try_wait() {
                            eprintln!("usbra-host: gnome grabber exited: {status}");
                            return blank_frame(
                                self.frame_id.saturating_add(1),
                                self.width,
                                self.height,
                            );
                        }
                    }
                    eprintln!("usbra-host: gnome grabber read failed: {e}");
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.grabber.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stdout.take();
        if let Some(mut session) = self.session.take() {
            if let Err(e) = session.stop() {
                eprintln!("usbra-host: ScreenCast Stop failed: {e}");
            } else {
                eprintln!("usbra-host: gnome virtual monitor removed");
            }
        }
    }
}

impl Drop for GnomeSource {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn blank_frame(frame_id: u64, w: u16, h: u16) -> Frame {
    Frame {
        frame_id,
        timestamp_ns: usbra_protocol::now_ns(),
        codec: codec::RAW,
        pixel_format: pixel_format::BGRX,
        rects: vec![Rect::new(0, 0, w, h)],
        payload: vec![0u8; w as usize * h as usize * 4],
    }
}

fn read_ubf1_frame_once(stdout: &mut ChildStdout) -> io::Result<Frame> {
    let mut hdr = [0u8; HEADER_LEN];
    stdout.read_exact(&mut hdr)?;
    let magic = u32::from_le_bytes(hdr[0..4].try_into().unwrap());
    if magic != UBF1_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad UBF1 magic 0x{magic:08x}"),
        ));
    }
    let frame_id = u64::from_le_bytes(hdr[4..12].try_into().unwrap());
    let ts = u64::from_le_bytes(hdr[12..20].try_into().unwrap());
    let w = u16::from_le_bytes(hdr[20..22].try_into().unwrap());
    let h = u16::from_le_bytes(hdr[22..24].try_into().unwrap());
    let plen = u32::from_le_bytes(hdr[28..32].try_into().unwrap()) as usize;
    if plen > 64 * 1024 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut payload = vec![0u8; plen];
    stdout.read_exact(&mut payload)?;
    let expect = w as usize * h as usize * 4;
    if payload.len() != expect {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("payload {} != {w}x{h}*4", payload.len()),
        ));
    }
    Ok(Frame {
        frame_id,
        timestamp_ns: ts,
        codec: codec::RAW,
        pixel_format: pixel_format::BGRX,
        rects: vec![Rect::new(0, 0, w, h)],
        payload,
    })
}

fn grabber_script_path() -> io::Result<PathBuf> {
    // Prefer repo-relative path from the host crate at build time.
    let from_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/m6-pw-grab.py");
    if from_manifest.is_file() {
        return Ok(from_manifest.canonicalize().unwrap_or(from_manifest));
    }
    // Fallback: alongside the running binary (install layouts later).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("m6-pw-grab.py");
            if p.is_file() {
                return Ok(p);
            }
            let p = dir.join("../scripts/m6-pw-grab.py");
            if p.is_file() {
                return Ok(p.canonicalize().unwrap_or(p));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "m6-pw-grab.py not found (expected scripts/m6-pw-grab.py next to the repo)",
    ))
}
