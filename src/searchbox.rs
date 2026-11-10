//! The title-bar box. One entry, three modes (a chip at its left shows which):
//!
//! - **idle**: a placeholder. Clicking the box opens the palette; Ctrl+Shift+F starts a find.
//! - **find** (`/`): searches the focused pane's output. Shows a match count, prev/next, case and
//!   regex toggles; the matches are highlighted in the pane.
//! - **command** (`>`): the command palette. A dropdown of actions, panes, directories and ssh
//!   hosts, filtered as you type (fuzzy), with a "Find in output" row to switch modes.
//!
//! Typing `>` or `/` as the first character switches mode. The box only reports what the user did
//! through callbacks; the app owns the terminals and decides which palette items exist.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::{glib, pango, prelude::*};

use crate::palette::{self, PaletteItem};
use crate::shortcuts::{Bindings, Scope};
use crate::terminal::{CaseMode, FindOptions, FindStatus};

const DEBOUNCE_MS: u64 = 80;
const POPOVER_MIN_WIDTH: i32 = 460;
/// Narrowest the dropdown gets, even in a tiny window.
const POPOVER_MIN_NARROW: i32 = 280;
/// The box itself never gets narrower than this (it shrinks with the window otherwise).
const BOX_MIN_WIDTH: i32 = 260;
const POPOVER_MAX_HEIGHT: i32 = 380;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Idle,
    Find,
    Command,
}

/// One row of the dropdown.
struct Shown {
    /// `None` is the "Find in output" bridge row.
    item: Option<usize>,
}

type Provider = Box<dyn Fn() -> Vec<PaletteItem>>;

struct Inner {
    root: gtk::Box,
    entry: gtk::Entry,
    chip: gtk::Label,
    count: gtk::Label,
    /// Find-only widgets (counter, prev/next, toggles).
    controls: Vec<gtk::Widget>,
    case_btn: gtk::ToggleButton,
    regex_btn: gtk::ToggleButton,
    popover: gtk::Popover,
    scrolled: gtk::ScrolledWindow,
    list: gtk::ListBox,

    mode: Cell<Mode>,
    shortcuts: RefCell<Rc<Bindings>>,
    /// Case handling when the "match case" toggle is off (from the config).
    base_case: Cell<CaseMode>,
    last_query: RefCell<String>,
    generation: Cell<u32>,
    /// Set while we change the entry text ourselves (mode switches).
    suppress: Cell<bool>,
    max_results: Cell<usize>,

    items: RefCell<Vec<PaletteItem>>,
    shown: RefCell<Vec<Shown>>,
    provider: RefCell<Option<Provider>>,

    on_open: RefCell<Option<Box<dyn Fn(Mode)>>>,
    on_close: RefCell<Option<Box<dyn Fn()>>>,
    on_query: RefCell<Option<Box<dyn Fn(FindOptions)>>>,
    on_step: RefCell<Option<Box<dyn Fn(bool)>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.popover.unparent();
    }
}

pub struct SearchBox {
    pub root: gtk::Box,
    inner: Rc<Inner>,
}

impl SearchBox {
    pub fn new(shortcuts: Rc<Bindings>) -> Self {
        let chip = gtk::Label::builder()
            .css_classes(["mode-chip"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        let entry = gtk::Entry::builder()
            .hexpand(true)
            .has_frame(false)
            .css_classes(["command-entry"])
            .xalign(0.5) // the placeholder is centred; typed text goes left (see below)
            .build();
        let count = gtk::Label::builder()
            .css_classes(["match-count"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        let flat = |icon: &str, tip: &str| {
            gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tip)
                .css_classes(["flat", "command-button"])
                .visible(false)
                .build()
        };
        let prev = flat("go-up-symbolic", "Previous match (Shift+Enter)");
        let next = flat("go-down-symbolic", "Next match (Enter)");
        let toggle = |label: &str, tip: &str| {
            gtk::ToggleButton::builder()
                .label(label)
                .tooltip_text(tip)
                .css_classes(["flat", "command-button", "command-toggle"])
                .visible(false)
                .build()
        };
        let case_btn = toggle("Aa", "Match case");
        let regex_btn = toggle(".*", "Regular expression");

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(2)
            .css_classes(["command-center"])
            .build();
        root.set_size_request(BOX_MIN_WIDTH, -1);
        for w in [
            chip.upcast_ref::<gtk::Widget>(),
            entry.upcast_ref(),
            count.upcast_ref(),
            prev.upcast_ref(),
            next.upcast_ref(),
            case_btn.upcast_ref(),
            regex_btn.upcast_ref(),
        ] {
            root.append(w);
        }
        let controls = vec![
            count.clone().upcast(),
            prev.clone().upcast(),
            next.clone().upcast(),
            case_btn.clone().upcast(),
            regex_btn.clone().upcast(),
        ];

        // The palette dropdown. It must never take keyboard focus from the entry.
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .focusable(false)
            .css_classes(["palette-list"])
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(POPOVER_MAX_HEIGHT)
            .focusable(false)
            .build();
        let popover = gtk::Popover::builder()
            .child(&scrolled)
            .autohide(false)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .can_focus(false)
            .focusable(false)
            .css_classes(["palette-popover"])
            .build();
        popover.set_parent(&root);

        let inner = Rc::new(Inner {
            root: root.clone(),
            entry,
            chip,
            count,
            controls,
            case_btn,
            regex_btn,
            popover,
            scrolled,
            list,
            mode: Cell::new(Mode::Idle),
            shortcuts: RefCell::new(shortcuts),
            base_case: Cell::new(CaseMode::Smart),
            last_query: RefCell::default(),
            generation: Cell::new(0),
            suppress: Cell::new(false),
            max_results: Cell::new(40),
            items: RefCell::default(),
            shown: RefCell::default(),
            provider: RefCell::default(),
            on_open: RefCell::default(),
            on_close: RefCell::default(),
            on_query: RefCell::default(),
            on_step: RefCell::default(),
        });
        inner.apply_mode_ui();
        Self::connect(&inner, &prev, &next);
        Self { root, inner }
    }

    fn connect(inner: &Rc<Inner>, prev: &gtk::Button, next: &gtk::Button) {
        let weak = Rc::downgrade(inner);
        let with = |weak: &Weak<Inner>, f: fn(&Rc<Inner>)| {
            if let Some(i) = weak.upgrade() {
                f(&i);
            }
        };

        inner.entry.connect_changed({
            let weak = weak.clone();
            move |_| with(&weak, Inner::text_changed)
        });
        for t in [&inner.case_btn, &inner.regex_btn] {
            t.connect_toggled({
                let weak = weak.clone();
                move |_| with(&weak, Inner::schedule_query)
            });
        }

        prev.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(i) = weak.upgrade() {
                    i.step(false);
                }
            }
        });
        next.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(i) = weak.upgrade() {
                    i.step(true);
                }
            }
        });

        // Clicking a dropdown row runs it.
        inner.list.connect_row_activated({
            let weak = weak.clone();
            move |_, row| {
                if let Some(i) = weak.upgrade() {
                    i.activate(row.index() as usize);
                }
            }
        });

        // Handle mode-specific shortcuts before the entry edits text or moves its cursor.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let weak = weak.clone();
            move |controller, _, _, _| {
                let Some(i) = weak.upgrade() else { return glib::Propagation::Proceed };
                let scope = match i.mode.get() {
                    Mode::Idle => return glib::Propagation::Proceed,
                    Mode::Find => Scope::Find,
                    Mode::Command => Scope::Palette,
                };
                let Some(event) = controller.current_event() else { return glib::Propagation::Proceed };
                let bindings = i.shortcuts.borrow().clone();
                match bindings.contextual_action(scope, &event) {
                    Some("close_search") => i.close(true),
                    Some("find_next") => i.step(true),
                    Some("find_previous") => i.step(false),
                    Some("find_toggle_case") => i.case_btn.set_active(!i.case_btn.is_active()),
                    Some("find_toggle_regex") => i.regex_btn.set_active(!i.regex_btn.is_active()),
                    Some("palette_next") => i.move_selection(1),
                    Some("palette_previous") => i.move_selection(-1),
                    Some("palette_page_down") => i.move_selection(6),
                    Some("palette_page_up") => i.move_selection(-6),
                    Some("palette_run") => i.run_selected(),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        inner.entry.add_controller(keys);

        // Clicking into the idle box opens the palette; leaving the palette closes it.
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter({
            let weak = weak.clone();
            move |_| {
                if let Some(i) = weak.upgrade() {
                    if i.mode.get() == Mode::Idle {
                        i.open_command();
                    }
                }
            }
        });
        focus.connect_leave({
            let weak = weak.clone();
            move |_| {
                let Some(i) = weak.upgrade() else { return };
                if i.mode.get() != Mode::Command {
                    return;
                }
                // Focus can bounce through the popover; only close if it really left.
                let weak = Rc::downgrade(&i);
                glib::idle_add_local_once(move || {
                    if let Some(i) = weak.upgrade() {
                        if i.mode.get() == Mode::Command && !i.entry.has_focus() {
                            i.close(false);
                        }
                    }
                });
            }
        });
        inner.entry.add_controller(focus);
    }

    // ---- API ----

    /// Called when the box opens in find or command mode.
    pub fn connect_open(&self, f: impl Fn(Mode) + 'static) {
        *self.inner.on_open.borrow_mut() = Some(Box::new(f));
    }

    /// Called when the box closes (Esc, or a palette item ran).
    pub fn connect_close(&self, f: impl Fn() + 'static) {
        *self.inner.on_close.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_query(&self, f: impl Fn(FindOptions) + 'static) {
        *self.inner.on_query.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_step(&self, f: impl Fn(bool) + 'static) {
        *self.inner.on_step.borrow_mut() = Some(Box::new(f));
    }

    /// Supplies the palette items each time the palette opens.
    pub fn connect_items(&self, f: impl Fn() -> Vec<PaletteItem> + 'static) {
        *self.inner.provider.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_max_results(&self, n: usize) {
        self.inner.max_results.set(n.clamp(5, 200));
    }

    /// Start a find.
    pub fn open_find(&self) {
        self.inner.open_find();
    }

    /// Open the command palette.
    pub fn open_command(&self) {
        self.inner.open_command();
    }

    pub fn is_find_open(&self) -> bool {
        self.inner.mode.get() == Mode::Find
    }

    /// What the box currently asks for in find mode.
    pub fn options(&self) -> FindOptions {
        self.inner.options()
    }

    /// Case handling when "match case" is off, and the initial regex toggle (from the config).
    pub fn set_defaults(&self, case: CaseMode, regex: bool) {
        self.inner.base_case.set(case);
        if self.inner.mode.get() == Mode::Idle {
            self.inner.regex_btn.set_active(regex);
        }
    }

    pub fn set_shortcuts(&self, shortcuts: Rc<Bindings>) {
        *self.inner.shortcuts.borrow_mut() = shortcuts;
        if self.inner.mode.get() == Mode::Idle {
            self.inner.apply_mode_ui();
        }
    }

    pub fn set_status(&self, status: &FindStatus) {
        self.inner.set_status(status);
    }
}

impl Inner {
    fn options(&self) -> FindOptions {
        FindOptions {
            query: self.entry.text().to_string(),
            case: if self.case_btn.is_active() { CaseMode::Sensitive } else { self.base_case.get() },
            regex: self.regex_btn.is_active(),
        }
    }

    /// Chip, placeholder, find controls and dropdown for the current mode.
    fn apply_mode_ui(&self) {
        let mode = self.mode.get();
        let idle_placeholder;
        let (chip, tip, placeholder) = match mode {
            Mode::Idle => {
                let bindings = self.shortcuts.borrow();
                let with_hint = |label: &str, id: &str| {
                    let hint = bindings.hint(id);
                    if hint.is_empty() { label.to_owned() } else { format!("{label} {hint}") }
                };
                idle_placeholder = format!("{}  ·  {}", with_hint("Commands", "palette"), with_hint("Find", "find"));
                ("", "", idle_placeholder.as_str())
            }
            Mode::Find => ("/", "Find in output (type > for commands)", "Find in output"),
            Mode::Command => (">", "Command palette (type / to find in output)", "Command, pane or directory"),
        };
        // The idle placeholder is centred; once the box is open (find / command) the caret and the
        // text start at the left, as you'd expect when typing.
        EditableExt::set_alignment(&self.entry, if mode == Mode::Idle { 0.5 } else { 0.0 });
        self.chip.set_label(chip);
        self.chip.set_tooltip_text(Some(tip));
        self.chip.set_visible(mode != Mode::Idle);
        self.entry.set_placeholder_text(Some(placeholder));
        if mode == Mode::Idle {
            self.root.remove_css_class("active");
        } else {
            self.root.add_css_class("active");
        }
        let find = mode == Mode::Find;
        for w in &self.controls {
            // The counter appears once there is something to count.
            w.set_visible(find && w != self.count.upcast_ref::<gtk::Widget>());
        }
        if mode != Mode::Command {
            self.popover.popdown();
        }
    }

    /// Replace the entry text without triggering the change handler.
    fn set_text_quiet(&self, text: &str) {
        self.suppress.set(true);
        self.entry.set_text(text);
        self.entry.set_position(-1);
        self.suppress.set(false);
    }

    fn notify_open(&self, mode: Mode) {
        if let Some(cb) = self.on_open.borrow().as_ref() {
            cb(mode);
        }
    }

    fn open_find(self: &Rc<Self>) {
        self.open_find_with(false);
    }

    /// Enter find mode. `keep_text` carries what is already typed (a `/` prefix or the palette's
    /// "Find in output" row); otherwise the box starts from the previous query.
    fn open_find_with(self: &Rc<Self>, keep_text: bool) {
        let was = self.mode.replace(Mode::Find);
        if !keep_text && was == Mode::Command {
            self.set_text_quiet(""); // the palette text isn't a search
        }
        self.apply_mode_ui();
        if !keep_text && self.entry.text().is_empty() {
            let last = self.last_query.borrow().clone();
            self.set_text_quiet(&last); // reopen with the previous query (selected)
        }
        self.entry.grab_focus();
        if keep_text {
            self.entry.set_position(-1);
        } else {
            self.entry.select_region(0, -1);
        }
        if was == Mode::Idle {
            self.notify_open(Mode::Find);
        }
        self.schedule_query();
    }

    fn open_command(self: &Rc<Self>) {
        self.open_command_with(false);
    }

    /// Enter command mode. `keep_text` carries what is already typed (a `>` prefix).
    fn open_command_with(self: &Rc<Self>, keep_text: bool) {
        let was = self.mode.replace(Mode::Command);
        if was == Mode::Find {
            self.generation.set(self.generation.get().wrapping_add(1)); // drop pending queries
            if !keep_text {
                *self.last_query.borrow_mut() = self.entry.text().to_string();
                self.set_text_quiet("");
            }
        }
        self.apply_mode_ui();
        if was != Mode::Command {
            if was == Mode::Idle {
                self.notify_open(Mode::Command);
            }
            let items = self.provider.borrow().as_ref().map(|p| p()).unwrap_or_default();
            *self.items.borrow_mut() = items;
        }
        self.entry.grab_focus();
        self.refresh_list();
    }

    /// Close whatever is open. `notify` = tell the app (it clears find highlights and gives the
    /// focus back to the terminal); false when the user clicked elsewhere themselves.
    fn close(self: &Rc<Self>, notify: bool) {
        let was = self.mode.replace(Mode::Idle);
        if was == Mode::Idle {
            return;
        }
        if was == Mode::Find {
            *self.last_query.borrow_mut() = self.entry.text().to_string();
        }
        self.generation.set(self.generation.get().wrapping_add(1)); // drop pending queries
        self.set_text_quiet("");
        self.apply_mode_ui();
        self.items.borrow_mut().clear();
        self.shown.borrow_mut().clear();
        if notify {
            if let Some(cb) = self.on_close.borrow().as_ref() {
                cb();
            }
        }
    }

    fn step(&self, forward: bool) {
        if self.mode.get() != Mode::Find {
            return;
        }
        if let Some(cb) = self.on_step.borrow().as_ref() {
            cb(forward);
        }
    }

    fn run_selected(self: &Rc<Self>) {
        let selected = self.list.selected_row().map(|row| row.index() as usize).unwrap_or(0);
        self.activate(selected);
    }

    /// The entry text changed: a leading `>` or `/` switches mode, otherwise update results.
    fn text_changed(self: &Rc<Self>) {
        if self.suppress.get() {
            return;
        }
        let text = self.entry.text().to_string();
        match self.mode.get() {
            Mode::Command => {
                if let Some(rest) = text.strip_prefix('/') {
                    self.set_text_quiet(rest);
                    self.open_find_with(true);
                } else {
                    self.refresh_list();
                }
            }
            Mode::Find => {
                if let Some(rest) = text.strip_prefix('>') {
                    self.set_text_quiet(rest);
                    self.open_command_with(true);
                } else {
                    self.schedule_query();
                }
            }
            Mode::Idle => {}
        }
    }

    /// Run the query shortly after the last change (typing, toggles).
    fn schedule_query(self: &Rc<Self>) {
        if self.mode.get() != Mode::Find {
            return;
        }
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(DEBOUNCE_MS), move || {
            let Some(i) = weak.upgrade() else { return };
            if i.generation.get() != generation || i.mode.get() != Mode::Find {
                return;
            }
            let opts = i.options();
            if let Some(cb) = i.on_query.borrow().as_ref() {
                cb(opts);
            }
        });
    }

    fn set_status(&self, s: &FindStatus) {
        let empty_query = self.entry.text().is_empty();
        let (text, class) = if !s.active || empty_query {
            (String::new(), "")
        } else if s.error.is_some() {
            ("invalid pattern".to_owned(), "error")
        } else if s.total == 0 {
            ("No results".to_owned(), "none")
        } else if s.total >= crate::terminal::MAX_MATCHES {
            (format!("{}/{}+", s.current, s.total), "")
        } else {
            (format!("{}/{}", s.current, s.total), "")
        };
        for c in ["error", "none"] {
            self.count.remove_css_class(c);
        }
        if !class.is_empty() {
            self.count.add_css_class(class);
        }
        self.count.set_label(&text);
        self.count.set_visible(self.mode.get() == Mode::Find && !text.is_empty());
        self.count.set_tooltip_text(s.error.as_deref());
    }

    // ---- palette dropdown ----

    /// Rebuild the dropdown for the current text.
    fn refresh_list(self: &Rc<Self>) {
        let query = self.entry.text().to_string();
        let ranked = palette::rank(&self.items.borrow(), &query, self.max_results.get());

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let mut shown = Vec::new();
        if !query.trim().is_empty() {
            self.list.append(&Self::row("Find", &format!("Find \"{query}\" in output"), "/", &[], ""));
            shown.push(Shown { item: None });
        }
        {
            let items = self.items.borrow();
            for r in &ranked {
                let it = &items[r.item];
                self.list.append(&Self::row(it.section, &it.title, &it.subtitle, &r.title_matches, "item"));
                shown.push(Shown { item: Some(r.item) });
            }
        }
        let empty = shown.is_empty();
        *self.shown.borrow_mut() = shown;

        if empty {
            self.popover.popdown();
            return;
        }
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
        self.scrolled.vadjustment().set_value(0.0);
        // Wide enough to read: at least the box's width, but never wider than the window. (An
        // explicit size request is needed: with the horizontal scrollbar off GTK sizes the
        // popover to its content's *minimum* width, and ellipsized labels have almost none.)
        let window_w = self.root.root().map_or(i32::MAX, |r| r.upcast_ref::<gtk::Widget>().width());
        let width = self
            .root
            .width()
            .max(POPOVER_MIN_WIDTH)
            .min((window_w - 24).max(POPOVER_MIN_NARROW));
        self.scrolled.set_size_request(width, -1);
        if !self.popover.is_visible() {
            self.popover.popup();
        }
    }

    /// One dropdown row: a small section tag, the title with matched letters emphasised, and dim
    /// secondary text on the right.
    fn row(section: &str, title: &str, subtitle: &str, matches: &[usize], class: &str) -> gtk::ListBoxRow {
        let tag = gtk::Label::builder()
            .label(section)
            .css_classes(["palette-tag"])
            .valign(gtk::Align::Center)
            .build();
        let title_label = gtk::Label::builder()
            .use_markup(true)
            .label(highlight_markup(title, matches))
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["palette-title"])
            .build();
        let sub = gtk::Label::builder()
            .label(subtitle)
            .xalign(1.0)
            .ellipsize(pango::EllipsizeMode::Middle)
            .max_width_chars(34)
            .css_classes(["palette-subtitle"])
            .build();
        let content = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(10).build();
        content.append(&tag);
        content.append(&title_label);
        content.append(&sub);
        let row = gtk::ListBoxRow::builder().child(&content).focusable(false).build();
        row.add_css_class("palette-row");
        if !class.is_empty() {
            row.add_css_class(class);
        }
        row
    }

    fn move_selection(&self, delta: i32) {
        let n = self.shown.borrow().len() as i32;
        if n == 0 {
            return;
        }
        let cur = self.list.selected_row().map_or(0, |r| r.index());
        let next = if delta.abs() > 1 {
            (cur + delta).clamp(0, n - 1) // paging stops at the ends
        } else {
            (cur + delta).rem_euclid(n) // arrows wrap
        };
        if let Some(row) = self.list.row_at_index(next) {
            self.list.select_row(Some(&row));
            self.ensure_visible(&row);
        }
    }

    /// Scroll the dropdown so `row` is fully in view.
    fn ensure_visible(&self, row: &gtk::ListBoxRow) {
        let Some(bounds) = row.compute_bounds(&self.list) else { return };
        let adj = self.scrolled.vadjustment();
        let (top, bottom) = (f64::from(bounds.y()), f64::from(bounds.y() + bounds.height()));
        if top < adj.value() {
            adj.set_value(top);
        } else if bottom > adj.value() + adj.page_size() {
            adj.set_value(bottom - adj.page_size());
        }
    }

    /// Run dropdown row `idx`. The bridge row switches to find; items close the box first (so the
    /// app can hand focus back) and then run.
    fn activate(self: &Rc<Self>, idx: usize) {
        let run = {
            let shown = self.shown.borrow();
            match shown.get(idx) {
                None => return,
                Some(Shown { item: None }) => None,
                Some(Shown { item: Some(i) }) => Some(self.items.borrow()[*i].run.clone()),
            }
        };
        match run {
            None => {
                // Keep what was typed and search for it.
                self.open_find_with(true);
            }
            Some(run) => {
                self.close(true);
                run();
            }
        }
    }
}

/// Pango markup for `text` with the characters at `matches` (char indices) in bold.
fn highlight_markup(text: &str, matches: &[usize]) -> String {
    let mut out = String::new();
    let mut open = false;
    for (i, ch) in text.chars().enumerate() {
        let hit = matches.contains(&i);
        if hit && !open {
            out.push_str("<b>");
            open = true;
        } else if !hit && open {
            out.push_str("</b>");
            open = false;
        }
        out.push_str(&glib::markup_escape_text(&ch.to_string()));
    }
    if open {
        out.push_str("</b>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::highlight_markup;

    #[test]
    fn markup_bolds_matches_and_escapes() {
        assert_eq!(highlight_markup("Split right", &[0, 1, 6]), "<b>Sp</b>lit <b>r</b>ight");
        assert_eq!(highlight_markup("a<b & c", &[1]), "a<b>&lt;</b>b &amp; c");
        assert_eq!(highlight_markup("plain", &[]), "plain");
    }
}
