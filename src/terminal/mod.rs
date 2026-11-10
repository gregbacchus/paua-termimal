//! A terminal widget: libghostty-vt (emulation) + portable-pty (shell) + GTK4/Pango (drawing).
//!
//! Why custom: VTE can't do font ligatures. Rendering through Pango lets HarfBuzz shape runs of
//! cells, and owning the drawing gives us pixel-smooth scrolling.

mod boxdraw;
mod input;
mod mouse;
mod osc133;
mod pty;
mod scrollbar;
mod search;
mod render;

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::{gdk, gio, glib, pango, prelude::*, subclass::prelude::*};
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    key::Mods,
    mouse as gmouse,
    render::{CellIterator, RowIterator},
    selection::gesture::{AutoscrollTickEvent, DragEvent, Gesture, PressEvent, ReleaseEvent},
    terminal::{Mode, ScrollViewport},
};

pub use render::{InactiveStyle, Rgb};
pub use scrollbar::{ButtonStyle, ScrollbarMode, ScrollbarStyle};
pub use search::{CaseMode, FindOptions, FindStatus, FindStyle, MAX_MATCHES};

/// Space between the terminal's edge and its text, in pixels. The terminal background fills it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Padding {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

/// Optional font weights (100-1000). `None` = normal / bold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FontWeights {
    pub regular: Option<u32>,
    pub bold: Option<u32>,
}

pub fn pango_weight(w: u32) -> pango::Weight {
    match w {
        100 => pango::Weight::Thin,
        200 => pango::Weight::Ultralight,
        300 => pango::Weight::Light,
        350 => pango::Weight::Semilight,
        380 => pango::Weight::Book,
        400 => pango::Weight::Normal,
        500 => pango::Weight::Medium,
        600 => pango::Weight::Semibold,
        700 => pango::Weight::Bold,
        800 => pango::Weight::Ultrabold,
        900 => pango::Weight::Heavy,
        1000 => pango::Weight::Ultraheavy,
        other => pango::Weight::__Unknown(other as i32),
    }
}

#[derive(Clone)]
pub struct TermConfig {
    /// Pango font description, e.g. "JetBrains Mono 11".
    pub font: String,
    pub weights: FontWeights,
    /// OpenType features string, e.g. "-calt" to turn ligatures off. `None` = font defaults (on).
    pub font_features: Option<String>,
    pub scrollback: usize,
    pub shell: Option<String>,
    pub cwd: Option<PathBuf>,
}

impl Default for TermConfig {
    fn default() -> Self {
        Self {
            font: "VictorMono Nerd Font Propo 11".into(),
            weights: FontWeights::default(),
            font_features: None,
            scrollback: 10_000,
            shell: None,
            cwd: None,
        }
    }
}

/// Everything mutable about one terminal. Lives behind a RefCell in the widget.
pub struct State {
    term: Terminal<'static, 'static>,
    rs: RenderState<'static>,
    rows_it: RowIterator<'static>,
    cells_it: CellIterator<'static>,

    writer: pty::SharedWriter,
    master: Box<dyn portable_pty::MasterPty + Send>,
    _child: Box<dyn portable_pty::Child + Send + Sync>,

    cfg: TermConfig,
    fonts: [pango::FontDescription; 4],
    cell_w: f32,
    cell_h: f32,
    ascent: f32,
    cols: u16,
    rows: u16,

    /// Smooth scrolling: pixels the content is shifted up by, in [0, cell_h).
    frac: f32,
    /// Smooth scrolling: pixels still to be applied by the tick callback (wheel easing).
    pending: f32,
    last_tick_us: i64,

    focused: bool,
    pwd_flag: Rc<Cell<bool>>,
    bell_flag: Rc<Cell<bool>>,
    title_flag: Rc<Cell<bool>>,

    // Widget size in pixels (for mouse encoding / selection geometry).
    px_w: u32,
    px_h: u32,
    pad: Padding,

    // Selection (libghostty gestures) and mouse reporting.
    gesture: Gesture<'static>,
    press: PressEvent<'static>,
    drag: DragEvent<'static>,
    tick: AutoscrollTickEvent<'static>,
    release: ReleaseEvent<'static>,
    menc: gmouse::Encoder<'static>,
    mev: gmouse::Event<'static>,
    dragging: mouse::Dragging,
    pointer: (f64, f64),
    wheel_accum: f32,
    autoscrolling: bool,
    /// Last seen "program enabled mouse tracking" value, to switch the pointer cursor shape.
    tracking: bool,

    // Running-command timer (header): driven by the foreground process group.
    cmd_cfg: crate::cmdtimer::TimerConfig,
    cmd_tracker: crate::cmdtimer::Tracker,
    cmd_shown: crate::cmdtimer::Shown,
    /// Program name of the current foreground command, looked up once per command.
    cmd_name: Option<(i32, String)>,
    // Shell integration (OSC 133): exact command start/end and exit status.
    osc: osc133::Scanner,
    /// The shell has sent at least one OSC 133 marker, so its markers are authoritative.
    osc_seen: bool,
    /// Bumped by every command start; identifies the command.
    osc_key: i64,
    /// Set from a command start until its end (monotonic microseconds).
    osc_since_us: Option<i64>,
    osc_finished: Option<crate::cmdtimer::Finished>,
    /// Exit status of the last finished command (shells with OSC 133).
    last_exit: Option<i32>,

    // Find in output.
    find: Option<FindState>,
    find_style: FindStyle,
    /// Output arrived since the last search; re-run it shortly.
    find_dirty: bool,

    // Session detection (ssh / sudo / ...): polled from the process running on the pty.
    shell_pid: Option<i32>,
    session_detect: bool,
    session_poll_ms: u32,
    session_last: crate::session::SessionInfo,
    session_polled_us: i64,

    // Scrollbar + scroll-to-bottom button. `sb` is refreshed on every draw. The alphas ease
    // towards their targets in the tick callback.
    sb_style: ScrollbarStyle,
    btn_style: ButtonStyle,
    sb: Option<scrollbar::Metrics>,
    bar_alpha: f32,
    btn_alpha: f32,
    bar_hot: bool,
    btn_hover: bool,
    /// Last time the user scrolled (monotonic microseconds); the bar stays visible for a moment.
    activity_us: i64,

    // Inactive-pane dimming: `dim_level` eases towards `dim_target` (0 or 1); the visible amount
    // is `dim_level * dim_strength`.
    inactive: InactiveStyle,
    dim_fade_ms: f32,
    dim_target: f32,
    dim_level: f32,
}

/// An active find.
struct FindState {
    opts: FindOptions,
    matches: Vec<search::Match>,
    /// Index into `matches` (meaningless when empty).
    current: usize,
    error: Option<String>,
}

impl FindState {
    fn status(&self) -> FindStatus {
        FindStatus {
            active: true,
            current: if self.matches.is_empty() { 0 } else { self.current + 1 },
            total: self.matches.len(),
            error: self.error.clone(),
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        // Closing a pane ends its shell (dropping the PTY master also hangs it up).
        let _ = self._child.kill();
    }
}

impl State {
    fn at_bottom(&self) -> bool {
        self.term
            .scrollbar()
            .map(|s| s.offset + s.len >= s.total)
            .unwrap_or(true)
    }

    fn at_top(&self) -> bool {
        self.term.scrollbar().map(|s| s.offset == 0).unwrap_or(true)
    }

    /// Apply a pixel scroll delta (positive = towards newer output).
    fn scroll_by(&mut self, dy: f32) {
        // No scrollback on the alternate screen / when apps track the mouse; leave those alone.
        if self.term.is_mouse_tracking().unwrap_or(false) {
            return;
        }
        let ch = self.cell_h;
        self.activity_us = glib::monotonic_time();
        self.frac += dy;
        while self.frac >= ch {
            if self.at_bottom() {
                self.frac = 0.0;
                break;
            }
            self.term.scroll_viewport(ScrollViewport::Delta(1));
            self.frac -= ch;
        }
        while self.frac < 0.0 {
            if self.at_top() {
                self.frac = 0.0;
                break;
            }
            self.term.scroll_viewport(ScrollViewport::Delta(-1));
            self.frac += ch;
        }
        if self.frac > 0.0 && self.at_bottom() {
            self.frac = 0.0;
        }
    }

    /// Write to the shell without touching the viewport (mouse reports, focus events).
    fn write_raw(&mut self, bytes: &[u8]) {
        let mut w = self.writer.borrow_mut();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    /// Keyboard input: jump back to the bottom, drop any selection, then write.
    /// (Re)run the current find. `keep_position` keeps the current match near where it was (used
    /// when output arrived); otherwise the first match at or below the viewport top is current.
    /// `jump` scrolls the current match into view.
    fn run_find(&mut self, opts: FindOptions, keep_position: bool, jump: bool) -> FindStatus {
        let previous = self
            .find
            .as_ref()
            .and_then(|f| f.matches.get(f.current).copied())
            .filter(|_| keep_position);
        let lines = search::screen_lines(&self.term);
        let (matches, error) = match search::find_matches(&lines, &opts, search::MAX_MATCHES) {
            Ok(m) => (m, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        let top = self.term.scrollbar().map(|s| s.offset as usize).unwrap_or(0);
        let current = match previous {
            Some(p) => matches
                .iter()
                .position(|m| (m.row, m.col) >= (p.row, p.col))
                .unwrap_or(matches.len().saturating_sub(1)),
            None => matches.iter().position(|m| m.row >= top).unwrap_or(0),
        };
        self.find = Some(FindState { opts, matches, current, error });
        self.find_dirty = false;
        if jump {
            self.reveal_current_match();
        }
        self.find.as_ref().map(FindState::status).unwrap_or_default()
    }

    /// Scroll so the current match is in view (roughly a third from the top).
    fn reveal_current_match(&mut self) {
        let Some(m) = self.find.as_ref().and_then(|f| f.matches.get(f.current).copied()) else { return };
        let Ok(sb) = self.term.scrollbar() else { return };
        let (top, rows) = (sb.offset as usize, usize::from(self.rows));
        if m.row < top || m.row >= top + rows {
            let target = m.row.saturating_sub(rows / 3);
            self.frac = 0.0;
            self.pending = 0.0;
            self.activity_us = glib::monotonic_time();
            self.term.scroll_viewport(ScrollViewport::Row(target));
        }
    }

    fn write_pty(&mut self, bytes: &[u8]) {
        self.term.scroll_viewport(ScrollViewport::Bottom);
        self.frac = 0.0;
        self.pending = 0.0;
        let _ = self.term.set_selection(None);
        self.write_raw(bytes);
    }
}

/// Font variants (regular, bold, italic, bold+italic) and cell metrics for a font description.
fn measure(
    ctx: &pango::Context,
    font: &str,
    weights: FontWeights,
) -> ([pango::FontDescription; 4], f32, f32, f32) {
    let base = pango::FontDescription::from_string(font);
    // Bold defaults to 700, or 200 heavier than a heavy regular weight (so bold stays distinct).
    let regular = weights.regular;
    let bold = weights
        .bold
        .or_else(|| regular.map(|r| (r + 200).clamp(700, 1000)));
    let variant = |is_bold: bool, italic: bool| {
        let mut fd = base.clone();
        match (is_bold, regular, bold) {
            (true, _, Some(b)) => fd.set_weight(pango_weight(b)),
            (true, _, None) => fd.set_weight(pango::Weight::Bold),
            (false, Some(r), _) => fd.set_weight(pango_weight(r)),
            (false, None, _) => {}
        }
        if italic {
            fd.set_style(pango::Style::Italic);
        }
        fd
    };
    let fonts = [
        variant(false, false),
        variant(true, false),
        variant(false, true),
        variant(true, true),
    ];

    let probe = pango::Layout::new(ctx);
    probe.set_font_description(Some(&fonts[0]));
    probe.set_text(&"M".repeat(20));
    let (pw, ph) = probe.size();
    let cell_w = pw as f32 / pango::SCALE as f32 / 20.0;
    let cell_h = (ph as f32 / pango::SCALE as f32).ceil();
    let ascent = probe.baseline() as f32 / pango::SCALE as f32;
    (fonts, cell_w, cell_h, ascent)
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TermView {
        pub state: RefCell<Option<State>>,
        pub on_pwd: RefCell<Option<Box<dyn Fn(&str)>>>,
        pub on_title: RefCell<Option<Box<dyn Fn(&str)>>>,
        pub on_focus: RefCell<Option<Box<dyn Fn(bool)>>>,
        pub on_exit: RefCell<Option<Box<dyn Fn()>>>,
        pub on_session: RefCell<Option<Box<dyn Fn(&crate::session::SessionInfo)>>>,
        pub on_find: RefCell<Option<Box<dyn Fn(&FindStatus)>>>,
        pub on_timer: RefCell<Option<Box<dyn Fn(&crate::cmdtimer::Shown)>>>,
        pub on_bell: RefCell<Option<Box<dyn Fn()>>>,
        pub on_prompt: RefCell<Option<Box<dyn Fn(Option<i32>)>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TermView {
        const NAME: &'static str = "PauaTermView";
        type Type = super::TermView;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("terminal");
        }
    }

    impl ObjectImpl for TermView {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            obj.set_overflow(gtk::Overflow::Hidden);
            obj.set_size_request(120, 60); // keep split panes from shrinking to nothing
        }
    }

    impl WidgetImpl for TermView {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let mut guard = self.state.borrow_mut();
            if let Some(st) = guard.as_mut() {
                render::draw(&*obj, st, snapshot);
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            let mut guard = self.state.borrow_mut();
            let Some(st) = guard.as_mut() else { return };

            st.px_w = width.max(1) as u32;
            st.px_h = height.max(1) as u32;
            let content_w = (width as f32 - st.pad.left - st.pad.right).max(1.0);
            let content_h = (height as f32 - st.pad.top - st.pad.bottom).max(1.0);
            let cols = ((content_w / st.cell_w).floor() as u16).max(2);
            let rows = ((content_h / st.cell_h).floor() as u16).max(1);
            if cols == st.cols && rows == st.rows {
                return;
            }
            st.cols = cols;
            st.rows = rows;
            st.frac = 0.0;
            let _ = st.term.resize(cols, rows, st.cell_w as u32, st.cell_h as u32);
            let _ = st.master.resize(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: width as u16,
                pixel_height: height as u16,
            });
        }
    }
}

glib::wrapper! {
    pub struct TermView(ObjectSubclass<imp::TermView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TermView {
    pub fn new(cfg: TermConfig) -> Self {
        let obj: Self = glib::Object::new();
        if let Err(e) = obj.init(cfg) {
            eprintln!("failed to start terminal: {e}");
        }
        obj.setup_controllers();
        obj
    }

    /// Called when the shell reports its working directory (OSC 7). Receives a `file://` URI.
    pub fn connect_pwd_changed(&self, f: impl Fn(&str) + 'static) {
        *self.imp().on_pwd.borrow_mut() = Some(Box::new(f));
    }

    /// Called when the program sets the window title (OSC 0/2).
    pub fn connect_title_changed(&self, f: impl Fn(&str) + 'static) {
        *self.imp().on_title.borrow_mut() = Some(Box::new(f));
    }

    /// Called when this terminal gains (`true`) or loses (`false`) keyboard focus.
    pub fn connect_focus_changed(&self, f: impl Fn(bool) + 'static) {
        *self.imp().on_focus.borrow_mut() = Some(Box::new(f));
    }

    /// Called when the shell/program started in this terminal exits. Without a handler the
    /// terminal just prints "[process exited]".
    pub fn connect_child_exited(&self, f: impl Fn() + 'static) {
        *self.imp().on_exit.borrow_mut() = Some(Box::new(f));
    }

    /// How inactive panes look (fade, hue/saturation/lightness) and how long the transition takes
    /// (`fade_ms`; 0 = instant).
    pub fn set_inactive_style(&self, style: InactiveStyle, fade_ms: u32) {
        if let Some(st) = self.imp().state.borrow_mut().as_mut() {
            st.inactive = style;
            st.dim_fade_ms = fade_ms as f32;
        }
        self.queue_draw();
    }

    /// Type `text` into the shell as if the user had (used by the command palette).
    pub fn type_text(&self, text: &str) {
        self.send_bytes(text.as_bytes());
    }

    /// Clear the screen and the scrollback, then ask the shell to redraw its prompt (Ctrl+L).
    /// A full-screen program (alternate screen) only gets the Ctrl+L.
    pub fn clear(&self) {
        if let Some(st) = self.imp().state.borrow_mut().as_mut() {
            let alt = st.term.active_screen().unwrap_or_default() == libghostty_vt::screen::Screen::Alternate;
            if !alt {
                st.term.vt_write(b"\x1b[3J\x1b[H\x1b[2J");
                st.frac = 0.0;
                st.pending = 0.0;
            }
        }
        self.send_bytes(b"\x0c");
    }

    /// Called when what the header's command timer shows changes (about once a second while a
    /// long command runs).
    pub fn connect_timer_changed(&self, f: impl Fn(&crate::cmdtimer::Shown) + 'static) {
        *self.imp().on_timer.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_command_timer(&self, cfg: crate::cmdtimer::TimerConfig) {
        if let Some(st) = self.imp().state.borrow_mut().as_mut() {
            st.cmd_cfg = cfg;
        }
    }

    /// Is a command running in the foreground (not the shell itself)? Feed the tracker.
    fn poll_command(&self) {
        let shown = {
            let mut guard = self.imp().state.borrow_mut();
            let Some(st) = guard.as_mut() else { return };
            if !st.cmd_cfg.enabled && st.cmd_shown == crate::cmdtimer::Shown::Hidden {
                return;
            }
            // A program other than the shell in the foreground of the pty (external commands).
            let leader = match (st.shell_pid, st.master.process_group_leader()) {
                (Some(shell), Some(l)) if l != shell => Some(l),
                _ => None,
            };
            if let Some(l) = leader {
                if st.cmd_name.as_ref().map(|(p, _)| *p) != Some(l) {
                    st.cmd_name = Some((l, crate::session::process_name(l).unwrap_or_default()));
                }
            }
            let alt = st.term.active_screen().unwrap_or_default() == libghostty_vt::screen::Screen::Alternate;
            // The program's name (to honour the ignore list); empty for a shell builtin.
            let name = if leader.is_some() {
                st.cmd_name.as_ref().map(|(_, n)| n.clone()).unwrap_or_default()
            } else {
                String::new()
            };
            // A shell that sends OSC 133 markers is authoritative (exact times, builtins, exit
            // status); otherwise a foreground process other than the shell means "running".
            let running = if st.osc_seen {
                st.osc_since_us.map(|since| crate::cmdtimer::Running {
                    key: st.osc_key,
                    name: &name,
                    since_us: Some(since),
                })
            } else {
                leader.map(|l| crate::cmdtimer::Running { key: i64::from(l), name: &name, since_us: None })
            };
            let finished = st.osc_finished.take();
            let shown = st.cmd_tracker.update(glib::monotonic_time(), running, finished, alt, &st.cmd_cfg);
            if shown == st.cmd_shown {
                return;
            }
            st.cmd_shown = shown.clone();
            shown
        };
        if let Some(cb) = self.imp().on_timer.borrow().as_ref() {
            cb(&shown);
        }
    }

    /// Called whenever the find status changes (new results, stepping, output arriving).
    pub fn connect_find_changed(&self, f: impl Fn(&FindStatus) + 'static) {
        *self.imp().on_find.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_find_style(&self, style: FindStyle) {
        if let Some(st) = self.imp().state.borrow_mut().as_mut() {
            st.find_style = style;
        }
        self.queue_draw();
    }

    fn notify_find(&self, status: &FindStatus) {
        if let Some(cb) = self.imp().on_find.borrow().as_ref() {
            cb(status);
        }
        self.queue_draw();
    }

    /// Start or update a find, jumping to the first match at or below the viewport.
    pub fn find(&self, opts: FindOptions) -> FindStatus {
        let status = match self.imp().state.borrow_mut().as_mut() {
            Some(st) => st.run_find(opts, false, true),
            None => FindStatus::default(),
        };
        self.notify_find(&status);
        status
    }

    /// Next (`forward`) or previous match, wrapping around.
    pub fn find_step(&self, forward: bool) -> FindStatus {
        let status = {
            let mut guard = self.imp().state.borrow_mut();
            let Some(st) = guard.as_mut() else { return FindStatus::default() };
            if let Some(f) = st.find.as_mut().filter(|f| !f.matches.is_empty()) {
                let n = f.matches.len();
                f.current = if forward { (f.current + 1) % n } else { (f.current + n - 1) % n };
            }
            st.reveal_current_match();
            st.find.as_ref().map(FindState::status).unwrap_or_default()
        };
        self.notify_find(&status);
        status
    }

    /// Stop finding and remove the highlights.
    pub fn find_clear(&self) {
        let had = match self.imp().state.borrow_mut().as_mut() {
            Some(st) => {
                st.find_dirty = false;
                st.find.take().is_some()
            }
            None => false,
        };
        if had {
            self.notify_find(&FindStatus::default());
        }
    }

    /// Re-run the find if output arrived since the last run (called from a timer).
    fn refresh_find(&self) {
        let status = {
            let mut guard = self.imp().state.borrow_mut();
            let Some(st) = guard.as_mut() else { return };
            if !st.find_dirty {
                return;
            }
            let Some(opts) = st.find.as_ref().map(|f| f.opts.clone()) else { return };
            st.run_find(opts, true, false)
        };
        self.notify_find(&status);
    }

    /// Called when the detected session (ssh, sudo, container...) changes.
    pub fn connect_session_changed(&self, f: impl Fn(&crate::session::SessionInfo) + 'static) {
        *self.imp().on_session.borrow_mut() = Some(Box::new(f));
    }

    /// Turn session detection on/off and set how often the process tree is inspected.
    pub fn set_session_detect(&self, enabled: bool, poll_ms: u32) {
        let mut cleared = false;
        if let Some(st) = self.imp().state.borrow_mut().as_mut() {
            st.session_detect = enabled;
            st.session_poll_ms = poll_ms.max(100);
            if !enabled && !st.session_last.is_empty() {
                st.session_last = crate::session::SessionInfo::default();
                cleared = true;
            }
        }
        if cleared {
            if let Some(cb) = self.imp().on_session.borrow().as_ref() {
                cb(&crate::session::SessionInfo::default());
            }
        }
    }

    /// Look at the process running on the pty; report changes.
    fn poll_session(&self) {
        let (pid, leader) = {
            let mut guard = self.imp().state.borrow_mut();
            let Some(st) = guard.as_mut() else { return };
            let now = glib::monotonic_time();
            if !st.session_detect || now - st.session_polled_us < i64::from(st.session_poll_ms) * 1000 {
                return;
            }
            st.session_polled_us = now;
            let Some(pid) = st.shell_pid else { return };
            (pid, st.master.process_group_leader())
        };
        let info = crate::session::detect(pid, leader);
        let changed = {
            let mut guard = self.imp().state.borrow_mut();
            let Some(st) = guard.as_mut() else { return };
            if st.session_last == info {
                false
            } else {
                st.session_last = info.clone();
                true
            }
        };
        if changed {
            if let Some(cb) = self.imp().on_session.borrow().as_ref() {
                cb(&info);
            }
        }
    }

    /// Scrollbar visibility and the scroll-to-bottom button.
    pub fn set_scroll_style(&self, bar: ScrollbarStyle, button: ButtonStyle) {
        if let Some(st) = self.imp().state.borrow_mut().as_mut() {
            st.sb_style = bar;
            st.btn_style = button;
        }
        self.queue_draw();
    }

    /// Mark this pane as inactive (dimmed) or active.
    pub fn set_dimmed(&self, dimmed: bool) {
        if let Some(st) = self.imp().state.borrow_mut().as_mut() {
            st.dim_target = if dimmed { 1.0 } else { 0.0 };
        }
        self.queue_draw();
    }

    /// Change the font at runtime (e.g. after a config reload). `font` is a Pango description
    /// string such as "VictorMono Nerd Font Propo 11".
    pub fn set_font(&self, font: &str, weights: FontWeights, features: Option<String>) {
        let (fonts, cell_w, cell_h, ascent) = measure(&self.pango_context(), font, weights);
        if let Some(st) = self.imp().state.borrow_mut().as_mut() {
            st.cfg.font = font.to_owned();
            st.cfg.weights = weights;
            st.cfg.font_features = features;
            st.fonts = fonts;
            st.cell_w = cell_w;
            st.cell_h = cell_h;
            st.ascent = ascent;
            st.cols = 0; // force size_allocate to recompute the grid with the new metrics
            st.rows = 0;
            st.frac = 0.0;
            st.pending = 0.0;
        }
        self.queue_allocate();
        self.queue_draw();
    }

    /// Space between the terminal's edge and its text (the background fills it).
    pub fn set_padding(&self, pad: Padding) {
        if let Some(st) = self.imp().state.borrow_mut().as_mut() {
            if st.pad == pad {
                return;
            }
            st.pad = pad;
            st.cols = 0; // force size_allocate to recompute the grid
            st.rows = 0;
            st.frac = 0.0;
        }
        self.queue_allocate();
        self.queue_draw();
    }

    /// Default foreground/background (what programs get for "default colour").
    pub fn set_colors(&self, fg: Rgb, bg: Rgb) {
        if let Some(st) = self.imp().state.borrow_mut().as_mut() {
            let _ = st.term.set_default_fg_color(Some(fg.into()));
            let _ = st.term.set_default_bg_color(Some(bg.into()));
        }
        self.queue_draw();
    }

    fn init(&self, cfg: TermConfig) -> Result<(), Box<dyn std::error::Error>> {
        // --- Fonts + cell metrics ---
        let (fonts, cell_w, cell_h, ascent) = measure(&self.pango_context(), &cfg.font, cfg.weights);

        // --- Emulator ---
        let (cols, rows) = (80u16, 24u16);
        let mut term = Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: cfg.scrollback,
        })?;

        let pty = pty::spawn(cols, rows, cfg.shell.as_deref(), cfg.cwd.clone())?;

        // Replies the emulator generates (device attributes, cursor reports...) go to the shell.
        let reply_writer = pty.writer.clone();
        term.on_pty_write(move |_term, data| {
            let mut w = reply_writer.borrow_mut();
            let _ = w.write_all(data);
            let _ = w.flush();
        })?;

        let pwd_flag = Rc::new(Cell::new(false));
        let title_flag = Rc::new(Cell::new(false));
        let bell_flag = Rc::new(Cell::new(false));
        {
            let f = bell_flag.clone();
            term.on_bell(move |_| f.set(true))?;
            let f = pwd_flag.clone();
            term.on_pwd_changed(move |_| f.set(true))?;
            let f = title_flag.clone();
            term.on_title_changed(move |_| f.set(true))?;
        }

        let shell_pid = pty.child.process_id().map(|p| p as i32);
        let state = State {
            term,
            rs: RenderState::new()?,
            rows_it: RowIterator::new()?,
            cells_it: CellIterator::new()?,
            writer: pty.writer,
            master: pty.master,
            _child: pty.child,
            cfg,
            fonts,
            cell_w,
            cell_h,
            ascent,
            cols,
            rows,
            frac: 0.0,
            pending: 0.0,
            last_tick_us: 0,
            focused: false,
            pwd_flag,
            bell_flag,
            title_flag,
            px_w: 1,
            px_h: 1,
            pad: Padding::default(),
            gesture: Gesture::new()?,
            press: PressEvent::new()?,
            drag: DragEvent::new()?,
            tick: AutoscrollTickEvent::new()?,
            release: ReleaseEvent::new()?,
            menc: gmouse::Encoder::new()?,
            mev: gmouse::Event::new()?,
            dragging: mouse::Dragging::None,
            pointer: (0.0, 0.0),
            wheel_accum: 0.0,
            autoscrolling: false,
            tracking: false,
            cmd_cfg: crate::cmdtimer::TimerConfig::default(),
            cmd_tracker: crate::cmdtimer::Tracker::default(),
            cmd_shown: crate::cmdtimer::Shown::Hidden,
            cmd_name: None,
            osc: osc133::Scanner::default(),
            osc_seen: false,
            osc_key: 0,
            osc_since_us: None,
            osc_finished: None,
            last_exit: None,
            find: None,
            find_style: FindStyle::default(),
            find_dirty: false,
            shell_pid,
            session_detect: true,
            session_poll_ms: 1000,
            session_last: crate::session::SessionInfo::default(),
            session_polled_us: 0,
            sb_style: ScrollbarStyle::default(),
            btn_style: ButtonStyle::default(),
            sb: None,
            bar_alpha: 0.0,
            btn_alpha: 0.0,
            bar_hot: false,
            btn_hover: false,
            activity_us: 0,
            inactive: InactiveStyle::default(),
            dim_fade_ms: 150.0,
            dim_target: 0.0,
            dim_level: 0.0,
        };
        *self.imp().state.borrow_mut() = Some(state);

        // --- Session detection: poll the process tree a few times a second (throttled inside) ---
        {
            let weak = self.downgrade();
            glib::timeout_add_local(Duration::from_millis(250), move || match weak.upgrade() {
                Some(obj) => {
                    obj.poll_session();
                    obj.poll_command();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
        }

        // --- Keep an active find up to date while output arrives ---
        {
            let weak = self.downgrade();
            glib::timeout_add_local(Duration::from_millis(300), move || match weak.upgrade() {
                Some(obj) => {
                    obj.refresh_find();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
        }

        // --- Pump PTY output into the emulator on the main thread ---
        let weak = self.downgrade();
        let rx = pty.output;
        glib::spawn_future_local(async move {
            while let Ok(mut data) = rx.recv().await {
                // Coalesce whatever else is already queued into one write + one redraw.
                while let Ok(more) = rx.try_recv() {
                    data.extend_from_slice(&more);
                    if data.len() > 1 << 20 {
                        break;
                    }
                }
                let Some(obj) = weak.upgrade() else { return };
                obj.feed(&data);
            }
            if let Some(obj) = weak.upgrade() {
                obj.child_exited();
            }
        });

        Ok(())
    }

    fn child_exited(&self) {
        if let Some(cb) = self.imp().on_exit.borrow().as_ref() {
            cb();
        } else {
            self.feed(b"\r\n\x1b[2m[process exited]\x1b[0m\r\n");
        }
    }

    /// Feed shell output to the emulator and schedule a redraw.
    fn feed(&self, data: &[u8]) {
        let imp = self.imp();
        let (mut pwd, mut title, mut cursor) = (None, None, None);
        let bell;
        let mut prompt: Option<Option<i32>> = None;
        {
            let mut guard = imp.state.borrow_mut();
            let Some(st) = guard.as_mut() else { return };
            let now = glib::monotonic_time();
            for marker in st.osc.scan(data) {
                use osc133::Marker;
                st.osc_seen = true;
                match marker {
                    Marker::CommandStart => {
                        st.osc_key += 1;
                        st.osc_since_us = Some(now);
                        st.osc_finished = None;
                    }
                    Marker::CommandEnd(exit) => {
                        st.last_exit = exit;
                        if st.osc_since_us.take().is_some() {
                            st.osc_finished = Some(crate::cmdtimer::Finished { at_us: now, exit });
                        }
                    }
                    // A new prompt without an end marker (e.g. the command was interrupted).
                    Marker::PromptStart | Marker::InputStart => {
                        if marker == Marker::PromptStart {
                            prompt = Some(st.last_exit);
                        }
                        if st.osc_since_us.take().is_some() {
                            st.osc_finished = Some(crate::cmdtimer::Finished { at_us: now, exit: None });
                        }
                    }
                }
            }
            st.term.vt_write(data);
            if st.find.is_some() {
                st.find_dirty = true;
            }
            let tracking = st.term.is_mouse_tracking().unwrap_or(false);
            if tracking != st.tracking {
                st.tracking = tracking;
                cursor = Some(if tracking { "default" } else { "text" });
            }
            if st.frac > 0.0 && st.at_bottom() {
                st.frac = 0.0;
            }
            if st.pwd_flag.replace(false) {
                pwd = st.term.pwd().ok().map(str::to_owned);
            }
            bell = st.bell_flag.replace(false);
            if st.title_flag.replace(false) {
                title = st.term.title().ok().map(str::to_owned);
            }
        }
        if let Some(name) = cursor {
            self.set_cursor_from_name(Some(name));
        }
        self.queue_draw();

        // Callbacks run after the state borrow is released so they may call back into us.
        if let (Some(p), Some(cb)) = (pwd, imp.on_pwd.borrow().as_ref()) {
            cb(&p);
        }
        if let (Some(t), Some(cb)) = (title, imp.on_title.borrow().as_ref()) {
            cb(&t);
        }
        if bell {
            if let Some(cb) = imp.on_bell.borrow().as_ref() {
                cb();
            }
        }
        if let (Some(exit), Some(cb)) = (prompt, imp.on_prompt.borrow().as_ref()) {
            cb(exit);
        }
    }

    /// Called each time the shell shows a prompt, with the last command's exit status (needs a
    /// shell that sends OSC 133, e.g. nushell).
    pub fn connect_prompt(&self, f: impl Fn(Option<i32>) + 'static) {
        *self.imp().on_prompt.borrow_mut() = Some(Box::new(f));
    }

    /// Called when the program rings the bell (BEL).
    pub fn connect_bell(&self, f: impl Fn() + 'static) {
        *self.imp().on_bell.borrow_mut() = Some(Box::new(f));
    }

    fn send_bytes(&self, bytes: &[u8]) {
        if let Some(st) = self.imp().state.borrow_mut().as_mut() {
            st.write_pty(bytes);
        }
        self.queue_draw();
    }

    fn paste(&self) {
        self.paste_from(self.clipboard());
    }

    fn paste_primary(&self) {
        self.paste_from(self.display().primary_clipboard());
    }

    fn paste_from(&self, clipboard: gdk::Clipboard) {
        let weak = self.downgrade();
        clipboard.read_text_async(None::<&gio::Cancellable>, move |res| {
            let (Some(obj), Ok(Some(text))) = (weak.upgrade(), res) else { return };
            let bracketed = obj
                .imp()
                .state
                .borrow()
                .as_ref()
                .and_then(|s| s.term.mode(Mode::BRACKETED_PASTE).ok())
                .unwrap_or(false);
            // Terminals expect CR for newlines in pasted text.
            let text = text.replace("\r\n", "\r").replace('\n', "\r");
            if bracketed {
                obj.send_bytes(format!("\x1b[200~{text}\x1b[201~").as_bytes());
            } else {
                obj.send_bytes(text.as_bytes());
            }
        });
    }

    /// Copy the selection to the clipboard (the Edit menu).
    pub fn copy(&self) {
        self.copy_selection(false);
    }

    /// Paste the clipboard (the Edit menu).
    pub fn paste_clipboard(&self) {
        self.paste();
    }

    /// Copy the selection to the clipboard (Ctrl+Shift+C) or to PRIMARY (on select, for
    /// middle-click paste).
    fn copy_selection(&self, primary: bool) {
        let text = self
            .imp()
            .state
            .borrow()
            .as_ref()
            .and_then(|s| s.selection_text());
        if let Some(text) = text {
            let clipboard = if primary {
                self.display().primary_clipboard()
            } else {
                self.clipboard()
            };
            clipboard.set_text(&text);
        }
    }

    // --- Mouse: selection, and reporting to programs that enable mouse tracking ---

    fn on_press(&self, button: u32, x: f64, y: f64, mods: Mods) {
        let mut paste_primary = false;
        {
            let mut guard = self.imp().state.borrow_mut();
            let Some(st) = guard.as_mut() else { return };
            st.pointer = (x, y);
            if button == 1 && st.scroll_ui_press(x, y) {
                drop(guard);
                self.queue_draw();
                return;
            }
            if st.mouse_reporting(mods) {
                st.dragging = mouse::Dragging::Report(button);
                st.report_mouse(gmouse::Action::Press, mouse::button_for(button), x, y, mods, true);
            } else {
                match button {
                    1 => {
                        st.dragging = mouse::Dragging::Select;
                        let now = Duration::from_micros(glib::monotonic_time().max(0) as u64);
                        st.sel_press(x, y, now);
                    }
                    2 => paste_primary = true,
                    _ => {}
                }
            }
        }
        if paste_primary {
            self.paste_primary();
        }
        self.queue_draw();
    }

    fn on_drag(&self, x: f64, y: f64, mods: Mods) {
        let mut start_autoscroll = false;
        {
            let mut guard = self.imp().state.borrow_mut();
            let Some(st) = guard.as_mut() else { return };
            st.pointer = (x, y);
            match st.dragging {
                mouse::Dragging::Report(b) => {
                    st.report_mouse(gmouse::Action::Motion, mouse::button_for(b), x, y, mods, true);
                }
                mouse::Dragging::Bar(grab) => st.scroll_bar_drag(y, grab),
                mouse::Dragging::Select => {
                    st.sel_drag(x, y, mods.contains(Mods::ALT)); // Alt+drag = rectangle
                    if st.wants_autoscroll() && !st.autoscrolling {
                        st.autoscrolling = true;
                        start_autoscroll = true;
                    }
                }
                mouse::Dragging::None => {}
            }
        }
        if start_autoscroll {
            self.start_autoscroll();
        }
        self.queue_draw();
    }

    fn on_release(&self, x: f64, y: f64, mods: Mods) {
        let mut copy = false;
        {
            let mut guard = self.imp().state.borrow_mut();
            let Some(st) = guard.as_mut() else { return };
            match st.dragging {
                mouse::Dragging::Report(b) => {
                    st.report_mouse(gmouse::Action::Release, mouse::button_for(b), x, y, mods, false);
                }
                mouse::Dragging::Select => {
                    st.sel_release(x, y);
                    copy = true;
                }
                mouse::Dragging::Bar(_) => {}
                mouse::Dragging::None => {}
            }
            st.dragging = mouse::Dragging::None;
        }
        if copy {
            self.copy_selection(true); // PRIMARY, like other Linux terminals
        }
        self.queue_draw();
    }

    /// While dragging a selection above/below the grid, keep scrolling and extending it.
    fn start_autoscroll(&self) {
        let weak = self.downgrade();
        glib::timeout_add_local(Duration::from_millis(40), move || {
            let Some(obj) = weak.upgrade() else { return glib::ControlFlow::Break };
            let keep = match obj.imp().state.borrow_mut().as_mut() {
                Some(st) => {
                    let keep = st.autoscroll_tick();
                    if !keep {
                        st.autoscrolling = false;
                    }
                    keep
                }
                None => false,
            };
            obj.queue_draw();
            if keep { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
        });
    }

    fn setup_controllers(&self) {
        // --- Input: IM context for text/compose/dead keys, key controller for the rest ---
        let im = gtk::IMMulticontext::new();
        im.set_client_widget(Some(self));
        im.set_use_preedit(false);
        im.connect_commit(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, text| obj.send_bytes(text.as_bytes())
        ));

        let key = gtk::EventControllerKey::new();
        key.set_im_context(Some(&im));
        key.connect_key_pressed(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, keyval, _code, state| {
                // Classic shortcuts: Shift+Insert pastes PRIMARY, Ctrl+Insert copies.
                if keyval == gdk::Key::Insert {
                    if state.contains(gdk::ModifierType::SHIFT_MASK) {
                        obj.paste_primary();
                        return glib::Propagation::Stop;
                    }
                    if state.contains(gdk::ModifierType::CONTROL_MASK) {
                        obj.copy_selection(false);
                        return glib::Propagation::Stop;
                    }
                }
                let ctrl_shift = gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK;
                if state.contains(ctrl_shift) {
                    match keyval.to_lower() {
                        gdk::Key::v => {
                            obj.paste();
                            return glib::Propagation::Stop;
                        }
                        gdk::Key::c => {
                            obj.copy_selection(false);
                            return glib::Propagation::Stop;
                        }
                        // Window shortcuts (find, command palette): not for the shell.
                        gdk::Key::f | gdk::Key::p => return glib::Propagation::Proceed,
                        _ => {}
                    }
                }
                let app_cursor = obj
                    .imp()
                    .state
                    .borrow()
                    .as_ref()
                    .and_then(|s| s.term.mode(Mode::DECCKM).ok())
                    .unwrap_or(false);
                match input::encode_key(keyval, state, app_cursor) {
                    Some(bytes) => {
                        obj.send_bytes(&bytes);
                        glib::Propagation::Stop
                    }
                    None => glib::Propagation::Proceed,
                }
            }
        ));
        self.add_controller(key);

        // --- Focus ---
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            #[strong]
            im,
            move |_| {
                im.focus_in();
                if let Some(st) = obj.imp().state.borrow_mut().as_mut() {
                    st.focused = true;
                    if st.term.mode(Mode::FOCUS_EVENT).unwrap_or(false) {
                        st.write_raw(b"\x1b[I");
                    }
                }
                obj.queue_draw();
                if let Some(cb) = obj.imp().on_focus.borrow().as_ref() {
                    cb(true);
                }
            }
        ));
        focus.connect_leave(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            #[strong]
            im,
            move |_| {
                im.focus_out();
                if let Some(st) = obj.imp().state.borrow_mut().as_mut() {
                    st.focused = false;
                    if st.term.mode(Mode::FOCUS_EVENT).unwrap_or(false) {
                        st.write_raw(b"\x1b[O");
                    }
                }
                obj.queue_draw();
                if let Some(cb) = obj.imp().on_focus.borrow().as_ref() {
                    cb(false);
                }
            }
        ));
        self.add_controller(focus);

        // --- Mouse buttons: press / drag / release (GestureDrag keeps tracking outside the widget) ---
        let drag = gtk::GestureDrag::new();
        drag.set_button(0); // any button
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            move |g, x, y| {
                obj.grab_focus();
                obj.on_press(g.current_button(), x, y, mouse::mods_from(g.current_event_state()));
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            move |g, dx, dy| {
                let (sx, sy) = g.start_point().unwrap_or((0.0, 0.0));
                obj.on_drag(sx + dx, sy + dy, mouse::mods_from(g.current_event_state()));
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            move |g, dx, dy| {
                let (sx, sy) = g.start_point().unwrap_or((0.0, 0.0));
                obj.on_release(sx + dx, sy + dy, mouse::mods_from(g.current_event_state()));
            }
        ));
        self.add_controller(drag);

        // Hover motion (only reported to programs that asked for any-event tracking).
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            move |c, x, y| {
                let mods = mouse::mods_from(c.current_event_state());
                let mut cursor = None;
                if let Some(st) = obj.imp().state.borrow_mut().as_mut() {
                    st.pointer = (x, y);
                    if st.update_hover(x, y) {
                        cursor = Some(st.pointer_cursor());
                    }
                    if st.dragging == mouse::Dragging::None
                        && !st.bar_hot
                        && !st.btn_hover
                        && st.mouse_reporting(mods)
                    {
                        st.report_mouse(gmouse::Action::Motion, None, x, y, mods, false);
                    }
                }
                if let Some(name) = cursor {
                    obj.set_cursor_from_name(Some(name));
                    obj.queue_draw();
                }
            }
        ));
        motion.connect_leave(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                let mut cursor = None;
                if let Some(st) = obj.imp().state.borrow_mut().as_mut() {
                    if st.bar_hot || st.btn_hover {
                        st.bar_hot = false;
                        st.btn_hover = false;
                        cursor = Some(st.pointer_cursor());
                    }
                }
                if let Some(name) = cursor {
                    obj.set_cursor_from_name(Some(name));
                    obj.queue_draw();
                }
            }
        ));
        self.add_controller(motion);
        self.set_cursor_from_name(Some("text"));

        // --- Smooth scrolling ---
        // Touchpads/kinetic scrolling give pixel deltas: applied directly. Mouse wheel clicks are
        // converted to a pixel target that the tick callback eases in over ~100ms.
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::KINETIC,
        );
        scroll.connect_scroll(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |ctrl, _dx, dy| {
                let wheel = ctrl.unit() == gdk::ScrollUnit::Wheel;
                let mods = mouse::mods_from(ctrl.current_event_state());
                if let Some(st) = obj.imp().state.borrow_mut().as_mut() {
                    let dy = dy as f32;
                    if st.mouse_reporting(mods) {
                        // Programs with mouse tracking (vim, htop...) get wheel button events.
                        let n = st.wheel_lines(dy, wheel);
                        let (x, y) = st.pointer;
                        for _ in 0..n.abs() {
                            let b = if n < 0 { gmouse::Button::Four } else { gmouse::Button::Five };
                            st.report_mouse(gmouse::Action::Press, Some(b), x, y, mods, false);
                        }
                    } else if st.wheel_as_arrows() {
                        // Alternate screen without mouse tracking (less, man): wheel = arrow keys.
                        let n = st.wheel_lines(dy, wheel);
                        let app = st.term.mode(Mode::DECCKM).unwrap_or(false);
                        let seq: &[u8] = match (n < 0, app) {
                            (true, false) => b"\x1b[A",
                            (true, true) => b"\x1bOA",
                            (false, false) => b"\x1b[B",
                            (false, true) => b"\x1bOB",
                        };
                        for _ in 0..n.abs() {
                            st.write_raw(seq);
                        }
                    } else if wheel {
                        st.pending += dy * st.cell_h * 3.0;
                    } else {
                        st.scroll_by(dy);
                    }
                }
                obj.queue_draw();
                glib::Propagation::Stop
            }
        ));
        self.add_controller(scroll);

        self.add_tick_callback(|obj, clock| {
            let view = obj.downcast_ref::<TermView>().expect("TermView");
            let mut redraw = false;
            if let Some(st) = view.imp().state.borrow_mut().as_mut() {
                let now = clock.frame_time();
                let dt = if st.last_tick_us == 0 {
                    0.016
                } else {
                    ((now - st.last_tick_us) as f32 / 1_000_000.0).clamp(0.0, 0.1)
                };
                st.last_tick_us = now;

                if (st.dim_level - st.dim_target).abs() > 0.001 {
                    let step = if st.dim_fade_ms <= 0.0 { 1.0 } else { dt * 1000.0 / st.dim_fade_ms };
                    st.dim_level = if st.dim_level < st.dim_target {
                        (st.dim_level + step).min(st.dim_target)
                    } else {
                        (st.dim_level - step).max(st.dim_target)
                    };
                    redraw = true;
                } else {
                    st.dim_level = st.dim_target;
                }

                // Scrollbar / button fade towards their targets.
                let scrollable = st.sb.is_some_and(|m| m.scrollable());
                let recent = glib::monotonic_time() - st.activity_us < i64::from(st.sb_style.idle_ms) * 1000;
                let dragging = matches!(st.dragging, mouse::Dragging::Bar(_));
                let bar_target = if !scrollable || st.sb_style.mode == ScrollbarMode::Never {
                    0.0
                } else if st.sb_style.mode == ScrollbarMode::Always || st.bar_hot || dragging || recent {
                    1.0
                } else {
                    0.0
                };
                let btn_target = if st.btn_style.enabled && st.sb.is_some_and(|m| m.scrollable() && !m.at_bottom()) {
                    1.0
                } else {
                    0.0
                };
                let step = if st.sb_style.fade_ms == 0 { 1.0 } else { dt * 1000.0 / st.sb_style.fade_ms as f32 };
                for (alpha, target) in [(&mut st.bar_alpha, bar_target), (&mut st.btn_alpha, btn_target)] {
                    if (*alpha - target).abs() > 0.001 {
                        *alpha = if *alpha < target { (*alpha + step).min(target) } else { (*alpha - step).max(target) };
                        redraw = true;
                    }
                }

                if st.pending.abs() > 0.01 {
                    let step = if st.pending.abs() < 0.5 {
                        st.pending
                    } else {
                        st.pending * (1.0 - (-dt / 0.045).exp())
                    };
                    st.pending -= step;
                    st.scroll_by(step);
                    redraw = true;
                }
            }
            if redraw {
                view.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// The emulator must answer a cursor-position query (CSI 6n) exactly once.
    #[test]
    fn cursor_position_query_is_answered_once() {
        let out: Rc<RefCell<Vec<u8>>> = Rc::default();
        let sink = out.clone();
        let mut term = Terminal::new(TerminalOptions { cols: 80, rows: 25, max_scrollback: 100 }).unwrap();
        term.on_pty_write(move |_t, data| sink.borrow_mut().extend_from_slice(data)).unwrap();

        term.vt_write(b"\x1b[25;4H\x1b[6n");
        assert_eq!(String::from_utf8_lossy(&out.borrow()), "\x1b[25;4R");

        // A second query gets one more reply, not two.
        out.borrow_mut().clear();
        term.vt_write(b"\x1b[6n");
        assert_eq!(String::from_utf8_lossy(&out.borrow()), "\x1b[25;4R");
    }
}
