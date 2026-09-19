//! `usbra-host --probe-gnome`: create a real virtual Display 2, hold, tear down.
//!
//! PipeWire frame consumption is still M6b — this proves the D-Bus half and
//! prints the node id. Optionally launches gst-launch like the Python probe
//! so Settings → Displays shows a sized monitor.

use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use crate::args::Args;
use crate::gnome::{cursor, ScreenCastSession, SessionConfig};

pub fn run(a: &Args) -> i32 {
    let cfg = SessionConfig {
        width: a.width,
        height: a.height,
        fps: a.fps,
        is_platform: true,
        cursor_mode: cursor::EMBEDDED,
    };

    eprintln!(
        "usbra-host --probe-gnome: creating RecordVirtual {}x{}@{} (is-platform, cursor=embedded)…",
        cfg.width, cfg.height, cfg.fps
    );

    let mut session = match ScreenCastSession::start(cfg.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to start ScreenCast session: {e}");
            eprintln!("  need a GNOME Wayland session with org.gnome.Mutter.ScreenCast on the bus");
            return 1;
        }
    };

    eprintln!("  session: {}", session.session_path());
    eprintln!("  stream:  {}", session.stream_path());
    eprintln!("  PipeWire node_id={}", session.node_id);

    let mut gst = None;
    if !a.no_gst {
        match spawn_gst(session.node_id, cfg.width, cfg.height, cfg.fps) {
            Ok(child) => {
                eprintln!(
                    "  gst-launch attached (proposes {}x{}@{}) — check Settings → Displays",
                    cfg.width, cfg.height, cfg.fps
                );
                thread::sleep(Duration::from_millis(1500));
                gst = Some(child);
            }
            Err(e) => {
                eprintln!("  warning: could not start gstreamer consumer: {e}");
                eprintln!("  virtual monitor may still appear; size negotiation needs a PW consumer");
            }
        }
    } else {
        eprintln!("  --no-gst: D-Bus only (monitor size may stay unset)");
    }

    let hold = if a.hold_secs == 0 { 20 } else { a.hold_secs };
    eprintln!("  holding for {hold}s — Ctrl-C stops early via process kill (Drop tears down)…");
    let _ = io::stdout().flush();
    thread::sleep(Duration::from_secs(hold));

    if let Some(mut child) = gst {
        let _ = child.kill();
        let _ = child.wait();
    }
    if let Err(e) = session.stop() {
        eprintln!("  warning: Stop() failed: {e}");
    } else {
        eprintln!("  session stopped — Display 2 should be gone");
    }
    0
}

fn spawn_gst(
    node_id: u32,
    width: u16,
    height: u16,
    fps: u32,
) -> io::Result<std::process::Child> {
    Command::new("gst-launch-1.0")
        .args([
            "-q",
            "pipewiresrc",
            &format!("path={node_id}"),
            "do-timestamp=true",
            "!",
            &format!("video/x-raw,width={width},height={height},framerate={fps}/1"),
            "!",
            "videoconvert",
            "!",
            "fakesink",
            "sync=false",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
}
