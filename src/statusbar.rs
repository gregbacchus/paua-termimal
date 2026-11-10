//! The status bar: a row of configurable widgets, left and right, that drop out by priority when
//! the window is too narrow.
//!
//! Anything that can be slow (an `exec` command, `git`, looking for project files) runs on a
//! worker thread with a hard timeout (`exec.rs`); the UI thread only ever applies finished results.
//! Each widget has at most one job in flight, and results from before a config reload are dropped.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

use crate::config::{Refresh, Side, StatusBarConfig, WhenConfig, WidgetConfig};
use crate::{exec, git, template};

/// What the bar needs from the app.
pub struct Host {
    /// Run a named action (`split_right`, `type:...`, ...).
    pub action: Box<dyn Fn(&str)>,
    pub toast: Box<dyn Fn(&str)>,
    /// Recently visited directories as `(shown, path)`, newest first.
    pub recent_dirs: Box<dyn Fn() -> Vec<(String, String)>>,
    /// Type `cd <path>` into the focused pane.
    pub cd: Box<dyn Fn(&str)>,
}

/// The focused pane, as far as widgets care.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Context {
    pub cwd: PathBuf,
    /// The working directory as shown in pane headers (`~/code/x`).
    pub path: String,
    pub dir: String,
    pub host: String,
    pub user: String,
    pub title: String,
    pub session: String,
    /// Exit status of the last command, when the shell reports it.
    pub exit: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Clock,
    Exec,
    Button,
    Menu,
    Dirs,
    Git,
}

struct ItemData {
    output: String,
    /// Result of the `when` check (true when there is none).
    when_ok: bool,
    fetched: Option<(PathBuf, Instant)>,
    in_flight: bool,
    pending: bool,
    last_interval: Option<Instant>,
}

struct Item {
    cfg: WidgetConfig,
    kind: Kind,
    widget: gtk::Widget,
    /// The text (for git: the branch button's text).
    label: gtk::Label,
    sync: Option<(gtk::Button, gtk::Label)>,
    branches: Option<Rc<BranchUi>>,
    wants: Cell<bool>,
    nat_w: Cell<i32>,
    data: RefCell<ItemData>,
}

#[derive(Default)]
struct GitState {
    status: Option<git::Status>,
    in_flight: bool,
    pending: bool,
    busy: bool,
    last: Option<Instant>,
}

enum Pick {
    Switch(String),
    Create(String),
}

/// The branch picker's widgets and data.
struct BranchUi {
    entry: gtk::SearchEntry,
    list: gtk::Box,
    branches: RefCell<Vec<git::Branch>>,
    first: RefCell<Option<Pick>>,
}

pub struct StatusBar {
    pub root: gtk::Box,
    scroller: gtk::ScrolledWindow,
    left: gtk::Box,
    right: gtk::Box,
    host: Host,
    cfg: RefCell<StatusBarConfig>,
    items: RefCell<Vec<Rc<Item>>>,
    ctx: RefCell<Context>,
    epoch: Cell<u64>,
    dirty: Cell<bool>,
    last_width: Cell<i32>,
    git: RefCell<GitState>,
    me: Weak<StatusBar>,
}

const SPACING: i32 = 2;
const GROUP_GAP: i32 = 24;

fn flat_label() -> gtk::Label {
    gtk::Label::builder().xalign(0.0).single_line_mode(true).build()
}

fn menu_popover() -> gtk::Popover {
    gtk::Popover::builder().has_arrow(false).position(gtk::PositionType::Top).build()
}

fn popover_rows() -> gtk::Box {
    let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(1).build();
    b.set_margin_top(2);
    b.set_margin_bottom(2);
    b
}

fn row_button(text: &str) -> gtk::Button {
    let l = gtk::Label::builder().label(text).xalign(0.0).build();
    let b = gtk::Button::builder().child(&l).css_classes(["flat", "sb-row"]).build();
    b
}

/// A CSS-safe class name for a widget id.
fn class_name(id: &str) -> String {
    let clean: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    format!("sb-{clean}")
}

/// Which widgets (`(priority, natural width)`, in layout order) stay when only `avail` pixels are
/// free: the lowest priority goes first, and of equals the one configured last.
fn fit(items: &[(i32, i32)], avail: i32) -> Vec<bool> {
    let mut keep = vec![true; items.len()];
    let mut total: i32 = GROUP_GAP + items.iter().map(|(_, w)| w + SPACING).sum::<i32>();
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&k| (items[k].0, std::cmp::Reverse(k)));
    for k in order {
        if total <= avail {
            break;
        }
        keep[k] = false;
        total -= items[k].1 + SPACING;
    }
    keep
}

/// Does `name` exist in `dir` or any parent?
fn exists_up(dir: &Path, name: &str) -> bool {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if d.join(name).exists() {
            return true;
        }
        cur = d.parent();
    }
    false
}

fn when_matches(w: &WhenConfig, cwd: &Path) -> bool {
    (w.files.is_empty() || w.files.iter().any(|f| exists_up(cwd, f))) && !w.not_files.iter().any(|f| exists_up(cwd, f))
}

fn join(icon: &str, text: &str) -> String {
    match (icon.is_empty(), text.is_empty()) {
        (_, true) => icon.to_owned(),
        (true, false) => text.to_owned(),
        (false, false) => format!("{icon} {text}"),
    }
}

/// `{status}` for git: the file-state icons, each with its count.
fn git_status_text(s: &git::Status, cfg: &crate::config::GitConfig) -> String {
    let i = &cfg.icons;
    let parts = [
        (&i.conflicted, s.conflicted),
        (&i.staged, s.staged),
        (&i.modified, s.modified),
        (&i.deleted, s.deleted),
        (&i.renamed, s.renamed),
        (&i.untracked, s.untracked),
        (&i.stashed, s.stashed),
    ];
    parts
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(icon, n)| if cfg.counts { format!("{icon}{n}") } else { (*icon).clone() })
        .collect::<Vec<_>>()
        .join(" ")
}

fn git_sync_text(s: &git::Status, cfg: &crate::config::GitConfig) -> String {
    let mut v = Vec::new();
    if s.ahead > 0 && s.behind > 0 {
        // Diverged: one icon, then (optionally) the counts.
        v.push(cfg.icons.diverged.clone());
        if cfg.counts {
            v.push(format!("{}{} {}{}", cfg.icons.ahead, s.ahead, cfg.icons.behind, s.behind));
        }
        return v.join(" ");
    }
    if s.behind > 0 {
        v.push(format!("{}{}", cfg.icons.behind, s.behind));
    }
    if s.ahead > 0 {
        v.push(format!("{}{}", cfg.icons.ahead, s.ahead));
    }
    v.join(" ")
}

/// What an `exec` / `when` widget's worker computes.
struct JobOut {
    when_ok: bool,
    output: String,
}

fn run_job(cfg: &WidgetConfig, cwd: &Path, timeout: Duration) -> JobOut {
    if let Some(w) = &cfg.when {
        if !when_matches(w, cwd) {
            return JobOut { when_ok: false, output: String::new() };
        }
    }
    let output = match &cfg.exec {
        Some(cmd) => match exec::run_shell(cmd, cwd, timeout) {
            Ok(o) if o.success => exec::first_line(&o.stdout),
            _ => String::new(),
        },
        None => String::new(),
    };
    JobOut { when_ok: true, output }
}

impl StatusBar {
    pub fn new(host: Host) -> Rc<Self> {
        let left = gtk::Box::builder().spacing(SPACING).hexpand(true).halign(gtk::Align::Start).build();
        let right = gtk::Box::builder().spacing(SPACING).halign(gtk::Align::End).build();
        let inner = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).build();
        inner.append(&left);
        inner.append(&right);
        // External policy = no minimum width, so the window can shrink and widgets drop out
        // instead of the bar forcing a minimum window width.
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .propagate_natural_height(true)
            .child(&inner)
            .build();
        let root = gtk::Box::builder().css_classes(["status-bar"]).build();
        root.append(&scroller);

        let sb = Rc::new_cyclic(|me| StatusBar {
            root,
            scroller,
            left,
            right,
            host,
            cfg: RefCell::new(StatusBarConfig::default()),
            items: RefCell::default(),
            ctx: RefCell::default(),
            epoch: Cell::new(0),
            dirty: Cell::new(true),
            last_width: Cell::new(-1),
            git: RefCell::default(),
            me: me.clone(),
        });

        let weak = Rc::downgrade(&sb);
        sb.root.add_tick_callback(move |_, _| {
            if let Some(sb) = weak.upgrade() {
                if sb.dirty.get() || sb.scroller.width() != sb.last_width.get() {
                    sb.relayout();
                }
            }
            glib::ControlFlow::Continue
        });
        let weak = Rc::downgrade(&sb);
        glib::timeout_add_seconds_local(1, move || match weak.upgrade() {
            Some(sb) => {
                sb.on_second();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        sb
    }

    // -----------------------------------------------------------------------------------------
    // Building
    // -----------------------------------------------------------------------------------------

    /// (Re)build the widgets from the config.
    pub fn configure(self: &Rc<Self>, cfg: &StatusBarConfig) {
        self.epoch.set(self.epoch.get() + 1);
        for b in [&self.left, &self.right] {
            while let Some(c) = b.first_child() {
                b.remove(&c);
            }
        }
        *self.cfg.borrow_mut() = cfg.clone();
        *self.git.borrow_mut() = GitState::default();
        self.root.set_visible(cfg.enabled);

        let mut items = Vec::new();
        for (n, w) in cfg.widgets.iter().enumerate() {
            let mut w = w.clone();
            if w.id.is_empty() {
                w.id = format!("w{n}");
            }
            let item = self.build_item(w);
            match item.cfg.side {
                Side::Left => self.left.append(&item.widget),
                Side::Right => self.right.append(&item.widget),
            }
            items.push(item);
        }
        *self.items.borrow_mut() = items;
        self.dirty.set(true);
        self.apply_all(true);
    }

    fn build_item(self: &Rc<Self>, cfg: WidgetConfig) -> Rc<Item> {
        let kind = match cfg.builtin.as_deref() {
            Some("clock") => Kind::Clock,
            Some("git") => Kind::Git,
            Some("dirs") => Kind::Dirs,
            Some(other) => {
                eprintln!("status bar: unknown builtin {other:?} (use clock, git or dirs)");
                Kind::Text
            }
            None if !cfg.menu.is_empty() => Kind::Menu,
            None if cfg.action.is_some() => Kind::Button,
            None if cfg.exec.is_some() => Kind::Exec,
            None => Kind::Text,
        };
        let classes = ["sb-item".to_owned(), class_name(&cfg.id)];
        let weak = self.me.clone();
        let mut sync = None;
        let mut branches = None;

        let (widget, label): (gtk::Widget, gtk::Label) = match kind {
            Kind::Text | Kind::Clock | Kind::Exec => {
                let l = flat_label();
                l.set_css_classes(&[&classes[0], &classes[1]]);
                (l.clone().upcast(), l)
            }
            Kind::Button => {
                let l = flat_label();
                let b = gtk::Button::builder().child(&l).css_classes([&classes[0][..], &classes[1][..], "flat"]).build();
                let action = cfg.action.clone().unwrap_or_default();
                b.connect_clicked(move |_| {
                    if let Some(sb) = weak.upgrade() {
                        (sb.host.action)(&action);
                    }
                });
                (b.upcast(), l)
            }
            Kind::Menu | Kind::Dirs => {
                let l = flat_label();
                let pop = menu_popover();
                let mb = gtk::MenuButton::builder()
                    .child(&l)
                    .popover(&pop)
                    .has_frame(false)
                    .css_classes([&classes[0][..], &classes[1][..]])
                    .build();
                let entries = cfg.menu.clone();
                let weak2 = weak.clone();
                pop.connect_show(move |pop| {
                    let Some(sb) = weak2.upgrade() else { return };
                    let rows = popover_rows();
                    if kind == Kind::Menu {
                        for e in &entries {
                            let b = row_button(&e.label);
                            let (w, action, pop) = (weak2.clone(), e.action.clone(), pop.clone());
                            b.connect_clicked(move |_| {
                                pop.popdown();
                                if let Some(sb) = w.upgrade() {
                                    (sb.host.action)(&action);
                                }
                            });
                            rows.append(&b);
                        }
                    } else {
                        let dirs = (sb.host.recent_dirs)();
                        if dirs.is_empty() {
                            rows.append(&row_button("No recent directories"));
                        }
                        for (shown, path) in dirs.into_iter().take(15) {
                            let b = row_button(&shown);
                            let (w, pop) = (weak2.clone(), pop.clone());
                            b.connect_clicked(move |_| {
                                pop.popdown();
                                if let Some(sb) = w.upgrade() {
                                    (sb.host.cd)(&path);
                                }
                            });
                            rows.append(&b);
                        }
                    }
                    pop.set_child(Some(&rows));
                });
                (mb.upcast(), l)
            }
            Kind::Git => {
                let l = flat_label();
                let pop = menu_popover();
                let mb = gtk::MenuButton::builder()
                    .child(&l)
                    .popover(&pop)
                    .has_frame(false)
                    .css_classes(["sb-git-branch"])
                    .build();
                let ui = Rc::new(self.build_branch_popover(&pop));
                branches = Some(ui);

                let sl = flat_label();
                let img = gtk::Image::from_icon_name("view-refresh-symbolic");
                let sbox = gtk::Box::builder().spacing(4).build();
                sbox.append(&img);
                sbox.append(&sl);
                let sbtn = gtk::Button::builder().child(&sbox).css_classes(["flat", "sb-git-sync"]).build();
                let weak2 = weak.clone();
                sbtn.connect_clicked(move |_| {
                    if let Some(sb) = weak2.upgrade() {
                        sb.git_sync();
                    }
                });
                sync = Some((sbtn.clone(), sl));

                let outer = gtk::Box::builder().css_classes([&classes[0][..], &classes[1][..]]).build();
                outer.append(&mb);
                outer.append(&sbtn);
                (outer.upcast(), l)
            }
        };
        widget.set_visible(false);
        Rc::new(Item {
            data: RefCell::new(ItemData {
                output: String::new(),
                when_ok: cfg.when.is_none(),
                fetched: None,
                in_flight: false,
                pending: false,
                last_interval: None,
            }),
            cfg,
            kind,
            widget,
            label,
            sync,
            branches,
            wants: Cell::new(false),
            nat_w: Cell::new(0),
        })
    }

    // -----------------------------------------------------------------------------------------
    // Layout: drop the lowest priority widgets until the rest fit
    // -----------------------------------------------------------------------------------------

    fn relayout(&self) {
        self.dirty.set(false);
        let avail = self.scroller.width();
        self.last_width.set(avail);
        let items = self.items.borrow();
        let mut cand: Vec<&Rc<Item>> = Vec::new();
        for it in items.iter() {
            if it.wants.get() {
                it.widget.set_visible(true);
                let (_, nat, _, _) = it.widget.measure(gtk::Orientation::Horizontal, -1);
                it.nat_w.set(nat);
                cand.push(it);
            } else {
                it.widget.set_visible(false);
            }
        }
        if avail <= 0 {
            return;
        }
        let sizes: Vec<(i32, i32)> = cand.iter().map(|i| (i.cfg.priority, i.nat_w.get())).collect();
        for (it, keep) in cand.iter().zip(fit(&sizes, avail)) {
            it.widget.set_visible(keep);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Data
    // -----------------------------------------------------------------------------------------

    /// The focused pane (or its directory, host, session...) changed.
    pub fn set_context(self: &Rc<Self>, ctx: Context) {
        let old = self.ctx.replace(ctx.clone());
        self.apply_all(old.cwd != ctx.cwd);
    }

    /// Something happened in the shell (a prompt appeared): re-run what depends on it.
    pub fn on_prompt(self: &Rc<Self>) {
        let items: Vec<Rc<Item>> = self.items.borrow().clone();
        for it in items.iter().filter(|i| i.cfg.refresh == Refresh::Prompt) {
            self.refresh_item(it, true);
        }
        self.refresh_git(Duration::from_millis(400));
        for it in items.iter() {
            self.render(it); // `{exit}` may have changed
        }
    }

    fn apply_all(self: &Rc<Self>, cwd_changed: bool) {
        let items: Vec<Rc<Item>> = self.items.borrow().clone();
        for it in &items {
            self.render(it);
            self.refresh_item(it, false);
        }
        if cwd_changed || self.git.borrow().last.is_none() {
            self.refresh_git(Duration::ZERO);
        }
    }

    fn needs_job(it: &Item) -> bool {
        it.kind == Kind::Exec || it.cfg.when.is_some()
    }

    fn timeout_for(&self, it: &Item) -> Duration {
        it.cfg.timeout.unwrap_or(self.cfg.borrow().timeout).0
    }

    fn refresh_item(self: &Rc<Self>, it: &Rc<Item>, force: bool) {
        if !Self::needs_job(it) || it.kind == Kind::Git {
            return;
        }
        let cwd = self.ctx.borrow().cwd.clone();
        if cwd.as_os_str().is_empty() {
            return; // no pane yet
        }
        {
            let mut d = it.data.borrow_mut();
            if d.in_flight {
                d.pending = true;
                return;
            }
            let ttl = it.cfg.cache.map_or(Duration::from_secs(30), |c| c.0);
            if !force {
                if let Some((c, t)) = &d.fetched {
                    if *c == cwd && t.elapsed() < ttl {
                        return;
                    }
                }
            }
            d.in_flight = true;
            d.last_interval = Some(Instant::now());
        }
        let (epoch, weak, it2) = (self.epoch.get(), self.me.clone(), it.clone());
        let (cfg, timeout, cwd2) = (it.cfg.clone(), self.timeout_for(it), cwd.clone());
        exec::spawn(move || run_job(&cfg, &cwd2, timeout), move |res| {
            let Some(sb) = weak.upgrade() else { return };
            if sb.epoch.get() != epoch {
                return;
            }
            let rerun = {
                let mut d = it2.data.borrow_mut();
                d.in_flight = false;
                match res {
                    Some(r) => {
                        d.when_ok = r.when_ok;
                        d.output = r.output;
                    }
                    None => d.output.clear(),
                }
                d.fetched = Some((cwd, Instant::now()));
                std::mem::take(&mut d.pending)
            };
            sb.render(&it2);
            if rerun {
                sb.refresh_item(&it2, true);
            }
        });
    }

    fn on_second(self: &Rc<Self>) {
        let items: Vec<Rc<Item>> = self.items.borrow().clone();
        for it in items.iter().filter(|i| i.kind == Kind::Clock) {
            self.render(it);
        }
        let active = self
            .root
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok())
            .is_some_and(|w| w.is_active());
        if !active {
            return;
        }
        for it in items.iter() {
            if let Some(iv) = it.cfg.interval {
                let due = it.data.borrow().last_interval.is_none_or(|t| t.elapsed() >= iv.0);
                if due {
                    self.refresh_item(it, true);
                }
            }
        }
        let iv = self.cfg.borrow().git.interval.0;
        self.refresh_git(iv);
    }

    // -----------------------------------------------------------------------------------------
    // Showing an item
    // -----------------------------------------------------------------------------------------

    fn render(&self, it: &Rc<Item>) {
        let ctx = self.ctx.borrow().clone();
        let data = it.data.borrow();
        let get = |k: &str| -> Option<String> {
            Some(match k {
                "output" => data.output.clone(),
                "path" => ctx.path.clone(),
                "dir" => ctx.dir.clone(),
                "host" => ctx.host.clone(),
                "user" => ctx.user.clone(),
                "title" => ctx.title.clone(),
                "session" => ctx.session.clone(),
                "exit" => ctx.exit.map(|e| e.to_string()).unwrap_or_default(),
                _ => return None,
            })
        };
        let mut wants = data.when_ok;
        let (text, tip) = match it.kind {
            Kind::Clock => {
                let fmt = it.cfg.format.as_deref().unwrap_or("%H:%M");
                let text = glib::DateTime::now_local()
                    .ok()
                    .and_then(|d| d.format(fmt).ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let tip = glib::DateTime::now_local().ok().and_then(|d| d.format("%A %e %B %Y, %T").ok());
                (text, tip.map(|s| s.to_string()))
            }
            Kind::Git => return self.render_git(it),
            Kind::Exec => {
                wants &= !(it.cfg.hide_empty && data.output.is_empty());
                (template::render(it.cfg.text.as_deref().unwrap_or("{output}"), &get), None)
            }
            Kind::Dirs => (template::render(it.cfg.text.as_deref().unwrap_or("{dir}"), &get), None),
            Kind::Menu | Kind::Button | Kind::Text => {
                let default = if matches!(it.kind, Kind::Text) { "" } else { &it.cfg.id };
                (template::render(it.cfg.text.as_deref().unwrap_or(default), &get), None)
            }
        };
        let shown = join(&it.cfg.icon, &text);
        wants &= !shown.is_empty();
        if it.label.label() != shown {
            it.label.set_label(&shown);
            self.dirty.set(true);
        }
        let tip = it.cfg.tooltip.as_ref().map(|t| template::render(t, &get)).or(tip);
        if it.widget.tooltip_text().map(|s| s.to_string()) != tip {
            it.widget.set_tooltip_text(tip.as_deref());
        }
        if it.wants.replace(wants) != wants {
            self.dirty.set(true);
        }
    }

    fn render_git(&self, it: &Rc<Item>) {
        let git = self.git.borrow();
        let cfg = self.cfg.borrow();
        let (Some(s), true) = (git.status.as_ref(), it.data.borrow().when_ok) else {
            if it.wants.replace(false) {
                self.dirty.set(true);
            }
            return;
        };
        let g = &cfg.git;
        let num = |n: u32| if n > 0 { n.to_string() } else { String::new() };
        let get = |k: &str| -> Option<String> {
            Some(match k {
                "branch" => s.branch.clone(),
                "upstream" => s.upstream.clone().unwrap_or_default(),
                "ahead" => num(s.ahead),
                "behind" => num(s.behind),
                "staged" => num(s.staged),
                "modified" => num(s.modified),
                "deleted" => num(s.deleted),
                "renamed" => num(s.renamed),
                "untracked" => num(s.untracked),
                "conflicted" => num(s.conflicted),
                "stashed" => num(s.stashed),
                "status" => git_status_text(s, g),
                "sync" => git_sync_text(s, g),
                "diverged" => if s.ahead > 0 && s.behind > 0 { g.icons.diverged.clone() } else { String::new() },
                "dirty" => if s.dirty() { "*".into() } else { String::new() },
                _ => return None,
            })
        };
        let text = join(&g.icon, template::render(&g.format, &get).trim());
        if it.label.label() != text {
            it.label.set_label(&text);
        }
        let mut tip = vec![format!("Branch: {}", s.branch)];
        if let Some(u) = &s.upstream {
            tip.push(format!("Upstream: {u}"));
        }
        for (n, what) in [
            (s.conflicted, "conflicted"),
            (s.staged, "staged"),
            (s.modified, "modified"),
            (s.deleted, "deleted"),
            (s.renamed, "renamed"),
            (s.untracked, "untracked"),
            (s.stashed, "stashed"),
        ] {
            if n > 0 {
                tip.push(format!("{n} {what}"));
            }
        }
        it.label.set_tooltip_text(Some(&tip.join("\n")));
        it.widget.set_css_classes(&["sb-item", &class_name(&it.cfg.id), if s.dirty() { "dirty" } else { "clean" }]);

        if let Some((btn, label)) = &it.sync {
            let show = g.sync_button && s.upstream.is_some();
            btn.set_visible(show);
            btn.set_sensitive(!git.busy);
            label.set_label(&if git.busy { "…".to_owned() } else { git_sync_text(s, g) });
            btn.set_tooltip_text(Some(&match (s.behind, s.ahead) {
                (0, 0) => "Fetch from the remote".to_owned(),
                (b, 0) => format!("Pull {b} commit(s)"),
                (0, a) => format!("Push {a} commit(s)"),
                (b, a) => format!("Pull {b}, then push {a} commit(s)"),
            }));
        }
        if it.wants.replace(true) != true {
            self.dirty.set(true);
        }
        self.dirty.set(true);
    }

    // -----------------------------------------------------------------------------------------
    // Git
    // -----------------------------------------------------------------------------------------

    fn has_git(&self) -> bool {
        self.items.borrow().iter().any(|i| i.kind == Kind::Git)
    }

    /// Refresh `git status` unless it was refreshed less than `min_age` ago.
    pub fn refresh_git(self: &Rc<Self>, min_age: Duration) {
        if !self.has_git() || self.ctx.borrow().cwd.as_os_str().is_empty() {
            return;
        }
        {
            let mut g = self.git.borrow_mut();
            if g.in_flight {
                g.pending = true;
                return;
            }
            if g.last.is_some_and(|t| t.elapsed() < min_age) {
                return;
            }
            g.in_flight = true;
            g.last = Some(Instant::now());
        }
        let (cwd, timeout) = (self.ctx.borrow().cwd.clone(), self.cfg.borrow().git.timeout.0);
        let (epoch, weak) = (self.epoch.get(), self.me.clone());
        exec::spawn(move || git::status(&cwd, timeout), move |res| {
            let Some(sb) = weak.upgrade() else { return };
            if sb.epoch.get() != epoch {
                return;
            }
            let rerun = {
                let mut g = sb.git.borrow_mut();
                g.in_flight = false;
                g.last = Some(Instant::now());
                g.status = match res {
                    Some(git::Fetched::Repo(s)) => Some(s),
                    _ => None, // no git, not a repository, or too slow: hide the widget
                };
                std::mem::take(&mut g.pending)
            };
            sb.render_git_items();
            if rerun {
                sb.refresh_git(Duration::ZERO);
            }
        });
    }

    fn render_git_items(&self) {
        let items: Vec<Rc<Item>> = self.items.borrow().iter().filter(|i| i.kind == Kind::Git).cloned().collect();
        for it in items {
            self.render_git(&it);
        }
    }

    /// Run git commands one after another in the background, then show how it went.
    fn git_op(self: &Rc<Self>, label: &str, steps: Vec<Vec<String>>, network: bool) {
        if self.git.borrow().busy {
            return;
        }
        self.git.borrow_mut().busy = true;
        self.render_git_items();
        let cwd = self.ctx.borrow().cwd.clone();
        let timeout = {
            let c = self.cfg.borrow();
            if network { c.git.network_timeout.0 } else { c.git.timeout.0.max(Duration::from_secs(10)) }
        };
        let (epoch, weak, label) = (self.epoch.get(), self.me.clone(), label.to_owned());
        exec::spawn(
            {
                let label = label.clone();
                move || -> Result<(), String> {
                    for step in &steps {
                        let args: Vec<&str> = step.iter().map(String::as_str).collect();
                        git::run(&cwd, &args, timeout).map_err(|e| format!("{label} failed: {e}"))?;
                    }
                    Ok(())
                }
            },
            move |res| {
                let Some(sb) = weak.upgrade() else { return };
                if sb.epoch.get() != epoch {
                    return;
                }
                sb.git.borrow_mut().busy = false;
                let msg = match res {
                    Some(Ok(())) => format!("{label}: done"),
                    Some(Err(e)) => e,
                    None => format!("{label} failed"),
                };
                (sb.host.toast)(&msg);
                sb.refresh_git(Duration::ZERO);
                sb.render_git_items();
            },
        );
    }

    fn git_sync(self: &Rc<Self>) {
        let (ahead, behind) = self.git.borrow().status.as_ref().map_or((0, 0), |s| (s.ahead, s.behind));
        let mut steps: Vec<Vec<String>> = Vec::new();
        let s = |a: &[&str]| a.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        if behind > 0 {
            steps.push(s(&["pull", "--ff-only"]));
        }
        if ahead > 0 {
            steps.push(s(&["push"]));
        }
        if steps.is_empty() {
            steps.push(s(&["fetch", "--prune"]));
        }
        self.git_op("Sync", steps, true);
    }

    fn build_branch_popover(self: &Rc<Self>, pop: &gtk::Popover) -> BranchUi {
        let entry = gtk::SearchEntry::builder().placeholder_text("Switch branch or create one").build();
        let list = popover_rows();
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .min_content_width(300)
            .min_content_height(60)
            .max_content_height(320)
            .propagate_natural_height(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let actions = gtk::Box::builder().spacing(4).homogeneous(true).build();
        for (label, steps) in [
            ("Fetch", vec!["fetch", "--prune"]),
            ("Pull", vec!["pull", "--ff-only"]),
            ("Push", vec!["push"]),
        ] {
            let b = gtk::Button::builder().label(label).css_classes(["flat"]).build();
            let (weak, pop) = (self.me.clone(), pop.clone());
            b.connect_clicked(move |_| {
                pop.popdown();
                if let Some(sb) = weak.upgrade() {
                    sb.git_op(label, vec![steps.iter().map(|x| (*x).to_owned()).collect()], true);
                }
            });
            actions.append(&b);
        }
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        content.set_margin_top(6);
        content.set_margin_bottom(6);
        content.set_margin_start(6);
        content.set_margin_end(6);
        content.append(&entry);
        content.append(&scroll);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&actions);
        pop.set_child(Some(&content));

        let ui = BranchUi { entry: entry.clone(), list, branches: RefCell::default(), first: RefCell::default() };

        // (Re)load the branches whenever the popover opens.
        let weak = self.me.clone();
        pop.connect_show(move |pop| {
            let Some(sb) = weak.upgrade() else { return };
            let Some(ui) = sb.items.borrow().iter().find_map(|i| i.branches.clone()) else { return };
            ui.entry.set_text("");
            ui.entry.grab_focus();
            while let Some(c) = ui.list.first_child() {
                ui.list.remove(&c);
            }
            ui.list.append(&gtk::Label::builder().label("Loading…").margin_top(8).margin_bottom(8).build());
            let (cwd, timeout) = (sb.ctx.borrow().cwd.clone(), sb.cfg.borrow().git.timeout.0.max(Duration::from_secs(5)));
            let (weak, pop) = (sb.me.clone(), pop.clone());
            exec::spawn(move || git::branches(&cwd, timeout), move |res| {
                let Some(sb) = weak.upgrade() else { return };
                let Some(ui) = sb.items.borrow().iter().find_map(|i| i.branches.clone()) else { return };
                match res {
                    Some(Ok(b)) => *ui.branches.borrow_mut() = b,
                    Some(Err(e)) => {
                        (sb.host.toast)(&format!("Branches: {e}"));
                        pop.popdown();
                        return;
                    }
                    None => return,
                }
                sb.refill_branches(&ui, &pop);
            });
        });

        let weak = self.me.clone();
        let pop2 = pop.clone();
        entry.connect_search_changed(move |_| {
            let Some(sb) = weak.upgrade() else { return };
            let Some(ui) = sb.items.borrow().iter().find_map(|i| i.branches.clone()) else { return };
            sb.refill_branches(&ui, &pop2);
        });
        let weak = self.me.clone();
        let pop3 = pop.clone();
        entry.connect_activate(move |_| {
            let Some(sb) = weak.upgrade() else { return };
            let Some(ui) = sb.items.borrow().iter().find_map(|i| i.branches.clone()) else { return };
            let pick = ui.first.borrow_mut().take();
            if let Some(p) = pick {
                pop3.popdown();
                sb.pick_branch(p);
            }
        });
        ui
    }

    fn refill_branches(self: &Rc<Self>, ui: &Rc<BranchUi>, pop: &gtk::Popover) {
        while let Some(c) = ui.list.first_child() {
            ui.list.remove(&c);
        }
        let q = ui.entry.text().trim().to_owned();
        let ql = q.to_lowercase();
        *ui.first.borrow_mut() = None;
        let branches = ui.branches.borrow().clone();
        let mut shown = 0;
        for b in branches.iter().filter(|b| ql.is_empty() || b.name.to_lowercase().contains(&ql)) {
            if shown >= 100 {
                break;
            }
            shown += 1;
            let text = format!("{}{}", if b.current { "✓ " } else { "   " }, b.name);
            let btn = row_button(&text);
            if b.current {
                btn.set_sensitive(false);
            } else {
                if ui.first.borrow().is_none() {
                    *ui.first.borrow_mut() = Some(Pick::Switch(b.switch_name().to_owned()));
                }
                let (weak, pop, name) = (self.me.clone(), pop.clone(), b.switch_name().to_owned());
                btn.connect_clicked(move |_| {
                    pop.popdown();
                    if let Some(sb) = weak.upgrade() {
                        sb.pick_branch(Pick::Switch(name.clone()));
                    }
                });
            }
            ui.list.append(&btn);
        }
        if !q.is_empty() && !branches.iter().any(|b| b.name == q) {
            let btn = row_button(&format!("+  Create branch “{q}”"));
            if ui.first.borrow().is_none() {
                *ui.first.borrow_mut() = Some(Pick::Create(q.clone()));
            }
            let (weak, pop, name) = (self.me.clone(), pop.clone(), q);
            btn.connect_clicked(move |_| {
                pop.popdown();
                if let Some(sb) = weak.upgrade() {
                    sb.pick_branch(Pick::Create(name.clone()));
                }
            });
            ui.list.append(&btn);
        }
    }

    fn pick_branch(self: &Rc<Self>, pick: Pick) {
        let (label, args) = match pick {
            Pick::Switch(n) => (format!("Switch to {n}"), vec!["switch".to_owned(), n]),
            Pick::Create(n) => (format!("Create {n}"), vec!["switch".to_owned(), "-c".to_owned(), n]),
        };
        self.git_op(&label, vec![args], false);
    }
}

/// CSS for the per-widget colours and the bar font.
pub fn css(cfg: &StatusBarConfig) -> String {
    let mut css = String::new();
    let f = &cfg.font;
    let mut decl = String::new();
    if let Some(family) = &f.family {
        decl += &format!("font-family: \"{}\"; ", family.replace('"', ""));
    }
    if let Some(size) = f.size {
        decl += &format!("font-size: {size}pt; ");
    }
    if let Some(w) = f.weight {
        decl += &format!("font-weight: {}; ", w.0);
    }
    if f.italic {
        decl += "font-style: italic; ";
    }
    if !decl.is_empty() {
        css += &format!("window .status-bar, window .status-bar label, window popover .sb-row label {{ {decl}}}\n");
    }
    for w in &cfg.widgets {
        let id = if w.id.is_empty() { continue } else { class_name(&w.id) };
        for (mode, dark) in [("dark", true), ("light", false)] {
            let mut decl = String::new();
            if let Some(c) = w.color.and_then(|c| c.resolve(dark)) {
                decl += &format!("color: {}; ", c.css());
            }
            if let Some(b) = w.background.and_then(|b| b.resolve(dark)) {
                decl += &format!("background-color: {}; ", b.css());
            }
            if !decl.is_empty() {
                css += &format!("window.{mode} .status-bar .{id}, window.{mode} .status-bar .{id} label {{ {decl}}}\n");
            }
        }
    }
    css
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_files_are_found_in_parents() {
        let base = std::env::temp_dir().join(format!("paua-when-{}", std::process::id()));
        let deep = base.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(base.join("package.json"), "{}").unwrap();
        let w = |files: &[&str], not: &[&str]| WhenConfig {
            files: files.iter().map(|s| s.to_string()).collect(),
            not_files: not.iter().map(|s| s.to_string()).collect(),
        };
        assert!(when_matches(&w(&["package.json"], &[]), &deep));
        assert!(when_matches(&w(&["nope", "package.json"], &[]), &deep)); // any of
        assert!(!when_matches(&w(&["Cargo.toml-not-here"], &[]), &deep));
        assert!(!when_matches(&w(&["package.json"], &["package.json"]), &deep));
        assert!(when_matches(&w(&[], &["bun.lockb-not-here"]), &deep));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn git_text_pieces() {
        let cfg = crate::config::GitConfig::default();
        let s = git::Status { staged: 2, modified: 1, untracked: 3, ahead: 1, behind: 4, ..Default::default() };
        assert_eq!(git_status_text(&s, &cfg), "●2 ✚1 ?3");
        assert_eq!(git_sync_text(&s, &cfg), "⇅ ↑1 ↓4");
        let behind_only = git::Status { behind: 2, ..Default::default() };
        assert_eq!(git_sync_text(&behind_only, &cfg), "↓2");
        let no_c = crate::config::GitConfig { counts: false, ..Default::default() };
        assert_eq!(git_sync_text(&s, &no_c), "⇅");
        let no_counts = crate::config::GitConfig { counts: false, ..cfg };
        assert_eq!(git_status_text(&s, &no_counts), "● ✚ ?");
        assert_eq!(git_status_text(&git::Status::default(), &no_counts), "");
    }

    #[test]
    fn lowest_priority_drops_first() {
        // (priority, width): total with gaps = 24 + (100+2) + (80+2) + (60+2) = 270
        let items = [(80, 100), (50, 80), (10, 60)];
        assert_eq!(fit(&items, 1000), vec![true, true, true]);
        assert_eq!(fit(&items, 270), vec![true, true, true]);
        assert_eq!(fit(&items, 269), vec![true, true, false]);
        assert_eq!(fit(&items, 190), vec![true, false, false]);
        assert_eq!(fit(&items, 10), vec![false, false, false]);
        // equal priorities: the later one goes first
        assert_eq!(fit(&[(5, 100), (5, 100)], 150), vec![true, false]);
    }

    #[test]
    fn helpers() {
        assert_eq!(class_name("my widget!"), "sb-my_widget_");
        assert_eq!(join("", "x"), "x");
        assert_eq!(join("i", ""), "i");
        assert_eq!(join("i", "x"), "i x");
    }
}
