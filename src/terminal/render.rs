//! Reading libghostty-vt render state into plain data, and drawing it with GTK snapshots + Pango.
//!
//! Ligatures: each run of cells with identical styling is laid out as ONE Pango layout, so
//! HarfBuzz (via Pango) can apply OpenType `liga`/`calt`. Wide glyphs get their own run so the
//! grid alignment stays exact. Runs are positioned at `col * cell_w`.

use gtk::{gdk, graphene, gsk, pango, prelude::*};
use libghostty_vt::{
    RenderState, Terminal,
    render::{CellIterator, CursorVisualStyle, RowIterator},
    screen::CellWide,
    style::{RgbColor, Underline},
};

use super::{State, boxdraw, scrollbar};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl From<RgbColor> for Rgb {
    fn from(c: RgbColor) -> Self {
        Rgb(c.r, c.g, c.b)
    }
}

impl From<Rgb> for RgbColor {
    fn from(c: Rgb) -> Self {
        RgbColor { r: c.0, g: c.1, b: c.2 }
    }
}

impl Rgb {
    fn rgba(self) -> gdk::RGBA {
        gdk::RGBA::new(
            f32::from(self.0) / 255.0,
            f32::from(self.1) / 255.0,
            f32::from(self.2) / 255.0,
            1.0,
        )
    }

    /// `#rrggbb`
    pub fn css(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// Perceived brightness, 0.0 (black) to 1.0 (white).
    pub fn luminance(self) -> f32 {
        (0.2126 * f32::from(self.0) + 0.7152 * f32::from(self.1) + 0.0722 * f32::from(self.2)) / 255.0
    }

    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let m = |a: u8, b: u8| (f32::from(a) * (1.0 - t) + f32::from(b) * t).round() as u8;
        Rgb(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2))
    }
}

/// How an inactive pane is restyled. Everything blends in with the fade, so `t = 0` is the normal
/// look and `t = 1` the fully inactive look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InactiveStyle {
    /// Fade towards the background colour, 0..1.
    pub dim: f32,
    /// Saturation multiplier (1 = unchanged, 0 = greyscale).
    pub saturation: f32,
    /// Lightness multiplier (1 = unchanged).
    pub lightness: f32,
    /// Hue rotation in degrees.
    pub hue_shift: f32,
}

impl Default for InactiveStyle {
    fn default() -> Self {
        Self { dim: 0.35, saturation: 1.0, lightness: 1.0, hue_shift: 0.0 }
    }
}

impl Rgb {
    fn to_hsl(self) -> (f32, f32, f32) {
        let (r, g, b) = (f32::from(self.0) / 255.0, f32::from(self.1) / 255.0, f32::from(self.2) / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let l = (max + min) / 2.0;
        let d = max - min;
        if d < 1e-6 {
            return (0.0, 0.0, l);
        }
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        } * 60.0;
        (h, s, l)
    }

    fn from_hsl(h: f32, s: f32, l: f32) -> Rgb {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let hp = h.rem_euclid(360.0) / 60.0;
        let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
        let (r, g, b) = match hp as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        let byte = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
        Rgb(byte(r), byte(g), byte(b))
    }

    /// Apply the inactive hue/saturation/lightness transform, blended in by `t` (0..1).
    pub fn adjust(self, st: &InactiveStyle, t: f32) -> Rgb {
        if (st.saturation - 1.0).abs() < 1e-4 && (st.lightness - 1.0).abs() < 1e-4 && st.hue_shift.abs() < 1e-4 {
            return self;
        }
        let (h, s, l) = self.to_hsl();
        Rgb::from_hsl(
            h + st.hue_shift * t,
            (s * (1.0 + (st.saturation - 1.0) * t)).clamp(0.0, 1.0),
            (l * (1.0 + (st.lightness - 1.0) * t)).clamp(0.0, 1.0),
        )
    }
}

/// Restyle a frame (and the extra scroll row) for an inactive pane; `t` is the fade level 0..1.
pub fn apply_inactive(frame: &mut Frame, extra: &mut Option<Vec<Cell>>, style: &InactiveStyle, t: f32) {
    let tint = |c: Rgb| c.adjust(style, t);
    frame.fg = tint(frame.fg);
    frame.bg = tint(frame.bg);
    for cell in frame.rows.iter_mut().flatten().chain(extra.iter_mut().flatten()) {
        cell.fg = tint(cell.fg);
        cell.bg = cell.bg.map(tint);
    }
}

#[derive(Clone)]
pub struct Cell {
    pub text: String,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub wide: bool,
    /// Second half of a wide glyph: nothing to draw.
    pub skip: bool,
}

pub struct Cursor {
    pub x: u16,
    pub y: u16,
    pub style: CursorVisualStyle,
    pub color: Option<Rgb>,
}

/// A find match to highlight within one viewport row.
#[derive(Clone, Copy)]
pub struct Hl {
    pub col: usize,
    pub cols: usize,
    pub current: bool,
}

pub struct Frame {
    pub rows: Vec<Vec<Cell>>,
    /// Find matches per viewport row (index `rows.len()` is the extra scroll row).
    pub highlights: Vec<Vec<Hl>>,
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Option<Cursor>,
}

/// Read the current viewport of `term` into plain data.
pub fn read_frame(
    term: &Terminal<'static, 'static>,
    rs: &mut RenderState<'static>,
    rows_it: &mut RowIterator<'static>,
    cells_it: &mut CellIterator<'static>,
) -> Option<Frame> {
    let snapshot = rs.update(term).ok()?;
    let colors = snapshot.colors().ok()?;
    let fg_default = Rgb::from(colors.foreground);
    let bg_default = Rgb::from(colors.background);
    // Selection highlight: a muted blend of the default foreground into the background.
    let selection_bg = bg_default.mix(fg_default, 0.28);

    let cursor = if snapshot.cursor_visible().unwrap_or(false) {
        match snapshot.cursor_viewport() {
            Ok(Some(c)) => Some(Cursor {
                x: c.x,
                y: c.y,
                style: snapshot
                    .cursor_visual_style()
                    .unwrap_or(CursorVisualStyle::Block),
                color: colors.cursor.map(Rgb::from),
            }),
            _ => None,
        }
    } else {
        None
    };

    let mut rows = Vec::new();
    let mut row_iter = rows_it.update(&snapshot).ok()?;
    while let Some(row) = row_iter.next() {
        let mut cells = Vec::new();
        let mut cell_iter = cells_it.update(row).ok()?;
        while let Some(cell) = cell_iter.next() {
            let graphemes = cell.graphemes().unwrap_or_default();
            let style = cell.style().unwrap_or_default();
            let wide = cell
                .raw_cell()
                .and_then(|c| c.wide())
                .unwrap_or(CellWide::Narrow);

            let mut fg = cell.fg_color().ok().flatten().map(Rgb::from);
            let mut bg = cell.bg_color().ok().flatten().map(Rgb::from);
            if style.inverse {
                let f = fg.unwrap_or(fg_default);
                let b = bg.unwrap_or(bg_default);
                fg = Some(b);
                bg = Some(f);
            }
            if cell.is_selected().unwrap_or(false) {
                bg = Some(selection_bg);
            }
            let mut fg = fg.unwrap_or(fg_default);
            if style.faint {
                fg = fg.mix(bg.unwrap_or(bg_default), 0.5);
            }

            let text = if style.invisible {
                String::new()
            } else {
                graphemes.into_iter().collect()
            };

            cells.push(Cell {
                text,
                fg,
                bg,
                bold: style.bold,
                italic: style.italic,
                underline: style.underline != Underline::None,
                strike: style.strikethrough,
                wide: wide == CellWide::Wide,
                skip: wide == CellWide::SpacerTail,
            });
        }
        rows.push(cells);
    }

    Some(Frame { rows, highlights: Vec::new(), fg: fg_default, bg: bg_default, cursor })
}

/// Everything needed to turn cells into pixels.
pub struct Metrics<'a> {
    pub ctx: &'a pango::Context,
    pub fonts: &'a [pango::FontDescription; 4],
    pub features: Option<&'a str>,
    pub cell_w: f32,
    pub cell_h: f32,
    pub ascent: f32,
    pub hl_match: gdk::RGBA,
    pub hl_current: gdk::RGBA,
}

fn font_index(bold: bool, italic: bool) -> usize {
    usize::from(bold) | (usize::from(italic) << 1)
}

fn make_layout(m: &Metrics, text: &str, bold: bool, italic: bool, underline: bool, strike: bool) -> pango::Layout {
    let layout = pango::Layout::new(m.ctx);
    layout.set_font_description(Some(&m.fonts[font_index(bold, italic)]));
    layout.set_text(text);

    let attrs = pango::AttrList::new();
    if underline {
        attrs.insert(pango::AttrInt::new_underline(pango::Underline::Single));
    }
    if strike {
        attrs.insert(pango::AttrInt::new_strikethrough(true));
    }
    if let Some(features) = m.features {
        attrs.insert(pango::AttrFontFeatures::new(features));
    }
    layout.set_attributes(Some(&attrs));
    layout
}

fn ch_h(m: &Metrics) -> f32 {
    m.cell_h
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> graphene::Rect {
    graphene::Rect::new(x, y, w, h)
}

/// Characters whose glyphs extend above the bounds many fonts declare for them. GTK's GPU glyph
/// cache clips glyphs to the declared bounds, which cuts the top off these (e.g. the hook of `[`
/// in Victor Mono). Runs containing them are rasterised with Cairo instead, which draws the real
/// outline.
fn needs_cairo(text: &str) -> bool {
    text.chars().any(|c| matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '/' | '\\'))
}

fn draw_text_at(snap: &gtk::Snapshot, m: &Metrics, layout: &pango::Layout, x: f32, y: f32, color: Rgb) {
    let baseline = layout.baseline() as f32 / pango::SCALE as f32;
    snap.save();
    snap.translate(&graphene::Point::new(x, y + (m.ascent - baseline)));
    if needs_cairo(&layout.text()) {
        let (w, h) = layout.pixel_size();
        // Generous margins: the real outlines poke out of the logical box. A GskCairoNode is used
        // directly (rather than Snapshot::append_cairo, whose context origin misbehaves under
        // scale transforms such as HiDPI); its context draws in the node's own coordinates.
        let bounds = graphene::Rect::new(-4.0, -8.0, w as f32 + 8.0, h as f32 + 16.0);
        let node = gsk::CairoNode::new(&bounds);
        {
            let cr = node.draw_context();
            let c = color.rgba();
            cr.set_source_rgba(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()), f64::from(c.alpha()));
            cr.move_to(0.0, 0.0);
            pangocairo::functions::show_layout(&cr, layout);
        }
        snap.append_node(&node);
    } else {
        snap.append_layout(layout, &color.rgba());
    }
    snap.restore();
}

/// Single-character cells drawn procedurally (box drawing, blocks) rather than from the font.
fn procedural_char(c: &Cell) -> Option<char> {
    let mut it = c.text.chars();
    match (it.next(), it.next()) {
        (Some(ch), None) if boxdraw::handles(ch) => Some(ch),
        _ => None,
    }
}

fn draw_row(snap: &gtk::Snapshot, m: &Metrics, row: &[Cell], y: f32, default_bg: Rgb, hls: &[Hl]) {
    let (cw, ch) = (m.cell_w, m.cell_h);

    // --- Backgrounds, coalesced into runs of equal colour ---
    let mut col = 0;
    while col < row.len() {
        match row[col].bg {
            Some(bg) if bg != default_bg => {
                let start = col;
                while col < row.len() && row[col].bg == Some(bg) {
                    col += 1;
                }
                snap.append_color(&bg.rgba(), &rect(start as f32 * cw, y, (col - start) as f32 * cw, ch));
            }
            _ => col += 1,
        }
    }

    // --- Find matches: translucent blocks under the text ---
    for h in hls {
        let colour = if h.current { &m.hl_current } else { &m.hl_match };
        snap.append_color(colour, &rect(h.col as f32 * cw, y, h.cols as f32 * cw, ch));
    }

    // --- Text runs (one Pango layout per run => ligatures) ---
    let mut col = 0;
    while col < row.len() {
        let first = &row[col];
        if first.skip {
            col += 1;
            continue;
        }

        if let Some(ch) = procedural_char(first) {
            let cell = boxdraw::Cell {
                x0: (col as f32 * cw).round(),
                x1: ((col + 1) as f32 * cw).round(),
                y0: y.round(),
                y1: (y + ch_h(m)).round(),
                t: (m.cell_h / 16.0).round().max(1.0),
            };
            boxdraw::draw(snap, ch, &cell, &first.fg.rgba());
            col += 1;
            continue;
        }

        let same_style = |c: &Cell| {
            !c.skip
                && procedural_char(c).is_none()
                && !c.wide
                && c.fg == first.fg
                && c.bold == first.bold
                && c.italic == first.italic
                && c.underline == first.underline
                && c.strike == first.strike
        };

        if first.wide {
            // Wide glyphs: own run, positioned exactly on the grid.
            if !first.text.is_empty() {
                let layout = make_layout(m, &first.text, first.bold, first.italic, first.underline, first.strike);
                draw_text_at(snap, m, &layout, col as f32 * cw, y, first.fg);
            }
            col += 2;
            continue;
        }

        let start = col;
        let mut text = String::new();
        while col < row.len() && same_style(&row[col]) {
            if row[col].text.is_empty() {
                text.push(' ');
            } else {
                text.push_str(&row[col].text);
            }
            col += 1;
        }
        if col == start {
            col += 1;
            continue;
        }
        // Skip runs that are only blanks and draw nothing visible.
        if text.trim().is_empty() && !first.underline && !first.strike {
            continue;
        }
        let layout = make_layout(m, &text, first.bold, first.italic, first.underline, first.strike);
        draw_text_at(snap, m, &layout, start as f32 * cw, y, first.fg);
    }
}

/// Scrollbar / scroll-to-bottom button state for one paint.
#[derive(Clone, Copy, Default)]
pub struct ScrollUi {
    pub metrics: Option<scrollbar::Metrics>,
    pub bar_alpha: f32,
    /// Pointer near the bar (or dragging): thicker and brighter.
    pub bar_hot: bool,
    pub btn_alpha: f32,
    pub btn_hover: bool,
    pub bar: scrollbar::ScrollbarStyle,
    pub btn: scrollbar::ButtonStyle,
}

fn rounded_fill(snap: &gtk::Snapshot, r: &graphene::Rect, radius: f32, color: &gdk::RGBA) {
    snap.push_rounded_clip(&gsk::RoundedRect::from_rect(r.clone(), radius));
    snap.append_color(color, r);
    snap.pop();
}

fn with_alpha(c: Rgb, a: f32) -> gdk::RGBA {
    let mut c = c.rgba();
    c.set_alpha(a.clamp(0.0, 1.0));
    c
}

/// Subtle scrollbar thumb and scroll-to-bottom button, drawn in the pane's own text colour.
fn draw_scroll_ui(snap: &gtk::Snapshot, ui: &ScrollUi, fg: Rgb, w: f32, h: f32) {
    let Some(m) = ui.metrics.filter(scrollbar::Metrics::scrollable) else { return };

    if ui.bar_alpha > 0.01 {
        if let Some(g) = scrollbar::geometry(h, m, &ui.bar) {
            let (colour, base_alpha) = ui.bar.color.map_or((fg, 1.0), |t| (t.rgb, t.alpha));
            let thickness = if ui.bar_hot { ui.bar.hover_width } else { ui.bar.width };
            let x = w - thickness - ui.bar.margin;
            if ui.bar_hot {
                let track = rect(x, g.track_top, thickness, g.track_h);
                let a = ui.bar.track_opacity * base_alpha * ui.bar_alpha;
                rounded_fill(snap, &track, thickness / 2.0, &with_alpha(colour, a));
            }
            let thumb = rect(x, g.thumb_y, thickness, g.thumb_h);
            let opacity = if ui.bar_hot { ui.bar.hover_opacity } else { ui.bar.opacity };
            rounded_fill(snap, &thumb, thickness / 2.0, &with_alpha(colour, opacity * base_alpha * ui.bar_alpha));
        }
    }

    if ui.btn.enabled && ui.btn_alpha > 0.01 && !m.at_bottom() {
        let (bx, by) = scrollbar::button_origin(w, h, &ui.btn);
        let size = ui.btn.size;
        let (colour, base_alpha) = ui.btn.color.map_or((fg, 1.0), |t| (t.rgb, t.alpha));
        let circle = rect(bx, by, size, size);
        let fill = if ui.btn_hover { ui.btn.bg_hover_opacity } else { ui.btn.bg_opacity };
        rounded_fill(snap, &circle, size / 2.0, &with_alpha(colour, fill * base_alpha * ui.btn_alpha));

        // Chevron pointing down, scaled with the button.
        let (cx, cy) = (bx + size / 2.0, by + size / 2.0);
        let k = size / 28.0;
        let pb = gsk::PathBuilder::new();
        pb.move_to(cx - 5.0 * k, cy - 2.5 * k);
        pb.line_to(cx, cy + 2.5 * k);
        pb.line_to(cx + 5.0 * k, cy - 2.5 * k);
        let stroke = gsk::Stroke::new(1.8 * k.max(0.6));
        stroke.set_line_cap(gsk::LineCap::Round);
        stroke.set_line_join(gsk::LineJoin::Round);
        let alpha = if ui.btn_hover { ui.btn.icon_hover_opacity } else { ui.btn.icon_opacity };
        snap.append_stroke(&pb.to_path(), &stroke, &with_alpha(colour, alpha * base_alpha * ui.btn_alpha));
    }
}

/// Draw one frame from live terminal state.
pub fn draw(widget: &impl IsA<gtk::Widget>, st: &mut State, snap: &gtk::Snapshot) {
    let (w, h) = (widget.width() as f32, widget.height() as f32);
    let ctx = widget.pango_context();

    let Some(mut frame) = read_frame(&st.term, &mut st.rs, &mut st.rows_it, &mut st.cells_it) else {
        return;
    };

    // Remember how far we're scrolled (for the scrollbar and for hit-testing).
    st.sb = st.term.scrollbar().ok().map(|s| scrollbar::Metrics {
        total: s.total as f32,
        offset: s.offset as f32 + st.frac / st.cell_h,
        len: s.len as f32,
    });

    // Extra row below the viewport for the fractional scroll offset.
    let mut extra_row = if st.frac > 0.0 {
        st.term
            .scroll_viewport(libghostty_vt::terminal::ScrollViewport::Delta(1));
        let below = read_frame(&st.term, &mut st.rs, &mut st.rows_it, &mut st.cells_it)
            .and_then(|f| f.rows.into_iter().last());
        st.term
            .scroll_viewport(libghostty_vt::terminal::ScrollViewport::Delta(-1));
        below
    } else {
        None
    };

    let level = st.dim_level;
    if level > 0.001 {
        apply_inactive(&mut frame, &mut extra_row, &st.inactive, level);
    }

    // Find highlights for the rows in view (plus the extra scroll row).
    if let Some(f) = &st.find {
        let top = st.term.scrollbar().map(|s| s.offset as usize).unwrap_or(0);
        let rows = frame.rows.len();
        frame.highlights = vec![Vec::new(); rows + 1];
        let start = f.matches.partition_point(|m| m.row < top);
        for (i, mt) in f.matches.iter().enumerate().skip(start) {
            if mt.row > top + rows {
                break;
            }
            frame.highlights[mt.row - top].push(Hl { col: mt.col, cols: mt.cols, current: i == f.current });
        }
    }

    let tint = |t: crate::config::Tint| with_alpha(t.rgb, t.alpha);
    let m = Metrics {
        ctx: &ctx,
        fonts: &st.fonts,
        features: st.cfg.font_features.as_deref(),
        cell_w: st.cell_w,
        cell_h: st.cell_h,
        ascent: st.ascent,
        hl_match: tint(st.find_style.matched),
        hl_current: tint(st.find_style.current),
    };
    let dim = level * st.inactive.dim;
    let ui = ScrollUi {
        metrics: if st.sb_style.mode == scrollbar::ScrollbarMode::Never { None } else { st.sb },
        bar_alpha: st.bar_alpha,
        bar_hot: st.bar_hot || matches!(st.dragging, super::mouse::Dragging::Bar(_)),
        btn_alpha: st.btn_alpha,
        btn_hover: st.btn_hover,
        bar: st.sb_style,
        btn: st.btn_style,
    };
    paint(snap, &m, &frame, extra_row.as_deref(), st.frac, st.focused, dim, st.pad, &ui, w, h);
}

/// Paint a frame into `snap`. `extra` is the row just below the viewport, used while a
/// smooth-scroll offset `frac` is non-zero so the bottom edge shows real content, not a gap.
/// `dim` (0..1) fades everything towards the background (inactive pane).
#[allow(clippy::too_many_arguments)]
pub fn paint(
    snap: &gtk::Snapshot,
    m: &Metrics,
    frame: &Frame,
    extra: Option<&[Cell]>,
    frac: f32,
    focused: bool,
    dim: f32,
    pad: super::Padding,
    ui: &ScrollUi,
    w: f32,
    h: f32,
) {
    snap.push_clip(&rect(0.0, 0.0, w, h));
    snap.append_color(&frame.bg.rgba(), &rect(0.0, 0.0, w, h));

    // Text lives inside the padding: shift and clip the content area.
    snap.push_clip(&rect(
        pad.left,
        pad.top,
        (w - pad.left - pad.right).max(0.0),
        (h - pad.top - pad.bottom).max(0.0),
    ));
    snap.save();
    snap.translate(&graphene::Point::new(pad.left, pad.top));

    for (i, row) in frame.rows.iter().enumerate() {
        let hls = frame.highlights.get(i).map_or(&[][..], Vec::as_slice);
        draw_row(snap, m, row, i as f32 * m.cell_h - frac, frame.bg, hls);
    }
    if let Some(row) = extra {
        let hls = frame.highlights.get(frame.rows.len()).map_or(&[][..], Vec::as_slice);
        draw_row(snap, m, row, frame.rows.len() as f32 * m.cell_h - frac, frame.bg, hls);
    }

    // --- Cursor ---
    if let Some(c) = &frame.cursor {
        let x = f32::from(c.x) * m.cell_w;
        let y = f32::from(c.y) * m.cell_h - frac;
        let color = c.color.unwrap_or(frame.fg);
        if focused {
            match c.style {
                CursorVisualStyle::Bar => {
                    snap.append_color(&color.rgba(), &rect(x, y, 2.0, m.cell_h));
                }
                CursorVisualStyle::Underline => {
                    snap.append_color(&color.rgba(), &rect(x, y + m.cell_h - 2.0, m.cell_w, 2.0));
                }
                _ => {
                    snap.append_color(&color.rgba(), &rect(x, y, m.cell_w, m.cell_h));
                    // Redraw the glyph under the block in the background colour.
                    if let Some(cell) = frame.rows.get(usize::from(c.y)).and_then(|r| r.get(usize::from(c.x))) {
                        if !cell.text.is_empty() {
                            let layout = make_layout(m, &cell.text, cell.bold, cell.italic, false, false);
                            draw_text_at(snap, m, &layout, x, y, frame.bg);
                        }
                    }
                }
            }
        } else {
            let col = color.rgba();
            snap.append_color(&col, &rect(x, y, m.cell_w, 1.0));
            snap.append_color(&col, &rect(x, y + m.cell_h - 1.0, m.cell_w, 1.0));
            snap.append_color(&col, &rect(x, y, 1.0, m.cell_h));
            snap.append_color(&col, &rect(x + m.cell_w - 1.0, y, 1.0, m.cell_h));
        }
    }

    snap.restore(); // padding offset
    snap.pop(); // content clip

    draw_scroll_ui(snap, ui, frame.fg, w, h);

    // --- Inactive-pane dimming: a translucent layer of the background colour over everything ---
    if dim > 0.001 {
        let mut c = frame.bg.rgba();
        c.set_alpha(dim.clamp(0.0, 1.0));
        snap.append_color(&c, &rect(0.0, 0.0, w, h));
    }

    snap.pop();
}

#[cfg(test)]
mod tests {
    //! Visual harness: renders sample terminal content to a PNG with GTK's Cairo renderer, so
    //! drawing problems (cropped glyphs, misaligned lines...) can be inspected without a window.
    //!
    //!   PAUA_PNG_DIR=/some/dir cargo test render_sample -- --ignored
    use super::*;
    use gtk::{gsk, graphene};
    use libghostty_vt::{
        RenderState, Terminal, TerminalOptions,
        render::{CellIterator, RowIterator},
    };

    fn render_png(text: &[u8], cols: u16, rows: u16, font: &str, inactive: Option<InactiveStyle>, gpu: bool, path: &std::path::Path) {
        gtk::init().expect("needs a display");
        // A realized (unmapped) window so the label's Pango context picks up our font options.
        let win = gtk::Window::new();
        let label = gtk::Label::new(None);
        win.set_child(Some(&label));
        let mut o = gtk::cairo::FontOptions::new().unwrap();
        match std::env::var("PAUA_HINT").as_deref() {
            Ok("metrics-off") => o.set_hint_metrics(gtk::cairo::HintMetrics::Off),
            Ok("none") => {
                o.set_hint_metrics(gtk::cairo::HintMetrics::Off);
                o.set_hint_style(gtk::cairo::HintStyle::None);
            }
            Ok("gray") => o.set_antialias(gtk::cairo::Antialias::Gray),
            Ok("gray-none") => {
                o.set_antialias(gtk::cairo::Antialias::Gray);
                o.set_hint_metrics(gtk::cairo::HintMetrics::Off);
                o.set_hint_style(gtk::cairo::HintStyle::None);
            }
            _ => {}
        }
        if std::env::var("PAUA_HINT").is_ok() {
            label.set_font_options(Some(&o));
        }
        gtk::prelude::WidgetExt::realize(&win);
        let ctx = label.pango_context();
        let (fonts, cell_w, cell_h, ascent) = super::super::measure(
            &ctx,
            font,
            super::super::FontWeights {
                regular: std::env::var("PAUA_WEIGHT").ok().and_then(|v| v.parse().ok()),
                bold: None,
            },
        );

        let mut term = Terminal::new(TerminalOptions { cols, rows, max_scrollback: 10 }).unwrap();
        term.set_default_fg_color(Some(Rgb(0xd4, 0xd4, 0xd8).into())).unwrap();
        term.set_default_bg_color(Some(Rgb(0x1e, 0x1e, 0x22).into())).unwrap();
        if let Some(n) = std::env::var("PAUA_LEAD").ok().and_then(|v| v.parse::<usize>().ok()) {
            term.vt_write("\r\n".repeat(n).as_bytes()); // blank rows at the top, as after `clear`
        }
        term.vt_write(text);

        let (mut rs, mut rows_it, mut cells_it) =
            (RenderState::new().unwrap(), RowIterator::new().unwrap(), CellIterator::new().unwrap());
        let mut frame = read_frame(&term, &mut rs, &mut rows_it, &mut cells_it).unwrap();
        if let Ok(q) = std::env::var("PAUA_FIND") {
            let lines = super::super::search::screen_lines(&term);
            let opts = super::super::search::FindOptions { query: q, ..Default::default() };
            let matches = super::super::search::find_matches(&lines, &opts, 1000).unwrap();
            let top = term.scrollbar().map(|s| s.offset as usize).unwrap_or(0);
            frame.highlights = vec![Vec::new(); frame.rows.len() + 1];
            for (i, m) in matches.iter().enumerate() {
                if m.row >= top && m.row - top < frame.rows.len() {
                    frame.highlights[m.row - top].push(Hl { col: m.col, cols: m.cols, current: i == 2 });
                }
            }
        }
        let mut dim = 0.0;
        if let Some(style) = inactive {
            apply_inactive(&mut frame, &mut None, &style, 1.0);
            dim = style.dim;
        }
        let m = Metrics {
            ctx: &ctx,
            fonts: &fonts,
            features: None,
            cell_w,
            cell_h,
            ascent,
            hl_match: with_alpha(Rgb(255, 196, 60), 0.32),
            hl_current: with_alpha(Rgb(255, 140, 0), 0.65),
        };

        let (w, h) = (cell_w * f32::from(cols), cell_h * f32::from(rows));
        let extra: f32 = 2.0 * std::env::var("PAUA_PAD").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
        let (w, h) = (w + extra, h + extra);
        let scale: f32 = std::env::var("PAUA_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(2.0);
        let snap = gtk::Snapshot::new();
        snap.scale(scale, scale);
        let p: f32 = std::env::var("PAUA_PAD").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let pad = super::super::Padding { top: p, right: p, bottom: p, left: p };
        let ui = match std::env::var("PAUA_SCROLLUI").as_deref() {
            Ok(mode) => ScrollUi {
                metrics: Some(scrollbar::Metrics { total: 200.0, offset: 60.0, len: f32::from(rows) }),
                bar_alpha: 1.0,
                bar_hot: mode == "hot",
                btn_alpha: 1.0,
                btn_hover: mode == "hot",
                bar: scrollbar::ScrollbarStyle::default(),
                btn: scrollbar::ButtonStyle::default(),
            },
            Err(_) => ScrollUi::default(),
        };
        paint(&snap, &m, &frame, None, 0.0, true, dim, pad, &ui, w, h);
        let node = snap.to_node().unwrap();

        let bounds = graphene::Rect::new(0.0, 0.0, (w * scale).ceil(), (h * scale).ceil());
        if gpu {
            // The renderer GTK actually uses on screen: realize an (unmapped) window's surface.
            let win = gtk::Window::new();
            gtk::prelude::WidgetExt::realize(&win);
            let surface = win.surface().unwrap();
            let renderer = if std::env::var("PAUA_RENDERER").as_deref() == Ok("ngl") {
                let r = gsk::NglRenderer::new();
                r.realize(Some(&surface)).unwrap();
                r.upcast::<gsk::Renderer>()
            } else {
                gsk::Renderer::for_surface(&surface).unwrap()
            };
            eprintln!("renderer: {} cell {}x{}", renderer.type_().name(), cell_w, cell_h);
            let texture = renderer.render_texture(&node, Some(&bounds));
            texture.save_to_png(path).unwrap();
            renderer.unrealize();
        } else {
            let renderer = gsk::CairoRenderer::new();
            renderer.realize_for_display(&gdk::Display::default().unwrap()).unwrap();
            let texture = renderer.render_texture(&node, Some(&bounds));
            texture.save_to_png(path).unwrap();
            renderer.unrealize();
        }
    }

    #[test]
    #[ignore = "needs a display; writes PNGs to $PAUA_PNG_DIR"]
    fn render_sample() {
        let dir = std::path::PathBuf::from(std::env::var("PAUA_PNG_DIR").expect("set PAUA_PNG_DIR"));
        let sample = "\
[process exited]\r\n\
\x1b[2m[process exited]\x1b[0m\r\n\
typed: [x] [] [ ] {} () <> |x| [a-z]+ \x1b[32m[ok]\x1b[0m\r\n\
-> => != === <= >= :: www /* */ ||\r\n\
\x1b[41m [!] \x1b[0m \x1b[44;97m [+] \x1b[0m\r\n\
\u{250c}\u{2500}\u{252c}\u{2500}\u{2510}  \u{256d}\u{2500}\u{256e}\r\n\
\u{2502}a\u{2502}b\u{2502}  \u{2502}c\u{2502}\r\n\
\u{251c}\u{2500}\u{253c}\u{2500}\u{2524}  \u{2570}\u{2500}\u{256f}\r\n\
\u{2514}\u{2500}\u{2534}\u{2500}\u{2518}  \u{2588}\u{2593}\u{2592}\u{2591}\r\n\u{65e5}\u{672c} typed [x] again\r\n";
        let sample: String = if std::env::var("PAUA_ASCII").is_ok() {
            // Every printable ASCII char in its own cell (separated by spaces), 24 per row.
            let chars: Vec<char> = (33u8..127).map(char::from).collect();
            chars.chunks(24).map(|c| c.iter().map(|ch| format!("{ch} ")).collect::<String>() + "\r\n").collect()
        } else {
            sample.to_owned()
        };
        let font = std::env::var("PAUA_TEST_FONT").unwrap_or_else(|_| "VictorMono Nerd Font Propo 11".into());
        render_png(sample.as_bytes(), 48, 9, &font, None, false, &dir.join("sample.png"));
        render_png(sample.as_bytes(), 48, 9, &font, Some(InactiveStyle { dim: 0.25, saturation: 0.35, lightness: 0.9, hue_shift: 0.0 }), false, &dir.join("sample-dim.png"));
        render_png(sample.as_bytes(), 48, 9, &font, None, true, &dir.join("sample-gpu.png"));
    }
}

#[cfg(test)]
mod extents_probe {
    use gtk::{pango, prelude::*};

    #[test]
    #[ignore = "diagnostic: prints Pango ink extents"]
    fn probe_bracket_extents() {
        gtk::init().unwrap();
        let ctx = gtk::Label::new(None).pango_context();
        for font in ["VictorMono Nerd Font Propo 11", "VictorMono Nerd Font Mono 11", "DejaVu Sans Mono 11"] {
            let fd = pango::FontDescription::from_string(font);
            for text in ["[", "x", "M", "|"] {
                let l = pango::Layout::new(&ctx);
                l.set_font_description(Some(&fd));
                l.set_text(text);
                let (ink, logical) = l.extents();
                let s = pango::SCALE as f32;
                eprintln!(
                    "{font:32} {text}: baseline {:6.2} ink y {:6.2}..{:6.2} (top above baseline {:5.2})  logical y {:6.2}..{:6.2}",
                    l.baseline() as f32 / s,
                    ink.y() as f32 / s,
                    (ink.y() + ink.height()) as f32 / s,
                    l.baseline() as f32 / s - ink.y() as f32 / s,
                    logical.y() as f32 / s,
                    (logical.y() + logical.height()) as f32 / s,
                );
            }
        }
    }
}

#[cfg(test)]
mod hsl_tests {
    use super::*;

    #[test]
    fn hsl_round_trips() {
        for c in [Rgb(0, 0, 0), Rgb(255, 255, 255), Rgb(200, 40, 90), Rgb(12, 200, 150), Rgb(0x4a, 0x2f, 0x80)] {
            let (h, s, l) = c.to_hsl();
            let back = Rgb::from_hsl(h, s, l);
            let close = |a: u8, b: u8| a.abs_diff(b) <= 1;
            assert!(close(back.0, c.0) && close(back.1, c.1) && close(back.2, c.2), "{c:?} -> {back:?}");
        }
    }

    #[test]
    fn inactive_transform_behaves() {
        let red = Rgb(220, 40, 40);
        let identity = InactiveStyle { dim: 0.3, saturation: 1.0, lightness: 1.0, hue_shift: 0.0 };
        assert_eq!(red.adjust(&identity, 1.0), red);

        // Zero saturation -> grey (r == g == b).
        let grey = red.adjust(&InactiveStyle { saturation: 0.0, ..identity }, 1.0);
        assert!(grey.0 == grey.1 && grey.1 == grey.2);

        // t = 0 is "not inactive": no change whatever the style.
        assert_eq!(red.adjust(&InactiveStyle { saturation: 0.0, lightness: 0.5, hue_shift: 90.0, ..identity }, 0.0), red);

        // A 120 degree hue shift turns red into green.
        let green = Rgb(255, 0, 0).adjust(&InactiveStyle { hue_shift: 120.0, ..identity }, 1.0);
        assert_eq!(green, Rgb(0, 255, 0));
    }
}
