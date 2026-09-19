//! JSONL stats + 1 Hz stderr summary.

use std::fs::File;
use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

pub struct Stats {
    start: Instant,
    file: Option<Mutex<File>>,
    inner: Mutex<Inner>,
}

struct Inner {
    frames: u64,
    bytes: u64,
    window_frames: u64,
    window_bytes: u64,
    window_start: Instant,
}

impl Stats {
    pub fn new(path: Option<&str>) -> std::io::Result<Stats> {
        let file = match path {
            Some(p) => Some(Mutex::new(File::create(p)?)),
            None => None,
        };
        Ok(Stats {
            start: Instant::now(),
            file,
            inner: Mutex::new(Inner {
                frames: 0,
                bytes: 0,
                window_frames: 0,
                window_bytes: 0,
                window_start: Instant::now(),
            }),
        })
    }

    /// One JSONL line: `{"t_ms":…,"ev":"…",…}`.
    pub fn event(&self, kind: &str, fields: &[(&str, String)]) {
        let t_ms = self.start.elapsed().as_millis();
        let mut line = format!("{{\"t_ms\":{t_ms},\"ev\":{}", json_str(kind));
        for (k, v) in fields {
            line.push(',');
            line.push_str(&json_str(k));
            line.push(':');
            line.push_str(&json_str(v));
        }
        line.push('}');
        if let Some(f) = &self.file {
            if let Ok(mut f) = f.lock() {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    /// Frame event + rolling 1 Hz stderr summary.
    pub fn frame(&self, frame_id: u64, rects: u64, bytes: u64, gen_us: u64, send_us: u64) {
        self.event(
            "frame",
            &[
                ("id", frame_id.to_string()),
                ("rects", rects.to_string()),
                ("bytes", bytes.to_string()),
                ("gen_us", gen_us.to_string()),
                ("send_us", send_us.to_string()),
            ],
        );
        let mut g = self.inner.lock().unwrap();
        g.frames += 1;
        g.bytes += bytes;
        g.window_frames += 1;
        g.window_bytes += bytes;
        let el = g.window_start.elapsed();
        if el.as_secs_f64() >= 1.0 {
            let fps = g.window_frames as f64 / el.as_secs_f64();
            let mbps = g.window_bytes as f64 / el.as_secs_f64() / (1024.0 * 1024.0);
            eprintln!(
                "[stats] {fps:6.1} fps  {mbps:7.2} MB/s  (total {} frames, {:.1} MB)",
                g.frames,
                g.bytes as f64 / (1024.0 * 1024.0)
            );
            g.window_frames = 0;
            g.window_bytes = 0;
            g.window_start = Instant::now();
        }
    }
}

/// Minimal JSON string escaping (we only ever emit our own strings + client
/// names, but be correct anyway).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
