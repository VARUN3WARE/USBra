//! Test-pattern frame source.
//!
//! Synthesizes an animated "desktop" (gradient background, bouncing box and
//! square, sweeping bar) with **true damage rectangles**, so transport and
//! rendering can be built and measured before any compositor integration
//! (M2–M5 of the roadmap). Output is BGRX (DRM XRGB8888 byte order), matching
//! what the GNOME/evdi backends will produce later.

use usbra_protocol::{codec, pixel_format, Frame, Rect};

const BOX_COLOR: [u8; 4] = [40, 60, 230, 255]; // BGRX → red box
const SQ_COLOR: [u8; 4] = [200, 140, 40, 255]; // BGRX → blue square
const BAR_COLOR: [u8; 4] = [235, 235, 235, 255]; // near-white sweep bar

pub struct TestSource {
    w: usize,
    h: usize,
    fps: u32,
    full_every_secs: u64,
    bg: Vec<u8>,
    canvas: Vec<u8>,
    frame_id: u64,
    tick: u64,
    box_x: i32,
    box_y: i32,
    box_dx: i32,
    box_dy: i32,
    box_w: usize,
    box_h: usize,
    sq_x: i32,
    sq_y: i32,
    sq_dx: i32,
    sq_dy: i32,
    sq_s: usize,
    bar_x: i32,
    bar_w: usize,
}

impl TestSource {
    pub fn new(width: u16, height: u16, fps: u32, full_every_secs: u64) -> TestSource {
        let w = width as usize;
        let h = height as usize;
        let mut bg = vec![0u8; w * h * 4];
        for y in 0..h {
            let b = (y * 255 / h.max(1)) as u8;
            let r = 255u8.saturating_sub(b);
            for x in 0..w {
                let g = (x * 255 / w.max(1)) as u8;
                let i = (y * w + x) * 4;
                bg[i] = b;
                bg[i + 1] = g;
                bg[i + 2] = r;
                bg[i + 3] = 255;
            }
        }
        let box_w = w.min(128);
        let box_h = h.min(96);
        let sq_s = 32.min(w).min(h);
        TestSource {
            w,
            h,
            fps,
            full_every_secs,
            canvas: bg.clone(),
            bg,
            frame_id: 0,
            tick: 0,
            box_x: w as i32 / 3,
            box_y: h as i32 / 3,
            box_dx: 7,
            box_dy: 5,
            box_w,
            box_h,
            sq_x: w as i32 / 2,
            sq_y: h as i32 / 2,
            sq_dx: -5,
            sq_dy: 8,
            sq_s,
            bar_x: 0,
            bar_w: 8,
        }
    }

    /// Clip an (x, y, w, h) rect in canvas coordinates; zero-area if outside.
    fn clip(&self, x: i32, y: i32, w: usize, h: usize) -> Rect {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w as i32).min(self.w as i32);
        let y1 = (y + h as i32).min(self.h as i32);
        if x1 <= x0 || y1 <= y0 {
            Rect::new(0, 0, 0, 0)
        } else {
            Rect::new(x0 as u16, y0 as u16, (x1 - x0) as u16, (y1 - y0) as u16)
        }
    }

    fn advance(&mut self) {
        self.box_x += self.box_dx;
        self.box_y += self.box_dy;
        if self.box_x <= 0 || self.box_x + self.box_w as i32 >= self.w as i32 {
            self.box_dx = -self.box_dx;
            self.box_x = self.box_x.clamp(0, (self.w - self.box_w) as i32);
        }
        if self.box_y <= 0 || self.box_y + self.box_h as i32 >= self.h as i32 {
            self.box_dy = -self.box_dy;
            self.box_y = self.box_y.clamp(0, (self.h - self.box_h) as i32);
        }
        self.sq_x += self.sq_dx;
        self.sq_y += self.sq_dy;
        if self.sq_x <= 0 || self.sq_x + self.sq_s as i32 >= self.w as i32 {
            self.sq_dx = -self.sq_dx;
            self.sq_x = self.sq_x.clamp(0, (self.w - self.sq_s) as i32);
        }
        if self.sq_y <= 0 || self.sq_y + self.sq_s as i32 >= self.h as i32 {
            self.sq_dy = -self.sq_dy;
            self.sq_y = self.sq_y.clamp(0, (self.h - self.sq_s) as i32);
        }
        let span = (self.w as i32 - self.bar_w as i32).max(1);
        self.bar_x = (self.bar_x + 4) % span;
    }

    /// Restore a rect to the background gradient.
    fn erase(&mut self, r: Rect) {
        if r.w == 0 || r.h == 0 {
            return;
        }
        let len = r.w as usize * 4;
        for y in r.y as usize..(r.y + r.h) as usize {
            let s = (y * self.w + r.x as usize) * 4;
            let bg = &self.bg[s..s + len];
            self.canvas[s..s + len].copy_from_slice(bg);
        }
    }

    fn fill(&mut self, r: Rect, color: [u8; 4]) {
        if r.w == 0 || r.h == 0 {
            return;
        }
        for y in r.y as usize..(r.y + r.h) as usize {
            let row = y * self.w;
            for x in r.x as usize..(r.x + r.w) as usize {
                let i = (row + x) * 4;
                self.canvas[i..i + 4].copy_from_slice(&color);
            }
        }
    }

    /// Produce the next frame. Damage = old ∪ new positions of every moving
    /// element; a full frame on frame 1 and every `full_every_secs`.
    pub fn next_frame(&mut self, timestamp_ns: u64) -> Frame {
        self.frame_id += 1;
        self.tick += 1;

        let old_box = self.clip(self.box_x, self.box_y, self.box_w, self.box_h);
        let old_sq = self.clip(self.sq_x, self.sq_y, self.sq_s, self.sq_s);
        let old_bar = self.clip(self.bar_x, 0, self.bar_w, self.h);
        self.advance();
        let new_box = self.clip(self.box_x, self.box_y, self.box_w, self.box_h);
        let new_sq = self.clip(self.sq_x, self.sq_y, self.sq_s, self.sq_s);
        let new_bar = self.clip(self.bar_x, 0, self.bar_w, self.h);

        let full = self.frame_id == 1
            || (self.full_every_secs > 0
                && self.tick % (self.fps.max(1) as u64 * self.full_every_secs) == 0);

        let rects: Vec<Rect> = if full {
            vec![Rect::new(0, 0, self.w as u16, self.h as u16)]
        } else {
            let mut v = Vec::with_capacity(6);
            for r in [old_box, old_sq, old_bar, new_box, new_sq, new_bar] {
                if r.w > 0 && r.h > 0 && !v.contains(&r) {
                    v.push(r);
                }
            }
            v
        };

        // Update the canvas within the damage regions: erase, then redraw the
        // elements intersecting each region (over-draw of identical pixels is
        // harmless; what matters is that every damaged pixel ends up correct).
        for r in &rects {
            self.erase(*r);
        }
        for r in &rects {
            for (er, color) in
                [(new_box, BOX_COLOR), (new_sq, SQ_COLOR), (new_bar, BAR_COLOR)]
            {
                if let Some(ir) = r.intersect(&er) {
                    self.fill(ir, color);
                }
            }
        }

        // Pack payload: tightly packed rows per rect, rect order (see protocol).
        let mut payload_len = 0usize;
        for r in &rects {
            payload_len += r.byte_len() as usize;
        }
        let mut payload = Vec::with_capacity(payload_len);
        for r in &rects {
            let row_len = r.w as usize * 4;
            for y in r.y as usize..(r.y + r.h) as usize {
                let s = (y * self.w + r.x as usize) * 4;
                payload.extend_from_slice(&self.canvas[s..s + row_len]);
            }
        }

        Frame {
            frame_id: self.frame_id,
            timestamp_ns,
            codec: codec::RAW,
            pixel_format: pixel_format::BGRX,
            rects,
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_frame_is_full_and_matches_packing() {
        let mut src = TestSource::new(320, 180, 60, 0);
        let f = src.next_frame(1);
        assert!(f.covers(320, 180));
        assert_eq!(f.payload.len(), 320 * 180 * 4);
        // payload rows must equal canvas rows — spot check first pixel
        assert_eq!(&f.payload[0..4], &[0, 0, 255, 255]); // top-left: B=0 G=0 R=255
    }

    #[test]
    fn damage_frames_are_partial_and_consistent() {
        let mut src = TestSource::new(320, 180, 60, 0);
        let _ = src.next_frame(1); // full
        let f = src.next_frame(2);
        assert!(!f.rects.is_empty());
        assert!(f.rects.len() <= 6);
        assert!(!f.covers(320, 180));
        let expect: usize = f.rects.iter().map(|r| r.byte_len() as usize).sum();
        assert_eq!(expect, f.payload.len());
        // rects stay inside the canvas
        for r in &f.rects {
            assert!(r.x + r.w <= 320);
            assert!(r.y + r.h <= 180);
        }
    }

    #[test]
    fn periodic_full_frame() {
        let mut src = TestSource::new(320, 180, 60, 1); // full every 1s = 60 ticks
        let mut fulls = 0;
        for i in 0..125 {
            let f = src.next_frame(i as u64);
            if f.covers(320, 180) {
                fulls += 1;
            }
        }
        assert_eq!(fulls, 3, "frame 1 + tick 60 + tick 120");
    }

    #[test]
    fn colors_are_primaries_for_format_debugging() {
        // Red box must be pure red in BGRX memory order: B=40? — no: BOX_COLOR
        // has B=40 G=60 R=230; a *pure* red check lives in the client. Here we
        // just pin the constants so accidental swizzle changes get caught.
        assert_eq!(BOX_COLOR[2], 230); // R
        assert_eq!(SQ_COLOR[0], 200); // B
    }
}
