//! Mouse handling on `State`: text selection (via libghostty's selection gestures), copying, and
//! reporting mouse events to programs like vim/htop (via libghostty's mouse encoder).

use std::time::Duration;

use gtk::glib;

use libghostty_vt::{
    fmt::Format,
    key::Mods,
    mouse as gmouse,
    screen::Screen,
    selection::{FormatOptions, gesture::Autoscroll},
    terminal::{Mode, Point, PointCoordinate, ScrollViewport},
};
use libghostty_vt::selection::gesture::Geometry;

use super::links::{self, Link, UrlModifier};
use super::render::read_frame;
use super::{State, scrollbar};

/// What the current mouse button press is doing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dragging {
    None,
    /// Selecting text.
    Select,
    /// Forwarding the drag to the program, with this GTK button number.
    Report(u32),
    /// Dragging the scrollbar thumb; the value is how far below the thumb's top it was grabbed.
    Bar(f32),
}

pub fn button_for(gtk_button: u32) -> Option<gmouse::Button> {
    match gtk_button {
        1 => Some(gmouse::Button::Left),
        2 => Some(gmouse::Button::Middle),
        3 => Some(gmouse::Button::Right),
        _ => None,
    }
}

pub fn mods_from(state: gtk::gdk::ModifierType) -> Mods {
    use gtk::gdk::ModifierType as M;
    let mut m = Mods::empty();
    if state.contains(M::SHIFT_MASK) {
        m |= Mods::SHIFT;
    }
    if state.contains(M::ALT_MASK) {
        m |= Mods::ALT;
    }
    if state.contains(M::CONTROL_MASK) {
        m |= Mods::CTRL;
    }
    m
}

impl State {
    // ---------------------------------------------------------------------------------------
    // Scrollbar and scroll-to-bottom button
    // ---------------------------------------------------------------------------------------

    pub fn scroll_to_bottom(&mut self) {
        self.frac = 0.0;
        self.pending = 0.0;
        self.activity_us = glib::monotonic_time();
        self.term.scroll_viewport(ScrollViewport::Bottom);
    }

    fn scroll_to_row(&mut self, row: f32) {
        self.frac = 0.0;
        self.pending = 0.0;
        self.activity_us = glib::monotonic_time();
        self.term.scroll_viewport(ScrollViewport::Row(row.round().max(0.0) as usize));
    }

    /// Whether a press at (x, y) hits the scroll button or scrollbar; if so, handle it.
    pub fn scroll_ui_press(&mut self, x: f64, y: f64) -> bool {
        let Some(m) = self.sb.filter(scrollbar::Metrics::scrollable) else { return false };
        let (w, h) = (self.px_w as f32, self.px_h as f32);
        let (xf, yf) = (x as f32, y as f32);

        if self.btn_style.enabled
            && !m.at_bottom()
            && self.btn_alpha > 0.3
            && scrollbar::in_button(w, h, xf, yf, &self.btn_style)
        {
            self.scroll_to_bottom();
            return true;
        }
        if self.sb_style.mode != scrollbar::ScrollbarMode::Never && scrollbar::in_zone(w, xf, &self.sb_style) {
            let Some(g) = scrollbar::geometry(h, m, &self.sb_style) else { return false };
            let grab = if (g.thumb_y..=g.thumb_y + g.thumb_h).contains(&yf) {
                yf - g.thumb_y
            } else {
                // Click on the track: jump so the thumb is centred on the pointer, then drag.
                let grab = g.thumb_h / 2.0;
                self.scroll_to_row(scrollbar::offset_for_thumb_top(&g, m, yf - grab));
                grab
            };
            self.dragging = Dragging::Bar(grab);
            self.bar_hot = true;
            self.activity_us = glib::monotonic_time();
            return true;
        }
        false
    }

    pub fn scroll_bar_drag(&mut self, y: f64, grab: f32) {
        let Some(m) = self.sb.filter(scrollbar::Metrics::scrollable) else { return };
        let Some(g) = scrollbar::geometry(self.px_h as f32, m, &self.sb_style) else { return };
        self.scroll_to_row(scrollbar::offset_for_thumb_top(&g, m, y as f32 - grab));
    }

    /// Update which scroll element (if any) the pointer is over. Returns whether that changed.
    pub fn update_hover(&mut self, x: f64, y: f64) -> bool {
        let (w, h) = (self.px_w as f32, self.px_h as f32);
        let m = self.sb.filter(scrollbar::Metrics::scrollable);
        let bar_hot = m.is_some()
            && self.sb_style.mode != scrollbar::ScrollbarMode::Never
            && (scrollbar::in_zone(w, x as f32, &self.sb_style) || matches!(self.dragging, Dragging::Bar(_)));
        let btn_hover = self.btn_style.enabled
            && m.is_some_and(|m| !m.at_bottom())
            && scrollbar::in_button(w, h, x as f32, y as f32, &self.btn_style);
        let changed = bar_hot != self.bar_hot || btn_hover != self.btn_hover;
        self.bar_hot = bar_hot;
        self.btn_hover = btn_hover;
        changed
    }

    // ---------------------------------------------------------------------------------------
    // Links (URLs and OSC 8 hyperlinks): followed with a modifier held
    // ---------------------------------------------------------------------------------------

    /// Is the open-link modifier held, and is the pointer free to hover a link (not dragging, not
    /// over the scrollbar, and the program isn't getting the mouse)?
    pub fn link_active(&self) -> bool {
        let held = match self.url_style.modifier {
            UrlModifier::Ctrl => self.mods.contains(Mods::CTRL),
            UrlModifier::Alt => self.mods.contains(Mods::ALT),
            UrlModifier::CtrlShift => self.mods.contains(Mods::CTRL | Mods::SHIFT),
        };
        self.url_style.enabled
            && held
            && self.dragging == Dragging::None
            && !self.bar_hot
            && !self.btn_hover
            && !self.mouse_reporting(self.mods)
    }

    /// The link under the pointer right now (reads the visible rows; only call while active).
    fn compute_link(&mut self) -> Option<Link> {
        let (x, y) = self.pointer;
        let (w, h) = (f64::from(self.px_w), f64::from(self.px_h));
        let (pl, pt) = (f64::from(self.pad.left), f64::from(self.pad.top));
        if x < pl || y < pt || x >= w - f64::from(self.pad.right) || y >= h - f64::from(self.pad.bottom) {
            return None;
        }
        let (col, row) = self.cell_at(x, y);
        if self.url_style.osc8 {
            if let Some(l) = self.osc8_link(col, row) {
                return Some(l);
            }
        }
        let re = self.url_re.as_ref()?;
        let frame = read_frame(&self.term, &mut self.rs, &mut self.rows_it, &mut self.cells_it, self.selection_bg)?;
        links::find_link(&frame.rows, &frame.wrapped, row as usize, col as usize, re)
    }

    /// The explicit OSC 8 hyperlink on viewport cell (`col`, `row`): its URI and the run of cells
    /// on that row that share it.
    fn osc8_link(&self, col: u16, row: u32) -> Option<Link> {
        let uri_at = |c: u16| -> Option<String> {
            let g = self.term.grid_ref(Point::Viewport(PointCoordinate { x: c, y: row })).ok()?;
            if !g.cell().ok()?.has_hyperlink().ok()? {
                return None;
            }
            let mut buf = [0u8; 2048];
            let n = g.hyperlink_uri(&mut buf).ok()?;
            (n > 0).then(|| String::from_utf8_lossy(&buf[..n.min(buf.len())]).into_owned())
        };
        let uri = uri_at(col)?;
        let (mut a, mut b) = (col, col);
        while a > 0 && uri_at(a - 1).as_deref() == Some(uri.as_str()) {
            a -= 1;
        }
        while b + 1 < self.cols && uri_at(b + 1).as_deref() == Some(uri.as_str()) {
            b += 1;
        }
        Some(Link { url: uri, spans: vec![(row as usize, usize::from(a), usize::from(b - a) + 1)] })
    }

    /// Recompute the hovered link. Returns true if it changed (redraw, cursor).
    pub fn refresh_link(&mut self) -> bool {
        let new = if self.link_active() { self.compute_link() } else { None };
        let new = new.filter(|l| self.url_style.allows(&l.url));
        let changed = new != self.link;
        self.link = new;
        changed
    }

    /// Mouse pointer shape for the current hover state.
    pub fn pointer_cursor(&self) -> &'static str {
        if self.btn_hover || self.link.is_some() {
            "pointer"
        } else if self.bar_hot || self.tracking {
            "default"
        } else {
            "text"
        }
    }

    /// Viewport cell under a widget-space pixel position (clamped to the grid).
    pub fn cell_at(&self, x: f64, y: f64) -> (u16, u32) {
        let (x, y) = (x - f64::from(self.pad.left), y - f64::from(self.pad.top));
        let col = (x / f64::from(self.cell_w)).floor().clamp(0.0, f64::from(self.cols) - 1.0);
        let row = ((y + f64::from(self.frac)) / f64::from(self.cell_h))
            .floor()
            .clamp(0.0, f64::from(self.rows) - 1.0);
        (col as u16, row as u32)
    }

    fn geometry(&self) -> Geometry {
        Geometry {
            columns: u32::from(self.cols),
            cell_width: self.cell_w.round().max(1.0) as u32,
            padding_left: self.pad.left.round() as u32,
            screen_height: self.px_h.max(1),
        }
    }

    /// Whether mouse events should go to the program (it enabled mouse tracking and the user
    /// isn't holding Shift to force a local selection).
    pub fn mouse_reporting(&self, mods: Mods) -> bool {
        !mods.contains(Mods::SHIFT) && self.term.is_mouse_tracking().unwrap_or(false)
    }

    /// Alternate screen + "alternate scroll" mode: the wheel should act as arrow keys.
    pub fn wheel_as_arrows(&self) -> bool {
        self.term.active_screen().unwrap_or_default() == Screen::Alternate
            && self.term.mode(Mode::ALT_SCROLL).unwrap_or(false)
    }

    // ---------------------------------------------------------------------------------------
    // Reporting to the program
    // ---------------------------------------------------------------------------------------

    pub fn report_mouse(
        &mut self,
        action: gmouse::Action,
        button: Option<gmouse::Button>,
        x: f64,
        y: f64,
        mods: Mods,
        any_button_pressed: bool,
    ) {
        self.menc.set_options_from_terminal(&self.term);
        self.menc.set_size(gmouse::EncoderSize {
            screen_width: self.px_w.max(1),
            screen_height: self.px_h.max(1),
            cell_width: self.cell_w.round().max(1.0) as u32,
            cell_height: self.cell_h.max(1.0) as u32,
            padding_top: self.pad.top.round() as u32,
            padding_bottom: self.pad.bottom.round() as u32,
            padding_right: self.pad.right.round() as u32,
            padding_left: self.pad.left.round() as u32,
        });
        self.menc.set_any_button_pressed(any_button_pressed);
        self.menc.set_track_last_cell(true);

        self.mev
            .set_action(action)
            .set_button(button)
            .set_mods(mods)
            .set_position(gmouse::Position {
                x: x as f32,
                y: (y + f64::from(self.frac)) as f32,
            });

        let mut buf = Vec::new();
        if self.menc.encode_to_vec(&self.mev, &mut buf).is_ok() && !buf.is_empty() {
            self.write_raw(&buf);
        }
    }

    // ---------------------------------------------------------------------------------------
    // Selection
    // ---------------------------------------------------------------------------------------

    pub fn sel_press(&mut self, x: f64, y: f64, now: Duration) {
        let (c, r) = self.cell_at(x, y);
        let Ok(gr) = self.term.grid_ref(Point::Viewport(PointCoordinate { x: c, y: r })) else {
            return;
        };
        let _ = self.press.set_position(x, y);
        let _ = self.press.set_time(now);
        let _ = self.press.set_repeat_distance(f64::from(self.cell_w));
        let _ = self.press.set_repeat_interval(Duration::from_millis(500));
        let sel = self.press.apply(&mut self.gesture, &self.term, gr);
        // A plain click clears any old selection; double/triple click produce a new one.
        let _ = self.term.set_selection(sel.ok().flatten().as_ref());
    }

    pub fn sel_drag(&mut self, x: f64, y: f64, rectangle: bool) {
        let (c, r) = self.cell_at(x, y);
        let Ok(gr) = self.term.grid_ref(Point::Viewport(PointCoordinate { x: c, y: r })) else {
            return;
        };
        let geometry = self.geometry();
        let _ = self.drag.set_position(x, y);
        let _ = self.drag.set_rectangle(rectangle);
        if let Ok(Some(sel)) = self.drag.apply(&mut self.gesture, &self.term, gr, geometry) {
            let _ = self.term.set_selection(Some(&sel));
        }
    }

    pub fn sel_release(&mut self, x: f64, y: f64) {
        let (c, r) = self.cell_at(x, y);
        let gr = self.term.grid_ref(Point::Viewport(PointCoordinate { x: c, y: r })).ok();
        let _ = self.release.apply(&mut self.gesture, &self.term, gr);
    }

    /// Whether dragging is currently asking the viewport to scroll (pointer above/below the grid).
    pub fn wants_autoscroll(&self) -> bool {
        self.gesture.autoscroll(&self.term).unwrap_or(Autoscroll::None) != Autoscroll::None
    }

    /// One autoscroll step. Returns whether to keep ticking.
    pub fn autoscroll_tick(&mut self) -> bool {
        let dir = self.gesture.autoscroll(&self.term).unwrap_or(Autoscroll::None);
        let (delta, row) = match dir {
            Autoscroll::Up => (-1, 0),
            Autoscroll::Down => (1, u32::from(self.rows).saturating_sub(1)),
            _ => return false,
        };
        self.frac = 0.0;
        self.term.scroll_viewport(ScrollViewport::Delta(delta));
        let (c, _) = self.cell_at(self.pointer.0, self.pointer.1);
        let geometry = self.geometry();
        if let Ok(Some(sel)) = self.tick.apply(
            &mut self.gesture,
            &self.term,
            PointCoordinate { x: c, y: row },
            geometry,
        ) {
            let _ = self.term.set_selection(Some(&sel));
        }
        true
    }

    /// The selected text, formatted like Ghostty's copy (unwrapped, trailing whitespace trimmed).
    pub fn selection_text(&self) -> Option<String> {
        let opts = FormatOptions::new()
            .with_emit_format(Format::Plain)
            .with_unwrap(true)
            .with_trim(true);
        let bytes = self.term.format_selection_alloc(None, opts).ok()??;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (!text.is_empty()).then_some(text)
    }

    /// Wheel movement converted to whole lines (signed; positive = down). Mouse wheels give
    /// clicks (3 lines each); touchpads give pixels, accumulated until a full line.
    pub fn wheel_lines(&mut self, dy: f32, wheel: bool) -> i32 {
        if wheel {
            return (dy * 3.0).round() as i32;
        }
        self.wheel_accum += dy;
        let lines = (self.wheel_accum / self.cell_h).trunc();
        self.wheel_accum -= lines * self.cell_h;
        lines as i32
    }
}

#[cfg(test)]
mod tests {
    //! Headless checks of the libghostty selection/mouse APIs as `State` uses them.
    use libghostty_vt::{
        Terminal, TerminalOptions,
        fmt::Format,
        mouse as gmouse,
        selection::{
            FormatOptions,
            gesture::{DragEvent, Gesture, Geometry, PressEvent, ReleaseEvent},
        },
        terminal::{Mode, Point, PointCoordinate},
    };
    use std::time::Duration;

    fn term_with(text: &[u8]) -> Terminal<'static, 'static> {
        let mut t = Terminal::new(TerminalOptions { cols: 40, rows: 5, max_scrollback: 100 }).unwrap();
        t.vt_write(text);
        t
    }

    fn at<'a>(t: &'a Terminal<'static, 'static>, x: u16, y: u32) -> libghostty_vt::screen::GridRef<'a> {
        t.grid_ref(Point::Viewport(PointCoordinate { x, y })).unwrap()
    }

    fn selected_text(t: &Terminal<'static, 'static>) -> Option<String> {
        let opts = FormatOptions::new().with_emit_format(Format::Plain).with_unwrap(true).with_trim(true);
        let b = t.format_selection_alloc(None, opts).ok()??;
        Some(String::from_utf8_lossy(&b).into_owned())
    }

    fn geometry() -> Geometry {
        Geometry { columns: 40, cell_width: 10, padding_left: 0, screen_height: 100 }
    }

    #[test]
    fn drag_selects_text_across_cells() {
        let t = term_with(b"hello world\r\nsecond line");
        let mut g = Gesture::new().unwrap();
        let (mut press, mut drag, mut release) =
            (PressEvent::new().unwrap(), DragEvent::new().unwrap(), ReleaseEvent::new().unwrap());

        press.set_position(5.0, 5.0).unwrap();
        press.set_time(Duration::from_millis(1000)).unwrap();
        let sel = press.apply(&mut g, &t, at(&t, 0, 0)).unwrap();
        t.set_selection(sel.as_ref()).unwrap();

        drag.set_position(55.0, 5.0).unwrap();
        let sel = drag.apply(&mut g, &t, at(&t, 5, 0), geometry()).unwrap().expect("drag gives a selection");
        t.set_selection(Some(&sel)).unwrap();
        release.apply(&mut g, &t, Some(at(&t, 5, 0))).unwrap();

        assert_eq!(selected_text(&t).as_deref(), Some("hello"));
    }

    #[test]
    fn double_click_selects_a_word() {
        let t = term_with(b"hello world");
        let mut g = Gesture::new().unwrap();
        let mut press = PressEvent::new().unwrap();
        press.set_repeat_distance(10.0).unwrap();
        press.set_repeat_interval(Duration::from_millis(500)).unwrap();

        press.set_position(75.0, 5.0).unwrap();
        press.set_time(Duration::from_millis(1000)).unwrap();
        press.apply(&mut g, &t, at(&t, 7, 0)).unwrap();
        press.set_time(Duration::from_millis(1150)).unwrap();
        let sel = press.apply(&mut g, &t, at(&t, 7, 0)).unwrap().expect("double click selects");
        t.set_selection(Some(&sel)).unwrap();

        assert_eq!(selected_text(&t).as_deref(), Some("world"));
    }

    #[test]
    fn mouse_encoder_reports_sgr_press_when_program_enables_tracking() {
        let mut t = term_with(b"");
        t.vt_write(b"\x1b[?1000h\x1b[?1006h"); // normal tracking + SGR format, as vim does
        assert!(t.is_mouse_tracking().unwrap());
        assert!(t.mode(Mode::SGR_MOUSE).unwrap());

        let mut enc = gmouse::Encoder::new().unwrap();
        enc.set_options_from_terminal(&t);
        enc.set_size(gmouse::EncoderSize {
            screen_width: 400, screen_height: 100, cell_width: 10, cell_height: 20,
            padding_top: 0, padding_bottom: 0, padding_right: 0, padding_left: 0,
        });
        let mut ev = gmouse::Event::new().unwrap();
        ev.set_action(gmouse::Action::Press)
            .set_button(Some(gmouse::Button::Left))
            .set_position(gmouse::Position { x: 25.0, y: 45.0 }); // col 3, row 3 (1-based)
        let mut out = Vec::new();
        enc.encode_to_vec(&ev, &mut out).unwrap();
        assert_eq!(String::from_utf8_lossy(&out), "\x1b[<0;3;3M");
    }
}
