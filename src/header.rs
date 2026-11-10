//! The line above each terminal: configurable text (template + path rewrites), font and
//! alignment, with pop-out and close buttons on the right.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::{glib, pango, prelude::*};

use crate::config::{HeaderAlign, HeaderConfig, PathReplacement, TimerPosition};

/// Space reserved on the right for Pop Out and Close.
const BUTTONS_WIDTH: i32 = 64;
/// Extra space reserved on the right while the command timer chip is showing.
const TIMER_WIDTH: i32 = 88;
/// Horizontal padding inside the label (`.pane-title` in style.css) plus a little slack.
const LABEL_PADDING: i32 = 9 + 4;

/// Everything the header text is built from. Kept so the text can be re-laid-out when the pane
/// is resized (path trimming depends on the available width).
#[derive(Clone, Default)]
pub struct HeaderInput {
    pub template: String,
    /// Working directory after prefix replacement, e.g. `~/code/app`.
    pub path: String,
    pub dir: String,
    pub title: String,
    pub host: String,
    pub user: String,
    /// Labels of detected sessions (ssh, sudo...) joined with " \u{b7} ".
    pub session: String,
    /// Shorten the path from its middle, by segments, when the text doesn't fit.
    pub trim: bool,
    pub ellipsis: String,
}

impl HeaderInput {
    fn render_with_path(&self, path: &str) -> String {
        render(
            &self.template,
            &Vars {
                path,
                dir: &self.dir,
                title: &self.title,
                host: &self.host,
                user: &self.user,
                session: &self.session,
            },
        )
    }

    /// The untrimmed text.
    pub fn full_text(&self) -> String {
        self.render_with_path(&self.path)
    }
}

pub struct PaneHeader {
    root: gtk::Overlay,
    label: gtk::Label,
    path_slot: gtk::Box,
    path_group: gtk::Box,
    badges: gtk::Box,
    /// The running-command timer chip (right side, before the buttons).
    timer: gtk::Label,
    flash: gtk::Box,
    row: gtk::Box,
    buttons: gtk::Box,
    timer_pos: Cell<TimerPosition>,
    align: Cell<HeaderAlign>,
    tabbed: Cell<bool>,
    input: Rc<RefCell<HeaderInput>>,
    pub copy: gtk::Button,
    pub pop_out: gtk::Button,
    pub close: gtk::Button,
}

impl PaneHeader {
    pub fn new() -> Self {
        let label = gtk::Label::builder()
            .hexpand(true)
            .ellipsize(pango::EllipsizeMode::Middle) // last resort, if even "…/dir" doesn't fit
            .single_line_mode(true)
            .css_classes(["pane-title"])
            .build();

        let copy = Self::button(crate::icons::COPY_PATH_ICON, "Copy path");
        let pop_out = Self::button("pop-out-symbolic", "Open in new window");
        let close = Self::button("window-close-symbolic", "Close pane");
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(2)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .margin_end(4)
            .build();
        let timer = gtk::Label::builder()
            .css_classes(["run-timer"])
            .valign(gtk::Align::Center)
            .margin_end(4)
            .visible(false)
            .build();
        buttons.append(&timer);
        buttons.append(&pop_out);
        buttons.append(&close);

        // Session badges (ssh, sudo...) sit before the text; they take space only when present.
        let badges = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .valign(gtk::Align::Center)
            .margin_start(6)
            .visible(false)
            .build();
        let row = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).hexpand(true).build();
        let path_group = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .halign(gtk::Align::Center)
            .build();
        path_group.append(&label);
        path_group.append(&copy);
        let path_slot = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).hexpand(true).build();
        path_slot.append(&path_group);
        row.append(&badges);
        row.append(&path_slot);

        let root = gtk::Overlay::builder()
            .accessible_role(gtk::AccessibleRole::Tab)
            .css_classes(["pane-header"])
            .build();
        root.set_child(Some(&row));
        let flash = gtk::Box::builder().css_classes(["pane-bell"]).can_target(false).can_focus(false).build();
        root.add_overlay(&flash);
        root.add_overlay(&buttons);

        // Fit to the whole slot so a shortened path can grow again when the pane gets wider.
        let input = Rc::new(RefCell::new(HeaderInput::default()));
        let last_width = Cell::new(-1);
        path_slot.add_tick_callback({
            let input = input.clone();
            let (label, copy) = (label.downgrade(), copy.downgrade());
            move |slot, _| {
                let (Some(label), Some(copy)) = (label.upgrade(), copy.upgrade()) else { return glib::ControlFlow::Break };
                let w = slot.width() - copy.measure(gtk::Orientation::Horizontal, -1).0;
                if w != last_width.get() {
                    last_width.set(w);
                    relayout(&label, &input.borrow(), w);
                }
                glib::ControlFlow::Continue
            }
        });

        Self { root, label, path_slot, path_group, badges, timer, flash, row, buttons, timer_pos: Cell::new(TimerPosition::Right), align: Cell::new(HeaderAlign::Center), tabbed: Cell::new(false), input, copy, pop_out, close }
    }

    pub fn button(icon: &str, tooltip: &str) -> gtk::Button {
        gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tooltip)
            .css_classes(["flat", "pane-button"])
            .valign(gtk::Align::Center)
            .build()
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    pub fn set_tabbed(&self, tabbed: bool) {
        if self.tabbed.replace(tabbed) == tabbed {
            return;
        }
        self.root.set_focusable(tabbed);
        self.close.set_tooltip_text(Some(if tabbed { "Close tab" } else { "Close pane" }));
        self.apply_margins();
    }

    pub fn set_selected(&self, selected: bool) {
        if selected {
            self.root.add_css_class("selected");
            self.root.remove_css_class("needs-attention");
        } else {
            self.root.remove_css_class("selected");
        }
        self.root.update_state(&[gtk::accessible::State::Selected(Some(selected))]);
    }

    pub fn set_input(&self, input: HeaderInput) {
        *self.input.borrow_mut() = input;
        self.relayout();
    }

    fn relayout(&self) {
        let width = self.path_slot.width() - self.copy.measure(gtk::Orientation::Horizontal, -1).0;
        relayout(&self.label, &self.input.borrow(), width);
    }

    #[cfg(test)]
    fn shown_text(&self) -> String {
        self.label.label().to_string()
    }

    /// The untrimmed text (for window titles).
    pub fn full_text(&self) -> String {
        self.input.borrow().full_text()
    }

    /// Apply font and alignment. `fallback_family` is used when the config sets no family.
    pub fn apply_style(&self, cfg: &HeaderConfig, fallback_family: &str) {
        let family = cfg.font.family.as_deref().unwrap_or(fallback_family);
        let mut fd = pango::FontDescription::new();
        fd.set_family(family);
        fd.set_size((cfg.font.size * pango::SCALE as f32).round() as i32);
        if let Some(w) = cfg.font.weight {
            fd.set_weight(crate::terminal::pango_weight(w.0));
        }
        let attrs = pango::AttrList::new();
        attrs.insert(pango::AttrFontDesc::new(&fd));
        self.label.set_attributes(Some(&attrs));

        self.copy.set_icon_name(&cfg.copy_icon);
        self.pop_out.set_icon_name(&cfg.pop_out_icon);
        self.close.set_icon_name(&cfg.close_icon);
        self.align.set(cfg.align);
        self.set_timer_position(cfg.command_timer.position);
        self.apply_margins();
        self.relayout(); // new font = new text widths
    }

    /// Flash the header (the stylesheet fades it out).
    pub fn flash_bell(&self) {
        self.flash.add_css_class("on");
        let flash = self.flash.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(40), move || {
            if let Some(f) = flash.upgrade() {
                f.remove_css_class("on");
            }
        });
    }

    fn set_timer_position(&self, pos: TimerPosition) {
        if self.timer_pos.get() == pos && self.timer.parent().is_some() {
            return;
        }
        self.timer_pos.set(pos);
        if let Some(parent) = self.timer.parent().and_downcast::<gtk::Box>() {
            parent.remove(&self.timer);
        }
        match pos {
            TimerPosition::Right => {
                self.timer.set_margin_start(0);
                self.timer.set_margin_end(4);
                self.buttons.prepend(&self.timer);
            }
            TimerPosition::Left => {
                self.timer.set_margin_start(6);
                self.timer.set_margin_end(0);
                self.row.prepend(&self.timer);
            }
        }
    }

    /// Show the running-command timer: `(icon, icon gap in px, text, finished, failed)`, or
    /// `None` to hide it. Nerd Font glyphs often draw wider than their advance, so the gap after
    /// the icon is added as letter spacing rather than relying on a space character.
    pub fn set_timer(&self, shown: Option<(&str, f32, &str, bool, bool)>) {
        match shown {
            Some((icon, gap, text, done, failed)) => {
                let icon_t = icon.trim_end();
                let markup = if icon_t.is_empty() {
                    glib::markup_escape_text(text).to_string()
                } else {
                    format!(
                        "<span letter_spacing=\"{}\">{}</span>{}",
                        (gap.max(0.0) * pango::SCALE as f32) as i32,
                        glib::markup_escape_text(icon_t),
                        glib::markup_escape_text(text)
                    )
                };
                self.timer.set_markup(&markup);
                for (class, on) in [("done", done), ("failed", failed)] {
                    if on {
                        self.timer.add_css_class(class);
                    } else {
                        self.timer.remove_css_class(class);
                    }
                }
                self.timer.set_visible(true);
            }
            None => self.timer.set_visible(false),
        }
        self.apply_margins();
    }

    /// Mark the header with the pane's most important session (or none) so the stylesheet can
    /// tint the whole header line.
    pub fn set_session_class(&self, class: Option<&'static str>) {
        for kind in crate::session::Kind::ALL {
            self.root.remove_css_class(kind.header_class());
        }
        if let Some(class) = class {
            self.root.add_css_class(class);
        }
    }

    /// Alignment and the space reserved for the buttons (and badges).
    fn apply_margins(&self) {
        let badges = self.badges.is_visible();
        // Room for the timer chip when it is showing, so the text never runs under it.
        let timer_right = self.timer.is_visible() && self.timer_pos.get() == TimerPosition::Right;
        let timer_left = self.timer.is_visible() && self.timer_pos.get() == TimerPosition::Left;
        let right = BUTTONS_WIDTH + if timer_right { TIMER_WIDTH } else { 0 };
        let badges = badges || timer_left; // something sits before the text
        let (align, start) = match self.align.get() {
            HeaderAlign::Start => (gtk::Align::Start, 0),
            // Tab headers share the pane width, so centre within the space left by the controls.
            HeaderAlign::Center => (gtk::Align::Center, if badges || self.tabbed.get() { 6 } else { right }),
            HeaderAlign::End => (gtk::Align::End, 0),
        };
        self.path_group.set_halign(align);
        self.path_slot.set_margin_start(start);
        self.path_slot.set_margin_end(right);
    }

    /// Show one badge per `(text, css kind)`; none hides the badge area.
    pub fn set_badges(&self, badges: &[(String, &'static str)]) {
        while let Some(child) = self.badges.first_child() {
            self.badges.remove(&child);
        }
        for (text, kind) in badges {
            let chip = gtk::Label::builder()
                .label(text)
                .css_classes(["session-badge", kind])
                .ellipsize(pango::EllipsizeMode::End)
                .max_width_chars(28)
                .single_line_mode(true)
                .build();
            self.badges.append(&chip);
        }
        self.badges.set_visible(!badges.is_empty());
        self.apply_margins();
    }
}

/// Set the label text, trimming the path from its middle if the text is wider than the label.
fn relayout(label: &gtk::Label, input: &HeaderInput, width: i32) {
    let full = input.full_text();
    let avail = width - LABEL_PADDING;
    if !input.trim || avail <= 0 {
        label.set_label(&full);
        return;
    }
    let attrs = label.attributes();
    let measure = |text: &str| {
        let layout = label.create_pango_layout(Some(text));
        layout.set_attributes(attrs.as_ref());
        layout.pixel_size().0
    };
    if measure(&full) <= avail {
        label.set_label(&full);
        return;
    }
    let fitted = fit_path(&input.path, &input.ellipsis, |p| measure(&input.render_with_path(p)) <= avail);
    label.set_label(&input.render_with_path(&fitted));
}

/// Shorten `path` until `fits` says it fits, by removing whole segments from the middle outwards
/// and replacing them with one `ellipsis`: `~/a/b/c/d/e` -> `~/a/…/d/e` -> `~/…/e` -> `…/e`.
/// The first and last segments are kept as long as possible. If nothing fits, the shortest form
/// (`…/last`) is returned and the label's own ellipsizing takes over.
pub fn fit_path(path: &str, ellipsis: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(path) {
        return path.to_owned();
    }
    let segs: Vec<&str> = path.split('/').collect();
    let n = segs.len();
    if n <= 2 {
        return path.to_owned();
    }

    // Middle segments, nearest the centre first (left one first on ties).
    let centre = (n as f32 - 1.0) / 2.0;
    let mut order: Vec<usize> = (1..n - 1).collect();
    order.sort_by(|a, b| {
        let (da, db) = ((*a as f32 - centre).abs(), (*b as f32 - centre).abs());
        da.partial_cmp(&db).unwrap().then(a.cmp(b))
    });

    let build = |removed: &[usize]| {
        let mut out: Vec<String> = Vec::new();
        let mut in_gap = false;
        for (i, seg) in segs.iter().enumerate() {
            if removed.contains(&i) {
                if !in_gap {
                    out.push(ellipsis.to_owned());
                    in_gap = true;
                }
            } else {
                out.push((*seg).to_owned());
                in_gap = false;
            }
        }
        out.join("/")
    };

    for k in 1..=order.len() {
        let candidate = build(&order[..k]);
        if fits(&candidate) {
            return candidate;
        }
    }
    format!("{ellipsis}/{}", segs[n - 1])
}

/// Values available to the header template.
pub struct Vars<'a> {
    pub session: &'a str,
    pub path: &'a str,
    pub dir: &'a str,
    pub title: &'a str,
    pub host: &'a str,
    pub user: &'a str,
}

impl Vars<'_> {
    fn get(&self, name: &str) -> Option<&str> {
        match name {
            "path" => Some(self.path),
            "dir" => Some(self.dir),
            "title" => Some(self.title),
            "host" => Some(self.host),
            "user" => Some(self.user),
            "session" => Some(self.session),
            _ => None,
        }
    }
}

/// Expand `{name}` and `{a|b|c}` (first non-empty). `{{` and `}}` are literal braces. Unknown
/// names are left as written so typos are visible.
pub fn render(template: &str, vars: &Vars) -> String {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut inner = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                if !closed {
                    out.push('{');
                    out.push_str(&inner);
                    continue;
                }
                let names: Vec<&str> = inner.split('|').map(str::trim).collect();
                if names.iter().any(|n| vars.get(n).is_none()) {
                    out.push('{');
                    out.push_str(&inner);
                    out.push('}');
                } else if let Some(v) = names.iter().filter_map(|n| vars.get(n)).find(|v| !v.is_empty()) {
                    out.push_str(v);
                }
            }
            c => out.push(c),
        }
    }
    out
}

fn expand_home(s: &str, home: Option<&str>) -> String {
    let Some(home) = home else { return s.to_owned() };
    if s == "~" || s == "$HOME" {
        home.to_owned()
    } else if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("$HOME/")) {
        format!("{}/{rest}", home.trim_end_matches('/'))
    } else {
        s.to_owned()
    }
}

/// Apply the path replacements to `path` (components always match whole, never part of a name).
///
/// - A replacement whose `from` **starts with `/`** matches the **start** of the path; the first such
///   match wins and the rest of the path follows. (`~` and `$HOME` stand for your home directory, so
///   they start with `/` once expanded.)
/// - Any other `from` (`Downloads`, `src/terminal`) matches those components **anywhere** in what is
///   left of the path, each time they occur (first listed wins at a given place).
///
/// Replaced text is never matched again.
pub fn replace_path(path: &str, reps: &[PathReplacement], home: Option<&str>) -> String {
    let expanded: Vec<(String, &PathReplacement)> = reps.iter().map(|r| (expand_home(&r.from, home), r)).collect();
    let anchored = |from: &str| from.starts_with('/');

    // 1. The start of the path.
    let mut head = String::new();
    let mut rest = path;
    for (from, r) in expanded.iter().filter(|(f, _)| anchored(f)) {
        let from = if from.len() > 1 { from.trim_end_matches('/') } else { from.as_str() };
        if from.is_empty() {
            continue;
        }
        if path == from {
            return r.to.clone();
        }
        let sep = if from == "/" { "" } else { "/" };
        if let Some(r2) = path.strip_prefix(&format!("{from}{sep}")) {
            head = format!("{}/", r.to.trim_end_matches('/'));
            rest = r2;
            break;
        }
    }

    // 2. Components anywhere in the rest.
    let patterns: Vec<(Vec<&str>, &str)> = expanded
        .iter()
        .filter(|(f, _)| !anchored(f))
        .map(|(f, r)| (f.trim_matches('/').split('/').filter(|c| !c.is_empty()).collect::<Vec<_>>(), r.to.as_str()))
        .filter(|(p, _)| !p.is_empty())
        .collect();
    if patterns.is_empty() {
        return format!("{head}{rest}");
    }
    let comps: Vec<&str> = rest.split('/').collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < comps.len() {
        let hit = patterns
            .iter()
            .find(|(p, _)| !comps[i].is_empty() && comps.len() - i >= p.len() && comps[i..i + p.len()] == p[..]);
        match hit {
            Some((p, to)) => {
                out.push((*to).to_owned());
                i += p.len();
            }
            None => {
                out.push(comps[i].to_owned());
                i += 1;
            }
        }
    }
    format!("{head}{}", out.join("/"))
}

/// Final path component, or the whole path for `/`.
pub fn dir_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(path)
}

/// Hostname and path from an OSC 7 `file://host/path` URI. The host falls back to this machine.
pub fn parse_uri(uri: &str) -> (String, Option<String>) {
    let host = uri
        .strip_prefix("file://")
        .and_then(|rest| rest.split('/').next())
        .filter(|h| !h.is_empty())
        .map_or_else(|| glib::host_name().to_string(), str::to_owned);
    let path = glib::filename_from_uri(uri).ok().map(|(p, _)| p.display().to_string());
    (host, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars<'a>(title: &'a str) -> Vars<'a> {
        Vars { path: "~/code/x", dir: "x", title, host: "box", user: "greg", session: "ssh box" }
    }

    fn rep(from: &str, to: &str) -> PathReplacement {
        PathReplacement { from: from.into(), to: to.into() }
    }

    #[test]
    fn template_substitution_and_fallbacks() {
        assert_eq!(render("{path}", &vars("")), "~/code/x");
        assert_eq!(render("{user}@{host}: {dir}", &vars("")), "greg@box: x");
        assert_eq!(render("[{session}] {dir}", &vars("")), "[ssh box] x");
        assert_eq!(render("{title|path}", &vars("vim")), "vim");
        assert_eq!(render("{title|path}", &vars("")), "~/code/x");
        assert_eq!(render("{{literal}} {dir}", &vars("")), "{literal} x");
        assert_eq!(render("{nope} {path", &vars("")), "{nope} {path");
    }

    #[test]
    fn prefix_replacement_matches_whole_components() {
        let reps = [rep("/home/greg", "~")];
        assert_eq!(replace_path("/home/greg", &reps, None), "~");
        assert_eq!(replace_path("/home/greg/code/x", &reps, None), "~/code/x");
        assert_eq!(replace_path("/home/gregory/x", &reps, None), "/home/gregory/x");
        assert_eq!(replace_path("/etc/hosts", &reps, None), "/etc/hosts");
    }

    #[test]
    fn components_match_anywhere_in_the_path() {
        let home = Some("/home/greg");
        let reps = [rep("$HOME", "H"), rep("Downloads", "D")];
        assert_eq!(replace_path("/home/greg/Downloads", &reps, home), "H/D");
        assert_eq!(replace_path("/home/greg/Downloads/x/Downloads", &reps, home), "H/D/x/D");
        assert_eq!(replace_path("/mnt/Downloads/a", &reps, home), "/mnt/D/a"); // not under $HOME
        assert_eq!(replace_path("/home/greg/MyDownloads", &reps, home), "H/MyDownloads"); // whole components only
        assert_eq!(replace_path("/home/greg", &reps, home), "H");
        // multi-component, and the first listed wins at one place
        let reps = [rep("src/terminal", "T"), rep("src", "S"), rep("terminal", "X")];
        assert_eq!(replace_path("/p/src/terminal/a", &reps, None), "/p/T/a");
        assert_eq!(replace_path("/p/src/other", &reps, None), "/p/S/other");
        assert_eq!(replace_path("/p/terminal", &reps, None), "/p/X");
        // replaced text isn't matched again
        assert_eq!(replace_path("/a/x", &[rep("x", "y"), rep("y", "z")], None), "/a/y");
        // an absolute `from` only matches at the start
        assert_eq!(replace_path("/a/home/greg", &[rep("/home/greg", "H")], None), "/a/home/greg");
    }

    #[test]
    fn replacements_expand_home_and_first_match_wins() {
        let reps = [rep("$HOME/work", "W"), rep("~", "\u{f015}")];
        let home = Some("/home/greg");
        assert_eq!(replace_path("/home/greg/work/app", &reps, home), "W/app");
        assert_eq!(replace_path("/home/greg/other", &reps, home), "\u{f015}/other");
        assert_eq!(replace_path("/home/greg", &reps, home), "\u{f015}");
    }

    #[test]
    fn path_trims_from_the_middle_by_segments() {
        let fits = |max: usize| move |s: &str| s.chars().count() <= max;
        let p = "~/code/sandbox/rust/ui-sandbox/src";
        assert_eq!(fit_path(p, "…", fits(100)), p);
        // Removes the centre first and keeps both ends, as little as needed.
        assert_eq!(fit_path(p, "…", fits(30)), "~/code/…/rust/ui-sandbox/src");
        assert_eq!(fit_path(p, "…", fits(26)), "~/code/…/ui-sandbox/src");
        assert_eq!(fit_path(p, "…", fits(18)), "~/…/ui-sandbox/src");
        assert_eq!(fit_path(p, "…", fits(10)), "~/…/src");
        // Nothing fits: shortest form, never an empty string.
        assert_eq!(fit_path(p, "…", fits(2)), "…/src");
        // Absolute paths keep their leading slash; custom ellipsis.
        assert_eq!(fit_path("/usr/local/share/doc/pkg", "...", fits(16)), "/usr/.../doc/pkg");
        assert_eq!(fit_path("/usr/local/share/doc/pkg", "...", fits(15)), "/.../doc/pkg");
        // Short paths can't be trimmed.
        assert_eq!(fit_path("~/x", "…", fits(1)), "~/x");
    }

    /// Real widgets: the shown text must follow the header's width. Needs a display (briefly shows
    /// a small window).
    #[test]
    #[ignore = "needs a display"]
    fn header_trims_to_its_width_and_restores() {
        adw::init().unwrap();
        let header = PaneHeader::new();
        header.apply_style(&HeaderConfig::default(), "Monospace");
        header.set_input(HeaderInput {
            template: "{path}".into(),
            path: "~/code/sandbox/rust/ui-sandbox/src/terminal".into(),
            dir: "terminal".into(),
            trim: true,
            ellipsis: "\u{2026}".into(),
            ..Default::default()
        });
        let window = gtk::Window::new();
        window.set_child(Some(header.widget()));
        window.set_default_size(300, 40);
        window.present();

        let pump = |ms: u64| {
            let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < end {
                glib::MainContext::default().iteration(false);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        pump(400);
        let narrow = header.shown_text();
        eprintln!("narrow: {narrow:?}");
        assert!(narrow.contains('\u{2026}') && narrow.ends_with("terminal"), "{narrow}");

        window.set_default_size(1400, 40);
        window.set_size_request(1400, 40);
        pump(600);
        let wide = header.shown_text();
        eprintln!("wide: {wide:?}");
        assert_eq!(wide, "~/code/sandbox/rust/ui-sandbox/src/terminal");
        let label = header.label.compute_bounds(header.widget()).unwrap();
        let copy = header.copy.compute_bounds(header.widget()).unwrap();
        let slot = header.path_slot.compute_bounds(header.widget()).unwrap();
        assert!((label.x() + label.width() - copy.x()).abs() <= 1.0);
        let group_center = (label.x() + copy.x() + copy.width()) / 2.0;
        assert!((group_center - (slot.x() + slot.width() / 2.0)).abs() <= 1.0);
        window.close();
    }

    #[test]
    fn dir_name_and_uri() {
        assert_eq!(dir_name("/home/greg/code"), "code");
        assert_eq!(dir_name("/"), "/");
        let (host, path) = parse_uri("file://box/home/greg/my%20dir");
        assert_eq!((host.as_str(), path.as_deref()), ("box", Some("/home/greg/my dir")));
    }
}
