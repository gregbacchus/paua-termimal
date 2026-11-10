//! Procedural drawing of box-drawing (U+2500..257F) and block-element (U+2580..259F) characters.
//!
//! Font glyphs for these rarely fill the whole cell, which leaves gaps between rows and
//! misaligned joins. Terminals draw them as exact, pixel-snapped shapes instead. Double-line
//! characters (U+2550..256C) are not handled yet and fall back to the font.

use gtk::{gdk, graphene, gsk, prelude::*};

/// Weights per direction: [left, right, up, down]; 0 = none, 1 = light, 2 = heavy.
const BOX: [[u8; 4]; 0x50] = [
    [1,1,0,0],[2,2,0,0],[0,0,1,1],[0,0,2,2],[1,1,0,0],[2,2,0,0],[0,0,1,1],[0,0,2,2],
    [1,1,0,0],[2,2,0,0],[0,0,1,1],[0,0,2,2],[0,1,0,1],[0,2,0,1],[0,1,0,2],[0,2,0,2],
    [1,0,0,1],[2,0,0,1],[1,0,0,2],[2,0,0,2],[0,1,1,0],[0,2,1,0],[0,1,2,0],[0,2,2,0],
    [1,0,1,0],[2,0,1,0],[1,0,2,0],[2,0,2,0],[0,1,1,1],[0,2,1,1],[0,1,2,1],[0,1,1,2],
    [0,1,2,2],[0,2,2,1],[0,2,1,2],[0,2,2,2],[1,0,1,1],[2,0,1,1],[1,0,2,1],[1,0,1,2],
    [1,0,2,2],[2,0,2,1],[2,0,1,2],[2,0,2,2],[1,1,0,1],[2,1,0,1],[1,2,0,1],[2,2,0,1],
    [1,1,0,2],[2,1,0,2],[1,2,0,2],[2,2,0,2],[1,1,1,0],[2,1,1,0],[1,2,1,0],[2,2,1,0],
    [1,1,2,0],[2,1,2,0],[1,2,2,0],[2,2,2,0],[1,1,1,1],[2,1,1,1],[1,2,1,1],[2,2,1,1],
    [1,1,2,1],[1,1,1,2],[1,1,2,2],[2,1,2,1],[1,2,2,1],[2,1,1,2],[1,2,1,2],[2,2,2,1],
    [2,2,1,2],[2,1,2,2],[1,2,2,2],[2,2,2,2],[1,1,0,0],[2,2,0,0],[0,0,1,1],[0,0,2,2],
];

/// U+2574..257F: half lines.
const HALF: [[u8; 4]; 12] = [
    [1,0,0,0],[0,0,1,0],[0,1,0,0],[0,0,0,1],[2,0,0,0],[0,0,2,0],
    [0,2,0,0],[0,0,0,2],[1,2,0,0],[0,0,1,2],[2,1,0,0],[0,0,2,1],
];

fn weights(cp: u32) -> Option<[u8; 4]> {
    match cp {
        0x2500..=0x254F => Some(BOX[(cp - 0x2500) as usize]),
        0x2574..=0x257F => Some(HALF[(cp - 0x2574) as usize]),
        _ => None,
    }
}

/// Whether `draw` will handle this character (so the caller can keep it out of text runs).
pub fn handles(c: char) -> bool {
    let cp = c as u32;
    weights(cp).is_some() || (0x256D..=0x2573).contains(&cp) || (0x2580..=0x259F).contains(&cp)
}

pub struct Cell {
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
    /// Light line thickness in px.
    pub t: f32,
}

fn fill(snap: &gtk::Snapshot, color: &gdk::RGBA, x0: f32, y0: f32, x1: f32, y1: f32) {
    if x1 > x0 && y1 > y0 {
        snap.append_color(color, &graphene::Rect::new(x0, y0, x1 - x0, y1 - y0));
    }
}

/// Pixel-snapped span of thickness `t` centred on `c`.
fn span(c: f32, t: f32) -> (f32, f32) {
    let start = (c - t / 2.0).round();
    (start, start + t)
}

pub fn draw(snap: &gtk::Snapshot, c: char, cell: &Cell, color: &gdk::RGBA) {
    let cp = c as u32;
    if let Some(w) = weights(cp) {
        draw_lines(snap, w, cell, color);
    } else if (0x256D..=0x2570).contains(&cp) {
        draw_arc(snap, cp, cell, color);
    } else if (0x2571..=0x2573).contains(&cp) {
        draw_diagonal(snap, cp, cell, color);
    } else if (0x2580..=0x259F).contains(&cp) {
        draw_block(snap, cp, cell, color);
    }
}

fn draw_lines(snap: &gtk::Snapshot, w: [u8; 4], cell: &Cell, color: &gdk::RGBA) {
    let [l, r, u, d] = w;
    let t = |w: u8| cell.t * f32::from(w);
    let cx = (cell.x0 + cell.x1) / 2.0;
    let cy = (cell.y0 + cell.y1) / 2.0;
    let (vx0, vx1) = span(cx, t(u.max(d)));
    let (hy0, hy1) = span(cy, t(l.max(r)));

    if l > 0 {
        let (a, b) = span(cy, t(l));
        fill(snap, color, cell.x0, a, vx1, b);
    }
    if r > 0 {
        let (a, b) = span(cy, t(r));
        fill(snap, color, vx0, a, cell.x1, b);
    }
    if u > 0 {
        let (a, b) = span(cx, t(u));
        fill(snap, color, a, cell.y0, b, hy1);
    }
    if d > 0 {
        let (a, b) = span(cx, t(d));
        fill(snap, color, a, hy0, b, cell.y1);
    }
}

fn stroke_path(snap: &gtk::Snapshot, build: impl FnOnce(&gsk::PathBuilder), width: f32, color: &gdk::RGBA) {
    let pb = gsk::PathBuilder::new();
    build(&pb);
    snap.append_stroke(&pb.to_path(), &gsk::Stroke::new(width), color);
}

/// Rounded corners ╭ ╮ ╯ ╰ drawn as a quarter-circle joining the two arms.
fn draw_arc(snap: &gtk::Snapshot, cp: u32, cell: &Cell, color: &gdk::RGBA) {
    let t = cell.t;
    let (vx0, vx1) = span((cell.x0 + cell.x1) / 2.0, t);
    let (hy0, hy1) = span((cell.y0 + cell.y1) / 2.0, t);
    let (cx, cy) = ((vx0 + vx1) / 2.0, (hy0 + hy1) / 2.0);
    let rad = ((cell.x1 - cell.x0) / 2.0).min((cell.y1 - cell.y0) / 2.0);
    let k = 0.5523 * rad;

    // (horizontal direction, vertical direction): +1 = right/down, -1 = left/up.
    let (hx, vy) = match cp {
        0x256D => (1.0, 1.0),   // ╭ right + down
        0x256E => (-1.0, 1.0),  // ╮ left + down
        0x256F => (-1.0, -1.0), // ╯ left + up
        _ => (1.0, -1.0),       // ╰ right + up
    };
    let v_end = if vy > 0.0 { cell.y1 } else { cell.y0 };
    let h_end = if hx > 0.0 { cell.x1 } else { cell.x0 };

    stroke_path(
        snap,
        |pb| {
            pb.move_to(cx, v_end);
            pb.line_to(cx, cy + vy * rad);
            pb.cubic_to(cx, cy + vy * (rad - k), cx + hx * (rad - k), cy, cx + hx * rad, cy);
            pb.line_to(h_end, cy);
        },
        t,
        color,
    );
}

fn draw_diagonal(snap: &gtk::Snapshot, cp: u32, cell: &Cell, color: &gdk::RGBA) {
    let (x0, x1, y0, y1) = (cell.x0, cell.x1, cell.y0, cell.y1);
    stroke_path(
        snap,
        |pb| {
            if cp != 0x2572 {
                // ╱ (and the first stroke of ╳)
                pb.move_to(x0, y1);
                pb.line_to(x1, y0);
            }
            if cp != 0x2571 {
                // ╲
                pb.move_to(x0, y0);
                pb.line_to(x1, y1);
            }
        },
        cell.t,
        color,
    );
}

fn draw_block(snap: &gtk::Snapshot, cp: u32, cell: &Cell, color: &gdk::RGBA) {
    let (x0, x1, y0, y1) = (cell.x0, cell.x1, cell.y0, cell.y1);
    let (w, h) = (x1 - x0, y1 - y0);
    let mx = x0 + (w / 2.0).round();
    let my = y0 + (h / 2.0).round();
    let eighth = |n: u32, len: f32| (len * n as f32 / 8.0).round();

    match cp {
        0x2580 => fill(snap, color, x0, y0, x1, my),
        0x2581..=0x2588 => fill(snap, color, x0, y1 - eighth(cp - 0x2580, h), x1, y1),
        0x2589..=0x258F => fill(snap, color, x0, y0, x0 + eighth(0x2590 - cp, w), y1),
        0x2590 => fill(snap, color, mx, y0, x1, y1),
        0x2591..=0x2593 => {
            let alpha = (cp - 0x2590) as f32 * 0.25;
            let mut c = *color;
            c.set_alpha(color.alpha() * alpha);
            fill(snap, &c, x0, y0, x1, y1);
        }
        0x2594 => fill(snap, color, x0, y0, x1, y0 + eighth(1, h)),
        0x2595 => fill(snap, color, x1 - eighth(1, w), y0, x1, y1),
        0x2596..=0x259F => {
            // Quadrant bits: UL=1, UR=2, LL=4, LR=8.
            let mask = [4, 8, 1, 1 | 4 | 8, 1 | 8, 1 | 2 | 4, 1 | 2 | 8, 2, 2 | 4, 2 | 4 | 8][(cp - 0x2596) as usize];
            if mask & 1 != 0 { fill(snap, color, x0, y0, mx, my); }
            if mask & 2 != 0 { fill(snap, color, mx, y0, x1, my); }
            if mask & 4 != 0 { fill(snap, color, x0, my, mx, y1); }
            if mask & 8 != 0 { fill(snap, color, mx, my, x1, y1); }
        }
        _ => {}
    }
}
