//! `usbra-host --selftest`: benchmark frame production without any network
//! or display. This is Milestone 2's measurement tool.

use std::time::Instant;

use usbra_protocol::{now_ns, Message};

use crate::args::Args;
use crate::testsource::TestSource;

pub fn run(a: &Args) {
    let mut src = TestSource::new(a.width, a.height, a.fps, a.full_every_secs);
    let frames = a.frames as usize;

    let mut gen_us: Vec<u128> = Vec::with_capacity(frames);
    let mut wire_bytes: u64 = 0;
    let mut payload_bytes: u64 = 0;
    let mut full_frames: u64 = 0;

    let start = Instant::now();
    for _ in 0..frames {
        let t0 = Instant::now();
        let f = src.next_frame(now_ns());
        payload_bytes += f.payload.len() as u64;
        let is_full = f.covers(a.width, a.height);
        let wire = Message::Frame(f).to_bytes();
        gen_us.push(t0.elapsed().as_micros());
        wire_bytes += wire.len() as u64;
        if is_full {
            full_frames += 1;
        }
    }
    let wall = start.elapsed();

    gen_us.sort_unstable();
    let p = |q: f64| pct(&gen_us, q);
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);

    println!(
        "USBra selftest — {} frames at {}x{} (target {} fps, full-frame every {} s)",
        frames, a.width, a.height, a.fps, a.full_every_secs
    );
    println!("  wall time          : {:.2} s ({:.0} frames/s sustained)", wall.as_secs_f64(),
        frames as f64 / wall.as_secs_f64());
    println!(
        "  generate+encode µs : p50={} p95={} p99={} max={}   (M2 target: p95 < 2000)",
        p(0.50), p(0.95), p(0.99), gen_us[gen_us.len() - 1]
    );
    println!(
        "  frames             : {} full, {} damage",
        full_frames,
        frames as u64 - full_frames
    );
    println!(
        "  bytes              : payload {:.2} MB, wire {:.2} MB (avg {:.1} KiB/frame)",
        mb(payload_bytes),
        mb(wire_bytes),
        payload_bytes as f64 / frames as f64 / 1024.0
    );
    let full_frame_bytes = a.width as u64 * a.height as u64 * 4;
    println!(
        "  full frame size    : {:.2} MB → at {} fps raw full-motion would need {:.0} MB/s \
         (USB2 carries ~35 MB/s — damage regions are what make this work)",
        mb(full_frame_bytes),
        a.fps,
        full_frame_bytes as f64 * a.fps as f64 / (1024.0 * 1024.0)
    );
}

fn pct(sorted: &[u128], q: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}
