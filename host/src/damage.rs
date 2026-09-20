//! Tile-based damage detection for full-frame BGRX canvases.
//!
//! Used by the GNOME grabber path until native `SPA_META_VideoDamage` is wired.
//! Compares 32×32 tiles and returns merged dirty rectangles (tight-packed
//! payload packing matches the protocol FRAME rules).

use usbra_protocol::Rect;

const TILE: usize = 32;
const BPP: usize = 4;

/// Find dirty regions between two tightly packed BGRX frames of size `w×h`.
/// Returns an empty vec if nothing changed. Caps at `max_rects` by expanding
/// to a full-frame rect when the dirty set would be larger (cheaper to send).
pub fn dirty_rects(prev: &[u8], curr: &[u8], w: u16, h: u16, max_rects: usize) -> Vec<Rect> {
    let w = w as usize;
    let h = h as usize;
    if prev.len() != curr.len() || prev.len() != w * h * BPP {
        return vec![Rect::new(0, 0, w as u16, h as u16)];
    }

    let tiles_x = (w + TILE - 1) / TILE;
    let tiles_y = (h + TILE - 1) / TILE;
    let mut dirty: Vec<Rect> = Vec::new();

    for ty in 0..tiles_y {
        let mut run_x0: Option<usize> = None;
        let y0 = ty * TILE;
        let th = (h - y0).min(TILE);
        for tx in 0..tiles_x {
            let x0 = tx * TILE;
            let tw = (w - x0).min(TILE);
            let changed = tile_changed(prev, curr, w, x0, y0, tw, th);
            if changed {
                if run_x0.is_none() {
                    run_x0 = Some(x0);
                }
            } else if let Some(rx0) = run_x0.take() {
                dirty.push(Rect::new(rx0 as u16, y0 as u16, (x0 - rx0) as u16, th as u16));
            }
        }
        if let Some(rx0) = run_x0 {
            dirty.push(Rect::new(rx0 as u16, y0 as u16, (w - rx0) as u16, th as u16));
        }
    }

    if dirty.is_empty() {
        return dirty;
    }

    // Vertical merge of identical-x-span adjacent rows (cheap coalescing).
    dirty = merge_vertical(dirty);

    if dirty.len() > max_rects {
        return vec![Rect::new(0, 0, w as u16, h as u16)];
    }
    dirty
}

fn tile_changed(
    prev: &[u8],
    curr: &[u8],
    stride_px: usize,
    x0: usize,
    y0: usize,
    tw: usize,
    th: usize,
) -> bool {
    let row_bytes = tw * BPP;
    for y in y0..y0 + th {
        let s = (y * stride_px + x0) * BPP;
        if prev[s..s + row_bytes] != curr[s..s + row_bytes] {
            return true;
        }
    }
    false
}

fn merge_vertical(mut rects: Vec<Rect>) -> Vec<Rect> {
    if rects.len() < 2 {
        return rects;
    }
    rects.sort_by_key(|r| (r.x, r.y));
    let mut out: Vec<Rect> = Vec::with_capacity(rects.len());
    for r in rects {
        if let Some(last) = out.last_mut() {
            if last.x == r.x
                && last.w == r.w
                && last.y as u32 + last.h as u32 == r.y as u32
            {
                last.h = last.h.saturating_add(r.h);
                continue;
            }
        }
        out.push(r);
    }
    out
}

/// Pack tightly packed BGRX rows for `rects` from a full canvas.
pub fn pack_rects(canvas: &[u8], stride_px: usize, rects: &[Rect]) -> Vec<u8> {
    let mut len = 0usize;
    for r in rects {
        len += r.byte_len() as usize;
    }
    let mut out = Vec::with_capacity(len);
    for r in rects {
        let row_len = r.w as usize * BPP;
        for y in r.y as usize..(r.y as usize + r.h as usize) {
            let s = (y * stride_px + r.x as usize) * BPP;
            out.extend_from_slice(&canvas[s..s + row_len]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_frames_no_damage() {
        let w = 64u16;
        let h = 64u16;
        let buf = vec![7u8; w as usize * h as usize * 4];
        assert!(dirty_rects(&buf, &buf, w, h, 64).is_empty());
    }

    #[test]
    fn one_tile_change() {
        let w = 64u16;
        let h = 64u16;
        let a = vec![0u8; w as usize * h as usize * 4];
        let mut b = a.clone();
        // Change top-left 32x32 tile
        for i in 0..(32 * 4) {
            b[i] = 1;
        }
        let d = dirty_rects(&a, &b, w, h, 64);
        assert!(!d.is_empty());
        assert!(d.iter().any(|r| r.x == 0 && r.y == 0));
    }
}
