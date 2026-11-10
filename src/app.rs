//! Application state: windows, panes, the loaded configs, and live reload.
//!
//! Layout model: every window is a full app window (title bar, status bar, workbench). Each owns
//! a tree under `Win::root` (a pane card, or `Paned` splits of pane cards), its own frame colour,
//! and its own "active" pane (the most recently focused one in that window). Popping a pane out
//! creates another such window. The app ends when the last window closes.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::{self, DirConfig, UserConfig};
use crate::exec;
use crate::frame::{Frame, TabUnderline};
use crate::header::{self, PaneHeader};
use crate::palette::{self, PaletteItem};
use crate::searchbox::SearchBox;
use crate::session::{self, Kind, SessionInfo};
use crate::shortcuts::{self, Bindings};
use crate::statusbar::{self, StatusBar};
use crate::themes;
use crate::terminal::{FontWeights, Rgb, TermConfig, TermView};

const WINDOW_TITLE: &str = "Paua";

thread_local! {
    /// Keeps the `App` alive for the whole run (windows only hold weak references to it).
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// A `.paua.yaml` that applies to a pane (the nearest one above its working directory).
struct DirEntry {
    path: PathBuf,
    cfg: DirConfig,
}

/// A top-level window.
struct Win {
    window: adw::ApplicationWindow,
    /// Holds this window's tree of panes/splits.
    root: gtk::Box,
    toasts: adw::ToastOverlay,
    frame: Frame,
    /// The title-bar search box, and the pane its find currently targets.
    search: Rc<SearchBox>,
    find_pane: RefCell<Weak<TerminalTab>>,
    status: Rc<StatusBar>,
}

struct Pane {
    card: gtk::Box,
    header_row: gtk::Box,
    new_tab_button: gtk::Button,
    headers: gtk::Box,
    header_viewport: gtk::Viewport,
    stack: gtk::Stack,
    win: RefCell<Rc<Win>>,
    tabs: RefCell<Vec<Rc<TerminalTab>>>,
    /// Higher = focused more recently. The highest in a window is that window's active pane.
    focus_seq: Cell<u64>,
}

impl Pane {
    fn selected_tab(&self) -> Option<Rc<TerminalTab>> {
        let child = self.stack.visible_child()?;
        self.tabs.borrow().iter().find(|tab| tab.term.upcast_ref::<gtk::Widget>() == &child).cloned()
    }

    fn select_tab(&self, tab: &TerminalTab) {
        self.stack.set_visible_child(&tab.term);
        for candidate in self.tabs.borrow().iter() {
            candidate.header.set_selected(std::ptr::eq(candidate.as_ref(), tab));
        }
        self.reveal_tab(tab);
    }

    fn reveal_tab(&self, tab: &TerminalTab) {
        if !self.header_viewport.is_mapped() {
            return;
        }
        // Adding, removing, or restyling a header changes its allocation. Reveal it after layout.
        let (headers, header) = (self.headers.downgrade(), tab.header.widget().downgrade());
        self.header_viewport.add_tick_callback(move |viewport, _| {
            let (viewport, headers, header) = (viewport.downgrade(), headers.clone(), header.clone());
            glib::idle_add_local_once(move || {
                let (Some(viewport), Some(headers), Some(header)) = (viewport.upgrade(), headers.upgrade(), header.upgrade()) else { return };
                let bounds = header.compute_bounds(&headers).filter(|_| header.has_css_class("selected"));
                if let (Some(bounds), Some(adjustment)) = (bounds, viewport.hadjustment()) {
                    adjustment.clamp_page(f64::from(bounds.x()), f64::from(bounds.x() + bounds.width()));
                }
            });
            glib::ControlFlow::Break
        });
    }

    fn update_tab_headers(&self) {
        let tabs = self.tabs.borrow();
        let tabbed = tabs.len() > 1;
        if tabbed {
            self.header_row.add_css_class("multiple-tabs");
        } else {
            self.header_row.remove_css_class("multiple-tabs");
        }
        for tab in tabs.iter() {
            tab.header.set_tabbed(tabbed);
        }
    }

    fn reorder_tab(&self, source: &gtk::Widget, target: &TerminalTab, after: bool) -> bool {
        let mut tabs = self.tabs.borrow_mut();
        let Some(from) = tabs.iter().position(|tab| tab.header.widget() == source) else { return false };
        let Some(to) = tabs.iter().position(|tab| std::ptr::eq(tab.as_ref(), target)) else { return false };
        let insert = to + usize::from(after);
        let index = insert - usize::from(from < insert);
        if from != index {
            let tab = tabs.remove(from);
            let previous = index.checked_sub(1).map(|i| tabs[i].header.widget());
            self.headers.reorder_child_after(source, previous);
            tabs.insert(index, tab);
        }
        true
    }
}

struct TerminalTab {
    term: TermView,
    header: PaneHeader,
    pane: Weak<Pane>,
    cwd: RefCell<PathBuf>,
    host: RefCell<String>,
    title: RefCell<String>,
    /// What the process tree says (ssh / sudo...); the shell's reported host is added on top.
    session: RefCell<SessionInfo>,
    dir: RefCell<Option<DirEntry>>,
    /// Project colour from the nearest `.vscode/settings.json` (read when the directory is
    /// entered and when the pane is focused; not watched).
    vscode_color: RefCell<Option<Rgb>>,
    /// Exit status of the last command (shells with OSC 133).
    last_exit: Cell<Option<i32>>,
}

impl TerminalTab {
    fn win(&self) -> Option<Rc<Win>> {
        self.pane.upgrade().map(|pane| pane.win.borrow().clone())
    }
}

/// Buttons in the header handle their own actions without switching or dragging the tab.
fn tab_header_hit(header: &gtk::Widget, x: f64, y: f64) -> bool {
    let mut widget = header.pick(x, y, gtk::PickFlags::DEFAULT);
    while let Some(hit) = widget {
        if &hit == header {
            return true;
        }
        if hit.is::<gtk::Button>() {
            return false;
        }
        widget = hit.parent();
    }
    false
}

pub struct App {
    gtk_app: adw::Application,
    wins: RefCell<Vec<Rc<Win>>>,
    next_win_id: Cell<u32>,
    cfg: RefCell<UserConfig>,
    shortcuts: RefCell<Rc<Bindings>>,
    panes: RefCell<Vec<Rc<Pane>>>,
    /// The most recently focused terminal in any window (drives `.paua.yaml` file watching).
    active: RefCell<Weak<TerminalTab>>,
    focus_counter: Cell<u64>,
    /// Recently visited directories, newest first (for the palette).
    recent_dirs: RefCell<Vec<PathBuf>>,
    /// 0 = follow the system, 1 = light, 2 = dark.
    scheme: Cell<usize>,
    /// Header colours from the config (generated CSS).
    header_css: gtk::CssProvider,
    user_monitor: RefCell<Vec<gio::FileMonitor>>,
    dir_monitor: RefCell<Vec<gio::FileMonitor>>,
    user_gen: Cell<u32>,
    dir_gen: Cell<u32>,
}

/// Header text for `{session}` and one badge `(text, css kind)` per detected session.
fn session_labels(cfg: &UserConfig, info: &SessionInfo) -> (String, Vec<(String, &'static str)>) {
    let mut plain = Vec::new();
    let mut badges = Vec::new();
    for item in &info.items {
        let kc = cfg.session.kind(item.kind);
        let text = session::format_label(&kc.label, item);
        if text.is_empty() {
            continue;
        }
        plain.push(text.clone());
        if cfg.session.badges {
            badges.push((format!("{}{text}", kc.icon), item.kind.name()));
        }
    }
    (plain.join(" \u{b7} "), badges)
}

/// Scrollbar and button styles for the current light/dark mode.
fn scroll_styles(cfg: &UserConfig) -> (crate::terminal::ScrollbarStyle, crate::terminal::ButtonStyle) {
    let dark = adw::StyleManager::default().is_dark();
    (cfg.terminal.scrollbar.style(dark), cfg.terminal.scroll_to_bottom_button.style(dark))
}

fn font_weights(cfg: &UserConfig) -> FontWeights {
    FontWeights {
        regular: cfg.font.weight.map(|w| w.0),
        bold: cfg.font.bold_weight.map(|w| w.0),
    }
}

/// Size of the first monitor in logical pixels (used to resolve `%` window sizes).
fn monitor_size() -> (i32, i32) {
    gdk::Display::default()
        .and_then(|d| d.monitors().item(0))
        .and_then(|m| m.downcast::<gdk::Monitor>().ok())
        .map(|m| {
            let g = m.geometry();
            (g.width(), g.height())
        })
        .unwrap_or((1920, 1080))
}

/// `(label, window action)` sections of the title-bar menus.
const MENUS: &[(&str, &[&[(&str, &str)]])] = &[
    (
        "_File",
        &[
            &[("_New Window", "win.new-window")],
            &[("New _Tab", "win.new-tab"), ("Close Ta_b", "win.close-tab"), ("Previous Tab", "win.previous-tab"), ("Next Tab", "win.next-tab")],
            &[("Split _Right", "win.split-right"), ("Split _Down", "win.split-down")],
            &[("Open Pane in New _Window", "win.pop-out"), ("_Close Pane", "win.close-pane")],
            &[("Open Config _File", "win.open-config"), ("_Reload Config", "win.reload-config")],
            &[("_Quit", "app.quit")],
        ],
    ),
    (
        "_Edit",
        &[
            &[("_Copy", "win.copy"), ("_Paste", "win.paste")],
            &[("Copy Working _Directory", "win.copy-path"), ("C_lear Terminal", "win.clear")],
            &[("_Find in Output", "win.find"), ("Command _Palette", "win.palette")],
        ],
    ),
    ("_View", &[&[("Cycle _Colour Scheme", "win.cycle-scheme")]]),
    ("_Help", &[&[("_Keyboard Shortcuts", "win.keyboard-shortcuts")], &[("_About Paua", "win.about")]]),
];

fn build_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for (title, sections) in MENUS {
        let submenu = gio::Menu::new();
        for items in *sections {
            let section = gio::Menu::new();
            for (label, action) in *items {
                section.append(Some(label), Some(action));
            }
            submenu.append_section(None, &section);
        }
        menu.append_submenu(Some(title), &submenu);
    }
    menu
}

/// Toggle the `light`/`dark` classes the stylesheet keys off.
fn apply_mode(window: &adw::ApplicationWindow, dark: bool) {
    let (add, remove) = if dark { ("dark", "light") } else { ("light", "dark") };
    window.remove_css_class(remove);
    window.add_css_class(add);
}

impl App {
    /// Build the first window and everything in it, then present it.
    pub fn build(gtk_app: &adw::Application) {
        let (cfg, load_error) = match config::load_user() {
            Ok(c) => (c, None),
            Err(e) => (UserConfig::default(), Some(e)),
        };
        Self::build_with_config(gtk_app, cfg, load_error);
    }

    fn build_with_config(gtk_app: &adw::Application, cfg: UserConfig, load_error: Option<String>) -> Rc<Self> {
        let (cfg, shortcuts, load_error) = match Bindings::resolve(&cfg.shortcuts) {
            Ok(bindings) => (cfg, bindings, load_error),
            Err(error) => {
                let cfg = UserConfig::default();
                let bindings = Bindings::resolve(&cfg.shortcuts).expect("valid default shortcuts");
                (cfg, bindings, Some(error))
            }
        };
        shortcuts.install(gtk_app, None);
        let header_css = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().expect("no display"),
            &header_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let maximized = cfg.window.maximized;
        let app = Rc::new(App {
            gtk_app: gtk_app.clone(),
            wins: RefCell::default(),
            next_win_id: Cell::new(0),
            cfg: RefCell::new(cfg),
            shortcuts: RefCell::new(Rc::new(shortcuts)),
            panes: RefCell::default(),
            active: RefCell::default(),
            focus_counter: Cell::new(0),
            recent_dirs: RefCell::default(),
            scheme: Cell::new(0),
            header_css,
            user_monitor: RefCell::default(),
            dir_monitor: RefCell::default(),
            user_gen: Cell::new(0),
            dir_gen: Cell::new(0),
        });
        APP.with(|a| *a.borrow_mut() = Some(app.clone()));
        app.apply_header_css();

        // --- Light/dark awareness (all windows) ---
        {
            let weak = Rc::downgrade(&app);
            adw::StyleManager::default().connect_dark_notify(move |sm| {
                if let Some(app) = weak.upgrade() {
                    for win in app.all_wins() {
                        apply_mode(&win.window, sm.is_dark());
                    }
                    let (theme, _) = themes::resolve(&app.cfg.borrow().terminal, sm.is_dark());
                    let (bar, btn) = scroll_styles(&app.cfg.borrow());
                    let find = app.cfg.borrow().search.style(sm.is_dark());
                    let urls = app.cfg.borrow().terminal.urls.style(sm.is_dark());
                    for p in app.all_tabs() {
                        p.term.set_theme(&theme);
                        p.term.set_scroll_style(bar, btn);
                        p.term.set_find_style(find);
                        p.term.set_url_style(urls.clone());
                    }
                    app.refresh_frames();
                }
            });
        }

        // The first window starts with a single pane.
        let win = app.create_window(maximized);
        let (first_pane, first_tab) = app.new_pane(&win, None);
        win.root.append(&first_pane.card);
        first_pane.focus_seq.set(app.next_focus_seq());
        *app.active.borrow_mut() = Rc::downgrade(&first_tab);

        app.update_pane_ui();
        app.refresh_frames();
        app.setup_user_monitor();
        app.watch_dir();

        if let Some(e) = load_error {
            app.toast(&format!("Config error: {e}"));
        }
        if let (_, Some(warning)) = themes::resolve(&app.cfg.borrow().terminal, adw::StyleManager::default().is_dark()) {
            app.toast(&format!("Config error: {warning}"));
        }

        win.window.present();
        first_tab.term.grab_focus();
        app
    }

    fn all_wins(&self) -> Vec<Rc<Win>> {
        self.wins.borrow().clone()
    }

    fn next_focus_seq(&self) -> u64 {
        let n = self.focus_counter.get() + 1;
        self.focus_counter.set(n);
        n
    }

    // -----------------------------------------------------------------------------------------
    // Windows
    // -----------------------------------------------------------------------------------------

    /// A full app window: title bar (icon, menu, search, colour scheme, split buttons), an empty
    /// workbench, and a status bar. The caller puts a pane into `root`.
    fn create_window(self: &Rc<Self>, maximized: bool) -> Rc<Win> {
        let id = self.next_win_id.get();
        self.next_win_id.set(id + 1);

        // --- Title bar: icon + menu bar (start), command-centre search (title), controls (end) ---
        let header = adw::HeaderBar::new();
        header.add_css_class("flat");
        header.pack_start(
            &gtk::Image::builder()
                .icon_name(crate::APP_ID)
                .pixel_size(20)
                .margin_start(6)
                .margin_end(2)
                .build(),
        );
        header.pack_start(&gtk::PopoverMenuBar::from_model(Some(&build_menu())));
        let search = Rc::new(SearchBox::new(self.shortcuts.borrow().clone()));
        {
            let cfg = self.cfg.borrow();
            search.set_defaults(cfg.search.case, cfg.search.regex);
            search.set_max_results(cfg.palette.max_results);
        }
        header.set_title_widget(Some(
            &adw::Clamp::builder().maximum_size(520).child(&search.root).build(),
        ));
        let scheme_button = gtk::Button::from_icon_name("weather-clear-night-symbolic");
        scheme_button.set_tooltip_text(Some("Cycle colour scheme: system / light / dark"));
        let split_down = gtk::Button::from_icon_name("split-down-symbolic");
        split_down.set_tooltip_text(Some("Split down"));
        let split_right = gtk::Button::from_icon_name("split-right-symbolic");
        split_right.set_tooltip_text(Some("Split right"));
        // pack_end stacks from the window controls inwards, so pack in reverse of reading order:
        // [colour scheme] [split right] [split down] [window controls]
        header.pack_end(&split_down);
        header.pack_end(&split_right);
        header.pack_end(&scheme_button);

        // --- Window layout: title bar / workbench / status bar ---
        let toasts = adw::ToastOverlay::new();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["workbench"])
            .vexpand(true)
            .build();
        toasts.set_child(Some(&root));
        let view = adw::ToolbarView::new();
        view.set_top_bar_style(adw::ToolbarStyle::Flat);
        view.set_bottom_bar_style(adw::ToolbarStyle::Flat);
        view.add_top_bar(&header);
        view.set_content(Some(&toasts));
        let win_slot: Rc<RefCell<Weak<Win>>> = Rc::default();
        let status = StatusBar::new(self.status_host(&win_slot));
        status.configure(&self.cfg.borrow().status_bar);
        view.add_bottom_bar(&status.root);

        let (mon_w, mon_h) = monitor_size();
        let (width, height) = {
            let cfg = self.cfg.borrow();
            (cfg.window.width.resolve(mon_w), cfg.window.height.resolve(mon_h))
        };
        let window = adw::ApplicationWindow::builder()
            .application(&self.gtk_app)
            .title(WINDOW_TITLE)
            .default_width(width)
            .default_height(height)
            .maximized(maximized)
            .content(&view)
            .build();
        window.add_css_class("themed");
        apply_mode(&window, adw::StyleManager::default().is_dark());

        let frame = Frame::new(id);
        frame.set_backdrop(self.cfg.borrow().window.inactive_dim);
        let win = Rc::new(Win {
            window: window.clone(),
            root,
            toasts,
            frame,
            search: search.clone(),
            find_pane: RefCell::default(),
            status,
        });
        *win_slot.borrow_mut() = Rc::downgrade(&win);

        // Search box <-> the window's active pane.
        {
            let (app, w) = (Rc::downgrade(self), Rc::downgrade(&win));
            search.connect_open(move |_mode| {
                if let (Some(app), Some(win)) = (app.upgrade(), w.upgrade()) {
                    app.begin_find(&win);
                }
            });
            let (app, w) = (Rc::downgrade(self), Rc::downgrade(&win));
            search.connect_items(move || match (app.upgrade(), w.upgrade()) {
                (Some(app), Some(win)) => app.palette_items(&win),
                _ => Vec::new(),
            });
            let (app, w) = (Rc::downgrade(self), Rc::downgrade(&win));
            search.connect_query(move |opts| {
                if let (Some(app), Some(win)) = (app.upgrade(), w.upgrade()) {
                    app.run_find(&win, opts);
                }
            });
            let (app, w) = (Rc::downgrade(self), Rc::downgrade(&win));
            search.connect_step(move |forward| {
                if let (Some(app), Some(win)) = (app.upgrade(), w.upgrade()) {
                    app.step_find(&win, forward);
                }
            });
            let (app, w) = (Rc::downgrade(self), Rc::downgrade(&win));
            search.connect_close(move || {
                if let (Some(app), Some(win)) = (app.upgrade(), w.upgrade()) {
                    app.end_find(&win);
                }
            });
            for action in shortcuts::window_actions() {
                let (app, w) = (Rc::downgrade(self), Rc::downgrade(&win));
                window.add_action_entries([gio::ActionEntry::builder(&action.gio_name())
                    .activate(move |_, _, _| {
                        if let (Some(app), Some(win)) = (app.upgrade(), w.upgrade()) {
                            app.run_action(&win, action.id);
                        }
                    })
                    .build()]);
            }
            let (app, w) = (Rc::downgrade(self), Rc::downgrade(&win));
            window.add_action_entries([gio::ActionEntry::builder("run-command")
                .parameter_type(Some(glib::VariantTy::STRING))
                .activate(move |_, _, target| {
                    let (Some(app), Some(win), Some(target)) = (app.upgrade(), w.upgrade(), target.and_then(glib::Variant::str)) else { return };
                    if shortcuts::is_command(target) {
                        app.run_action(&win, target);
                    }
                })
                .build()]);
        }

        // Buttons act on the window they are in.
        for (button, orientation) in [
            (&split_right, gtk::Orientation::Horizontal),
            (&split_down, gtk::Orientation::Vertical),
        ] {
            let (app, win) = (Rc::downgrade(self), Rc::downgrade(&win));
            button.connect_clicked(move |_| {
                if let (Some(app), Some(win)) = (app.upgrade(), win.upgrade()) {
                    app.split(&win, orientation);
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            scheme_button.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.cycle_scheme();
                }
            });
        }

        // Closing a window closes its panes (ending their shells). The app ends with the last one.
        {
            let (app, win) = (Rc::downgrade(self), Rc::downgrade(&win));
            window.connect_close_request(move |_| {
                if let (Some(app), Some(win)) = (app.upgrade(), win.upgrade()) {
                    let panes = app.panes_in(&win);
                    app.panes.borrow_mut().retain(|p| !Rc::ptr_eq(&p.win.borrow(), &win));
                    for pane in panes {
                        for tab in pane.tabs.borrow_mut().drain(..) {
                            tab.term.shutdown();
                        }
                    }
                    app.wins.borrow_mut().retain(|w| !Rc::ptr_eq(w, &win));
                    if app.wins.borrow().is_empty() {
                        app.gtk_app.quit();
                    } else {
                        app.update_pane_ui();
                        app.watch_dir();
                    }
                }
                glib::Propagation::Proceed
            });
        }

        self.wins.borrow_mut().push(win.clone());
        win
    }

    /// Cycle System -> Light -> Dark (handy for testing; a real app would put this in a menu).
    fn cycle_scheme(&self) {
        let next = (self.scheme.get() + 1) % 3;
        self.scheme.set(next);
        adw::StyleManager::default().set_color_scheme(match next {
            0 => adw::ColorScheme::Default,
            1 => adw::ColorScheme::ForceLight,
            _ => adw::ColorScheme::ForceDark,
        });
    }

    /// A window's title (shown in the overview / alt-tab) follows its active pane's header.
    fn refresh_window_title(&self, win: &Rc<Win>) {
        let title = self
            .win_active(win)
            .map(|p| p.header.full_text())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| WINDOW_TITLE.to_owned());
        win.window.set_title(Some(&title));
    }

    // -----------------------------------------------------------------------------------------
    // Panes
    // -----------------------------------------------------------------------------------------

    fn term_config(&self) -> TermConfig {
        let cfg = self.cfg.borrow();
        TermConfig {
            font: cfg.font.pango_string(),
            weights: font_weights(&cfg),
            font_features: cfg.font.features.clone(),
            scrollback: cfg.terminal.scrollback,
            shell: cfg.terminal.shell.clone(),
            cwd: None,
            env: cfg.terminal.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        }
    }

    /// A rounded card with one row of headers above the terminal stack. The caller attaches it.
    fn new_pane(self: &Rc<Self>, win: &Rc<Win>, cwd: Option<PathBuf>) -> (Rc<Pane>, Rc<TerminalTab>) {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["pane"])
            .overflow(gtk::Overflow::Hidden) // clip children to the rounded corners
            .hexpand(true)
            .vexpand(true)
            .build();
        let headers = gtk::Box::builder()
            .accessible_role(gtk::AccessibleRole::TabList)
            .homogeneous(true)
            .spacing(1)
            .build();
        let header_viewport = gtk::Viewport::builder().child(&headers).scroll_to_focus(true).build();
        let header_scroll = gtk::ScrolledWindow::builder()
            .child(&header_viewport)
            // Let the row scroll without a scrollbar over the header buttons.
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .hexpand(true)
            .build();
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let adjustment = header_scroll.hadjustment();
        wheel.connect_scroll(move |wheel, _, dy| {
            if dy == 0.0 || wheel.unit() != gdk::ScrollUnit::Wheel || adjustment.upper() <= adjustment.page_size() {
                return glib::Propagation::Proceed;
            }
            adjustment.set_value(adjustment.value() + dy * 52.0);
            glib::Propagation::Stop
        });
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        header_scroll.add_controller(wheel);
        let add = PaneHeader::button(&self.cfg.borrow().panes.tabs.new_tab_icon, "New tab");
        self.update_new_tab_tooltip(&add);
        add.add_css_class("pane-new-tab");
        let header_row = gtk::Box::builder().css_classes(["pane-header-row"]).build();
        header_row.append(&header_scroll);
        header_row.append(&add);
        let stack = gtk::Stack::builder()
            .hexpand(true)
            .vexpand(true)
            .build();
        card.append(&header_row);
        card.append(&stack);
        let pane = Rc::new(Pane {
            card,
            header_row,
            new_tab_button: add.clone(),
            headers,
            header_viewport,
            stack,
            win: RefCell::new(win.clone()),
            tabs: RefCell::default(),
            focus_seq: Cell::new(0),
        });
        self.panes.borrow_mut().push(pane.clone());
        let tab = self.new_tab(&pane, cwd);

        let (app, group) = (Rc::downgrade(self), Rc::downgrade(&pane));
        add.connect_clicked(move |_| {
            if let (Some(app), Some(group)) = (app.upgrade(), group.upgrade()) {
                if let Some(tab) = group.selected_tab() {
                    app.add_tab(&tab);
                }
            }
        });
        let (app, group) = (Rc::downgrade(self), Rc::downgrade(&pane));
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(move |_| {
            if let (Some(app), Some(group)) = (app.upgrade(), group.upgrade()) {
                if let Some(tab) = group.selected_tab() {
                    app.set_active(&tab);
                }
            }
        });
        pane.card.add_controller(focus);
        (pane, tab)
    }

    fn update_new_tab_tooltip(&self, button: &gtk::Button) {
        let hint = self.shortcuts.borrow().hint("new_tab");
        let tooltip = if hint.is_empty() { "New tab".into() } else { format!("New tab ({hint})") };
        button.set_tooltip_text(Some(&tooltip));
    }

    fn new_tab(self: &Rc<Self>, group: &Rc<Pane>, cwd: Option<PathBuf>) -> Rc<TerminalTab> {
        let header = PaneHeader::new();

        let cwd = cwd
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        let mut term_cfg = self.term_config();
        term_cfg.cwd = Some(cwd.clone());
        let term = TermView::new(term_cfg);
        {
            let (theme, _) = themes::resolve(&self.cfg.borrow().terminal, adw::StyleManager::default().is_dark());
            term.set_theme(&theme);
        }
        {
            let cfg = self.cfg.borrow();
            term.set_inactive_style(cfg.panes.inactive.style(), cfg.panes.inactive.fade_ms);
            term.set_padding(cfg.panes.padding);
            let (bar, btn) = scroll_styles(&cfg);
            term.set_scroll_style(bar, btn);
            term.set_session_detect(cfg.session.detect, cfg.session.poll_ms);
            term.set_find_style(cfg.search.style(adw::StyleManager::default().is_dark()));
            term.set_url_style(cfg.terminal.urls.style(adw::StyleManager::default().is_dark()));
            term.set_command_timer(cfg.panes.header.command_timer.settings());
            header.apply_style(&cfg.panes.header, &cfg.font.family);
        }

        let pane = Rc::new(TerminalTab {
            term: term.clone(),
            header,
            pane: Rc::downgrade(group),
            cwd: RefCell::new(cwd),
            host: RefCell::new(glib::host_name().to_string()),
            title: RefCell::default(),
            session: RefCell::default(),
            dir: RefCell::new(None),
            vscode_color: RefCell::new(None),
            last_exit: Cell::new(None),
        });
        self.resolve_dir(&pane);
        self.refresh_header(&pane);
        self.remember_dir(&pane.cwd.borrow());

        let app = Rc::downgrade(self);
        let weak_pane = Rc::downgrade(&pane);
        term.connect_pwd_changed({
            let (app, pane) = (app.clone(), weak_pane.clone());
            move |uri| {
                if let (Some(app), Some(pane)) = (app.upgrade(), pane.upgrade()) {
                    app.on_pwd(&pane, uri);
                }
            }
        });
        term.connect_title_changed({
            let (app, pane) = (app.clone(), weak_pane.clone());
            move |title| {
                if let (Some(app), Some(pane)) = (app.upgrade(), pane.upgrade()) {
                    *pane.title.borrow_mut() = title.to_owned();
                    app.refresh_header(&pane);
                }
            }
        });
        term.connect_bell({
            let (app, pane) = (app.clone(), weak_pane.clone());
            move || {
                if let (Some(app), Some(pane)) = (app.upgrade(), pane.upgrade()) {
                    if app.cfg.borrow().panes.header.bell.enabled {
                        pane.header.flash_bell();
                        if !pane.header.widget().has_css_class("selected") {
                            pane.header.widget().add_css_class("needs-attention");
                        }
                    }
                }
            }
        });
        term.connect_timer_changed({
            let (app, pane) = (app.clone(), weak_pane.clone());
            move |shown| {
                use crate::cmdtimer::Shown;
                let (Some(app), Some(pane)) = (app.upgrade(), pane.upgrade()) else { return };
                let (icon, gap) = {
                    let c = app.cfg.borrow();
                    (c.panes.header.command_timer.icon.clone(), c.panes.header.command_timer.icon_gap)
                };
                match shown {
                    Shown::Hidden => pane.header.set_timer(None),
                    Shown::Running(t) => pane.header.set_timer(Some((&icon, gap, &t, false, false))),
                    Shown::Done { text, failed } => pane.header.set_timer(Some((&icon, gap, &text, true, *failed))),
                }
            }
        });
        term.connect_prompt({
            let (app, pane) = (app.clone(), weak_pane.clone());
            move |exit| {
                if let (Some(app), Some(pane)) = (app.upgrade(), pane.upgrade()) {
                    pane.last_exit.set(exit);
                    if let Some(win) = pane.win().filter(|_| app.is_win_active(&pane)) {
                        app.refresh_status(&win);
                        win.status.on_prompt();
                    }
                }
            }
        });
        term.connect_find_changed({
            let (app, pane) = (app.clone(), weak_pane.clone());
            move |status| {
                if let (Some(app), Some(pane)) = (app.upgrade(), pane.upgrade()) {
                    app.on_find_status(&pane, status);
                }
            }
        });
        term.connect_session_changed({
            let (app, pane) = (app.clone(), weak_pane.clone());
            move |info| {
                if let (Some(app), Some(pane)) = (app.upgrade(), pane.upgrade()) {
                    *pane.session.borrow_mut() = info.clone();
                    app.refresh_session(&pane);
                }
            }
        });
        term.connect_child_exited({
            let (app, pane) = (app.clone(), weak_pane.clone());
            move || {
                if let (Some(app), Some(pane)) = (app.upgrade(), pane.upgrade()) {
                    app.close_tab(&pane);
                }
            }
        });
        term.connect_focus_changed({
            let (app, pane) = (app.clone(), weak_pane.clone());
            move |focused| {
                if let (true, Some(app), Some(pane)) = (focused, app.upgrade(), pane.upgrade()) {
                    app.set_active(&pane);
                }
            }
        });

        // Header buttons.
        let on_click = |button: &gtk::Button, f: fn(&Rc<App>, &Rc<TerminalTab>)| {
            let (app, pane) = (app.clone(), weak_pane.clone());
            button.connect_clicked(move |_| {
                if let (Some(app), Some(pane)) = (app.upgrade(), pane.upgrade()) {
                    f(&app, &pane);
                }
            });
        };
        on_click(&pane.header.copy, App::copy_path);
        on_click(&pane.header.pop_out, App::pop_out);
        on_click(&pane.header.close, App::close_tab);
        self.connect_tab_header(&pane);

        group.tabs.borrow_mut().push(pane.clone());
        group.headers.append(pane.header.widget());
        group.stack.add_child(&term);
        group.update_tab_headers();
        group.select_tab(&pane);
        pane
    }

    fn connect_tab_header(self: &Rc<Self>, tab: &Rc<TerminalTab>) {
        let header = tab.header.widget();
        let (app, target) = (Rc::downgrade(self), Rc::downgrade(tab));
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_PRIMARY).build();
        click.connect_released({
            let (app, target) = (app.clone(), target.clone());
            move |_, _, x, y| {
                if let (Some(app), Some(tab)) = (app.upgrade(), target.upgrade()) {
                    if tab_header_hit(tab.header.widget(), x, y) {
                        app.activate_tab(&tab);
                    }
                }
            }
        });
        header.add_controller(click);

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let (app, target) = (app.clone(), target.clone());
            move |controller, key, _, _| {
                if controller.widget().is_some_and(|w| w.has_focus())
                    && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space)
                {
                    if let (Some(app), Some(tab)) = (app.upgrade(), target.upgrade()) {
                        app.activate_tab(&tab);
                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            }
        });
        header.add_controller(keys);

        let drag = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
        drag.connect_prepare({
            let target = target.clone();
            move |_, x, y| {
                let tab = target.upgrade()?;
                let pane = tab.pane.upgrade()?;
                let header = tab.header.widget();
                (pane.tabs.borrow().len() > 1 && tab_header_hit(header, x, y))
                    .then(|| gdk::ContentProvider::for_value(&header.to_value()))
            }
        });
        header.add_controller(drag);

        let drop = gtk::DropTarget::new(gtk::Widget::static_type(), gdk::DragAction::MOVE);
        drop.connect_drop(move |_, value, x, _| {
            let (Ok(source), Some(tab)) = (value.get::<gtk::Widget>(), target.upgrade()) else { return false };
            let Some(pane) = tab.pane.upgrade() else { return false };
            let after = x >= f64::from(tab.header.widget().width()) / 2.0;
            pane.reorder_tab(&source, &tab, after)
        });
        header.add_controller(drop);
    }

    fn add_tab(self: &Rc<Self>, current: &Rc<TerminalTab>) {
        let Some(pane) = current.pane.upgrade() else { return };
        let cwd = current.cwd.borrow().clone();
        let tab = self.new_tab(&pane, Some(cwd));
        self.set_active(&tab);
        Self::focus_later(&tab.term);
    }

    // -----------------------------------------------------------------------------------------
    // Find in output (the title-bar search box)
    // -----------------------------------------------------------------------------------------

    /// The box was opened: it searches the window's active pane.
    fn begin_find(&self, win: &Rc<Win>) {
        *win.find_pane.borrow_mut() = self.win_active(win).map_or_else(Weak::new, |p| Rc::downgrade(&p));
    }

    fn find_target(win: &Rc<Win>) -> Option<Rc<TerminalTab>> {
        win.find_pane.borrow().upgrade()
    }

    fn run_find(&self, win: &Rc<Win>, opts: crate::terminal::FindOptions) {
        if Self::find_target(win).is_none() {
            self.begin_find(win);
        }
        if let Some(pane) = Self::find_target(win) {
            let status = pane.term.find(opts);
            win.search.set_status(&status);
        }
    }

    fn step_find(&self, win: &Rc<Win>, forward: bool) {
        if let Some(pane) = Self::find_target(win) {
            let status = pane.term.find_step(forward);
            win.search.set_status(&status);
        }
    }

    /// Esc: remove the highlights and give the terminal focus back.
    fn end_find(&self, win: &Rc<Win>) {
        if let Some(pane) = Self::find_target(win) {
            pane.term.find_clear();
            Self::focus_later(&pane.term);
        }
        *win.find_pane.borrow_mut() = Weak::new();
    }

    /// A pane's find status changed (results, stepping, output arriving): show it if this pane is
    /// what its window's box is searching.
    fn on_find_status(&self, pane: &Rc<TerminalTab>, status: &crate::terminal::FindStatus) {
        let Some(win) = pane.win() else { return };
        if Self::find_target(&win).is_some_and(|t| Rc::ptr_eq(&t, pane)) {
            win.search.set_status(status);
        }
    }

    /// The user focused another pane while a find is open: move the find there.
    fn retarget_find(&self, pane: &Rc<TerminalTab>) {
        let Some(win) = pane.win() else { return };
        if !win.search.is_find_open() || Self::find_target(&win).is_some_and(|t| Rc::ptr_eq(&t, pane)) {
            return;
        }
        if let Some(old) = Self::find_target(&win) {
            old.term.find_clear();
        }
        *win.find_pane.borrow_mut() = Rc::downgrade(pane);
        let status = pane.term.find(win.search.options());
        win.search.set_status(&status);
    }

    // -----------------------------------------------------------------------------------------
    // Command palette
    // -----------------------------------------------------------------------------------------

    fn remember_dir(&self, dir: &std::path::Path) {
        let cap = self.cfg.borrow().palette.recent_dirs;
        palette::push_recent(&mut self.recent_dirs.borrow_mut(), dir.to_path_buf(), cap.max(1));
    }

    /// Bring a terminal tab (and its window) to the front and focus it.
    fn goto_tab(self: &Rc<Self>, tab: &Rc<TerminalTab>) {
        let Some(win) = tab.win() else { return };
        win.window.present();
        self.activate_tab(tab);
    }

    fn activate_tab(self: &Rc<Self>, tab: &Rc<TerminalTab>) {
        let Some(pane) = tab.pane.upgrade() else { return };
        pane.select_tab(tab);
        self.set_active(tab);
        Self::focus_later(&tab.term);
    }

    /// A new window with one pane, started in `cwd`.
    fn new_window_at(self: &Rc<Self>, cwd: PathBuf) {
        let win = self.create_window(false);
        let (pane, tab) = self.new_pane(&win, Some(cwd));
        win.root.append(&pane.card);
        pane.focus_seq.set(self.next_focus_seq());
        *self.active.borrow_mut() = Rc::downgrade(&tab);
        self.update_pane_ui();
        self.refresh_frames();
        win.window.present();
        Self::focus_later(&tab.term);
    }

    /// Open the config file in the default editor, creating a commented stub if there is none.
    fn open_config_file(&self) {
        let path = config::user_config_path();
        if !path.exists() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(
                &path,
                "# paua config. Every field is optional; see examples/config.yaml in the source tree.\n",
            );
        }
        let uri = gio::File::for_path(&path).uri();
        if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE) {
            self.toast(&format!("Couldn't open {}: {e}", path.display()));
        }
    }

    /// Everything the palette offers for `win` (built each time it opens).
    fn palette_items(self: &Rc<Self>, win: &Rc<Win>) -> Vec<PaletteItem> {
        let cfg = self.cfg.borrow();
        let active = self.win_active(win);
        let (app, w) = (Rc::downgrade(self), Rc::downgrade(win));
        let mut items: Vec<PaletteItem> = Vec::new();

        let bindings = self.shortcuts.borrow();
        for action in shortcuts::window_actions().filter(|action| action.id != "palette") {
            let (app, w) = (app.clone(), w.clone());
            let hint = bindings.hint(action.id);
            items.push(PaletteItem::new("Command", action.title, hint, move || {
                if let (Some(app), Some(win)) = (app.upgrade(), w.upgrade()) {
                    app.run_action(&win, action.id);
                }
            }));
        }

        // Your own commands: typed into the focused pane.
        if let Some(pane) = &active {
            for c in &cfg.palette.commands {
                let (term, text) = (pane.term.clone(), format!("{}\r", c.run));
                let sub = c.subtitle.clone().unwrap_or_else(|| c.run.clone());
                items.push(PaletteItem::new("Command", c.title.clone(), sub, move || term.type_text(&text)));
            }
        }

        // Other tabs, including those in other panes and windows.
        for p in self.all_tabs() {
            if active.as_ref().is_some_and(|a| Rc::ptr_eq(a, &p)) {
                continue;
            }
            let text = p.header.full_text();
            let title = if text.trim().is_empty() { p.cwd.borrow().display().to_string() } else { text };
            let (app, tab) = (app.clone(), Rc::downgrade(&p));
            items.push(PaletteItem::new("Tab", format!("Go to {title}"), "", move || {
                if let (Some(a), Some(tab)) = (app.upgrade(), tab.upgrade()) {
                    a.goto_tab(&tab);
                }
            }));
        }

        // `cd` to recent directories, and `ssh` to configured hosts, in the focused pane.
        if let Some(pane) = &active {
            if cfg.palette.directories {
                let here = pane.cwd.borrow().clone();
                for dir in self.recent_dirs.borrow().iter().filter(|d| **d != here) {
                    // The plain path: the palette's font may not have the header's icons.
                    let full = dir.display().to_string();
                    let (term, text) = (pane.term.clone(), format!("cd {}\r", palette::shell_quote(&full)));
                    items.push(PaletteItem::new("Directory", format!("cd {full}"), "", move || term.type_text(&text)));
                }
            }
            if cfg.palette.ssh_hosts {
                for host in palette::load_ssh_hosts(&glib::home_dir()) {
                    let (term, text) = (pane.term.clone(), format!("ssh {}\r", palette::shell_quote(&host)));
                    items.push(PaletteItem::new("SSH", format!("ssh {host}"), "", move || term.type_text(&text)));
                }
            }
        }
        items
    }

    // -----------------------------------------------------------------------------------------
    // Status bar
    // -----------------------------------------------------------------------------------------

    /// What a window's status bar needs from the app.
    fn status_host(self: &Rc<Self>, slot: &Rc<RefCell<Weak<Win>>>) -> statusbar::Host {
        let (app, win) = (Rc::downgrade(self), slot.clone());
        let with = move |f: &dyn Fn(&Rc<App>, &Rc<Win>)| {
            if let (Some(a), Some(w)) = (app.upgrade(), win.borrow().upgrade()) {
                f(&a, &w);
            }
        };
        let with = Rc::new(with);
        statusbar::Host {
            action: Box::new({
                let with = with.clone();
                move |action| with(&|a, w| a.run_action(w, action))
            }),
            toast: Box::new({
                let with = with.clone();
                move |msg| with(&|a, w| a.toast_in(w, msg))
            }),
            recent_dirs: Box::new({
                let (app, win) = (Rc::downgrade(self), slot.clone());
                move || {
                    let (Some(a), Some(w)) = (app.upgrade(), win.borrow().upgrade()) else { return Vec::new() };
                    let here = a.win_active(&w).map(|p| p.cwd.borrow().clone());
                    let cfg = a.cfg.borrow();
                    let home = glib::home_dir().display().to_string();
                    a.recent_dirs
                        .borrow()
                        .iter()
                        .filter(|d| Some(*d) != here.as_ref())
                        .map(|d| {
                            let full = d.display().to_string();
                            let shown = header::replace_path(&full, &cfg.panes.header.path_replacements, Some(&home));
                            (shown, full)
                        })
                        .collect()
                }
            }),
            cd: Box::new({
                let with = with.clone();
                move |path| {
                    with(&|a, w| {
                        if let Some(p) = a.win_active(w) {
                            p.term.type_text(&format!("cd {}\r", palette::shell_quote(path)));
                        }
                    })
                }
            }),
        }
    }

    /// Tell a window's status bar about its focused pane.
    fn refresh_status(&self, win: &Rc<Win>) {
        let Some(pane) = self.win_active(win) else { return };
        let info = self.effective_session(&pane);
        let cfg = self.cfg.borrow();
        let (session, _) = session_labels(&cfg, &info);
        let cwd = pane.cwd.borrow().clone();
        let cwd_text = cwd.display().to_string();
        let home = glib::home_dir().display().to_string();
        let ctx = statusbar::Context {
            path: header::replace_path(&cwd_text, &cfg.panes.header.path_replacements, Some(&home)),
            dir: header::dir_name(&cwd_text).to_owned(),
            host: pane.host.borrow().clone(),
            user: glib::user_name().to_string_lossy().into_owned(),
            title: pane.title.borrow().clone(),
            session,
            exit: pane.last_exit.get(),
            cwd,
        };
        drop(cfg);
        win.status.set_context(ctx);
    }

    fn show_about(win: &Rc<Win>) {
        let about = adw::AboutDialog::builder()
            .application_name("Paua")
            .application_icon(crate::APP_ID)
            .version(env!("CARGO_PKG_VERSION"))
            .comments("A colourful terminal")
            .website("https://github.com/gregbacchus/paua-termimal")
            .developer_name("Greg Bacchus")
            .build();
        about.present(Some(&win.window));
    }

    /// A status bar button or menu entry was used.
    fn run_action(self: &Rc<Self>, win: &Rc<Win>, action: &str) {
        let pane = self.win_active(win);
        match action {
            "split_right" => self.split(win, gtk::Orientation::Horizontal),
            "split_down" => self.split(win, gtk::Orientation::Vertical),
            "find" => win.search.open_find(),
            "palette" => win.search.open_command(),
            "new_window" => {
                let cwd = pane.map(|p| p.cwd.borrow().clone()).unwrap_or_default();
                self.new_window_at(cwd);
            }
            "new_tab" => {
                if let Some(tab) = pane { self.add_tab(&tab); }
            }
            "close_tab" => {
                if let Some(tab) = pane { self.close_tab(&tab); }
            }
            "previous_tab" | "next_tab" => {
                let Some(tab) = pane else { return };
                let Some(group) = tab.pane.upgrade() else { return };
                let next = {
                    let tabs = group.tabs.borrow();
                    let Some(current) = tabs.iter().position(|candidate| Rc::ptr_eq(candidate, &tab)) else { return };
                    let direction = if action == "next_tab" { 1 } else { tabs.len() - 1 };
                    tabs[(current + direction) % tabs.len()].clone()
                };
                if !Rc::ptr_eq(&tab, &next) {
                    self.activate_tab(&next);
                }
            }
            "cycle_scheme" => self.cycle_scheme(),
            "about" => Self::show_about(win),
            "keyboard_shortcuts" => self.shortcuts.borrow().show_dialog(&win.window),
            "git_sync" | "git_fetch" | "git_pull" | "git_push" => win.status.git_action(action),
            "reload_config" => self.reload_user(),
            "open_config" => self.open_config_file(),
            "copy" | "paste" | "paste_primary" if Self::edit_clipboard(win, action) => {}
            "clear" | "scroll_to_bottom" | "close_pane" | "pop_out" | "copy_path" | "copy" | "paste" | "paste_primary" if pane.is_none() => {}
            "copy" => pane.unwrap().term.copy(),
            "paste_primary" => pane.unwrap().term.paste_primary(),
            "paste" => pane.unwrap().term.paste_clipboard(),
            "clear" => pane.unwrap().term.clear(),
            "scroll_to_bottom" => pane.unwrap().term.scroll_to_bottom(),
            "close_pane" => self.request_close_pane(&pane.unwrap()),
            "pop_out" => self.pop_out(&pane.unwrap()),
            "copy_path" => self.copy_path(&pane.unwrap()),
            a if a.starts_with("type:") => {
                if let Some(p) = pane {
                    p.term.type_text(&format!("{}\r", &a["type:".len()..]));
                }
            }
            a if a.starts_with("exec:") => {
                let cmd = a["exec:".len()..].to_owned();
                let cwd = pane.map(|p| p.cwd.borrow().clone()).unwrap_or_default();
                let (app, w) = (Rc::downgrade(self), Rc::downgrade(win));
                exec::spawn(
                    move || match exec::run_shell(&cmd, &cwd, std::time::Duration::from_secs(60)) {
                        Ok(o) => {
                            let line = exec::first_line(if o.success || o.stderr.is_empty() { &o.stdout } else { &o.stderr });
                            if o.success { format!("Done {line}") } else { format!("Failed: {line}") }
                        }
                        Err(e) => format!("Failed: {e}"),
                    },
                    move |res| {
                        if let (Some(a), Some(w), Some(msg)) = (app.upgrade(), w.upgrade(), res) {
                            a.toast_in(&w, msg.trim());
                        }
                    },
                );
            }
            other => self.toast_in(win, &format!("Unknown status bar action: {other}")),
        }
    }

    /// Keep GTK's text-field clipboard keys working when Find or another entry has focus.
    fn edit_clipboard(win: &Win, action: &str) -> bool {
        let name = if action == "copy" { "clipboard.copy" } else { "clipboard.paste" };
        GtkWindowExt::focus(&win.window).filter(|widget| widget.is::<gtk::Editable>())
            .is_some_and(|widget| widget.activate_action(name, None).is_ok())
    }

    /// Copy the pane's real working directory (not the shortened display form).
    fn copy_path(self: &Rc<Self>, pane: &Rc<TerminalTab>) {
        let path = pane.cwd.borrow().display().to_string();
        pane.term.clipboard().set_text(&path);
        if let Some(win) = pane.win() {
            self.toast_in(&win, "Path copied");
        }
    }

    fn all_tabs(&self) -> Vec<Rc<TerminalTab>> {
        self.panes.borrow().iter().flat_map(|pane| pane.tabs.borrow().clone()).collect()
    }

    fn panes_in(&self, win: &Rc<Win>) -> Vec<Rc<Pane>> {
        self.panes
            .borrow()
            .iter()
            .filter(|p| Rc::ptr_eq(&p.win.borrow(), win))
            .cloned()
            .collect()
    }

    /// The selected terminal of the most recently focused pane in `win`.
    fn win_active(&self, win: &Rc<Win>) -> Option<Rc<TerminalTab>> {
        self.panes_in(win).into_iter().max_by_key(|p| p.focus_seq.get())?.selected_tab()
    }

    fn is_win_active(&self, pane: &Rc<TerminalTab>) -> bool {
        pane.win().and_then(|win| self.win_active(&win)).is_some_and(|a| Rc::ptr_eq(&a, pane))
    }

    fn new_paned(orientation: gtk::Orientation) -> gtk::Paned {
        gtk::Paned::builder()
            .orientation(orientation)
            .wide_handle(true)
            .hexpand(true)
            .vexpand(true)
            .resize_start_child(true)
            .resize_end_child(true)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .build()
    }

    /// Replace `old` with `new` inside `parent` (a `Paned` or a window's root box).
    fn replace_child(parent: &gtk::Widget, old: &gtk::Widget, new: &gtk::Widget) {
        if let Some(p) = parent.downcast_ref::<gtk::Paned>() {
            if p.start_child().as_ref() == Some(old) {
                p.set_start_child(Some(new));
            } else {
                p.set_end_child(Some(new));
            }
        } else if let Some(b) = parent.downcast_ref::<gtk::Box>() {
            b.remove(old);
            b.append(new);
        }
    }

    /// Split `win`'s active pane, putting a new terminal (same directory) to its right or below.
    fn split(self: &Rc<Self>, win: &Rc<Win>, orientation: gtk::Orientation) {
        let Some(target) = self.win_active(win) else { return };
        let Some(pane) = target.pane.upgrade() else { return };
        let card = pane.card.clone();
        let Some(parent) = card.parent() else { return };

        let (w, h) = (card.width(), card.height());
        let (new_pane, new_tab) = self.new_pane(win, Some(target.cwd.borrow().clone()));
        let paned = Self::new_paned(orientation);
        paned.set_position(if orientation == gtk::Orientation::Horizontal { w / 2 } else { h / 2 });

        // Put the new Paned where the old card was, with the card and the new pane inside it.
        // The card has to be detached from its old parent before it can join the new one.
        let placeholder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        Self::replace_child(&parent, card.upcast_ref(), placeholder.upcast_ref());
        paned.set_start_child(Some(&card));
        paned.set_end_child(Some(&new_pane.card));
        Self::replace_child(&parent, placeholder.upcast_ref(), paned.upcast_ref());

        self.set_active(&new_tab);
        Self::focus_later(&new_tab.term);
    }

    /// Take a pane's card out of its window's split tree; its sibling takes the freed space.
    /// Returns false if the card is the window's only content (there is nothing to collapse into).
    fn detach_card(&self, pane: &Rc<Pane>) -> bool {
        let card = pane.card.clone();
        let Some(parent) = card.parent() else { return false };
        let Some(paned) = parent.downcast_ref::<gtk::Paned>() else { return false };
        let sibling = if paned.start_child().as_ref() == Some(card.upcast_ref()) {
            paned.end_child()
        } else {
            paned.start_child()
        };
        let (Some(sibling), Some(grandparent)) = (sibling, paned.parent()) else { return false };

        // The pane usually holds focus; drop it first so GTK doesn't warn about a focus child
        // disappearing from the Paned.
        let win = pane.win.borrow().clone();
        if GtkWindowExt::focus(&win.window).is_some_and(|focus| focus.is_ancestor(paned)) {
            GtkWindowExt::set_focus(&win.window, None::<&gtk::Widget>);
        }
        paned.set_start_child(None::<&gtk::Widget>);
        paned.set_end_child(None::<&gtk::Widget>);
        Self::replace_child(&grandparent, paned.upcast_ref(), &sibling);
        true
    }

    fn focus_later(term: &TermView) {
        let term = term.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(term) = term.upgrade().filter(|term| term.is_mapped()) {
                term.grab_focus();
            }
        });
    }

    fn close_tab(self: &Rc<Self>, tab: &Rc<TerminalTab>) {
        let Some(pane) = tab.pane.upgrade() else { return };
        let Some(index) = pane.tabs.borrow().iter().position(|candidate| Rc::ptr_eq(candidate, tab)) else { return };
        let was_active = self.is_win_active(tab);
        let was_global = self.is_active_anywhere(tab);
        let selected = pane.selected_tab().is_some_and(|current| Rc::ptr_eq(&current, tab));
        if pane.tabs.borrow().len() == 1 {
            self.close_pane(&pane);
            return;
        }
        let next = {
            let mut tabs = pane.tabs.borrow_mut();
            tabs.remove(index);
            selected.then(|| tabs[index.min(tabs.len() - 1)].clone())
        };
        pane.headers.remove(tab.header.widget());
        pane.update_tab_headers();
        if let Some(next) = &next {
            pane.select_tab(next);
        }
        pane.stack.remove(&tab.term);
        tab.term.shutdown();
        if let Some(next) = next.filter(|_| was_active) {
            if was_global {
                self.set_active(&next);
                Self::focus_later(&next.term);
            } else if let Some(win) = next.win() {
                self.retarget_find(&next);
                self.refresh_window_title(&win);
                self.refresh_status(&win);
                self.refresh_frames();
            }
        }
    }

    fn request_close_pane(self: &Rc<Self>, tab: &Rc<TerminalTab>) {
        let Some(pane) = tab.pane.upgrade() else { return };
        let count = pane.tabs.borrow().len();
        if count <= 1 {
            self.close_pane(&pane);
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading("Close pane?")
            .body(format!("This ends all {count} terminal sessions in the pane."))
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("close", "Close Pane");
        dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let (app, weak) = (Rc::downgrade(self), Rc::downgrade(&pane));
        dialog.connect_response(Some("close"), move |_, _| {
            if let (Some(app), Some(pane)) = (app.upgrade(), weak.upgrade()) {
                app.close_pane(&pane);
            }
        });
        dialog.present(Some(&pane.win.borrow().window));
    }

    /// Close the whole group. A sibling takes over the freed space, or the last pane closes its window.
    fn close_pane(self: &Rc<Self>, pane: &Rc<Pane>) {
        if pane.tabs.borrow().is_empty() {
            return;
        }
        let win = pane.win.borrow().clone();
        let was_global = pane.selected_tab().is_some_and(|tab| self.is_active_anywhere(&tab));
        let detached = self.detach_card(pane);
        self.panes.borrow_mut().retain(|p| !Rc::ptr_eq(p, pane));
        for tab in pane.tabs.borrow_mut().drain(..) {
            tab.term.shutdown();
        }
        if detached {
            if let Some(next) = self.win_active(&win) {
                if was_global {
                    self.set_active(&next);
                } else {
                    self.retarget_find(&next);
                }
                if GtkWindowExt::focus(&win.window).is_none() {
                    Self::focus_later(&next.term);
                }
            }
            self.update_pane_ui();
            self.refresh_frames();
            self.watch_dir();
            return;
        }
        win.window.close();
    }

    /// Move a pane and its running tabs into a new full window. Offered only while its current
    /// window would keep at least one other pane.
    fn pop_out(self: &Rc<Self>, tab: &Rc<TerminalTab>) {
        let Some(pane) = tab.pane.upgrade() else { return };
        let from = pane.win.borrow().clone();
        if self.panes_in(&from).len() < 2 || !self.detach_card(&pane) {
            return;
        }

        let win = self.create_window(false);
        if Self::find_target(&from).is_some_and(|t| t.pane.upgrade().is_some_and(|p| Rc::ptr_eq(&p, &pane))) {
            if let Some(old) = from.find_pane.replace(Weak::new()).upgrade() {
                old.term.find_clear();
            }
        }
        *pane.win.borrow_mut() = win.clone();
        win.root.append(&pane.card);
        pane.select_tab(tab);

        pane.focus_seq.set(self.next_focus_seq());
        *self.active.borrow_mut() = Rc::downgrade(tab);
        if let Some(next) = self.win_active(&from) {
            self.retarget_find(&next);
            Self::focus_later(&next.term);
        }

        self.refresh_header(tab);
        self.update_pane_ui();
        self.refresh_frames();
        self.watch_dir();
        win.window.present();
        pane.reveal_tab(tab);
        Self::focus_later(&tab.term);
    }

    /// Per window: dim every pane except that window's active one (only when it has several),
    /// enable the pop-out button only where it makes sense, and refresh the window title.
    fn update_pane_ui(&self) {
        for win in self.all_wins() {
            self.refresh_status(&win);
            let panes = self.panes_in(&win);
            let active = self.win_active(&win).and_then(|tab| tab.pane.upgrade());
            let multiple = panes.len() > 1;
            for p in &panes {
                let inactive = multiple && !active.as_ref().is_some_and(|a| Rc::ptr_eq(a, p));
                for tab in p.tabs.borrow().iter() {
                    tab.term.set_dimmed(inactive);
                    tab.header.pop_out.set_sensitive(multiple);
                }
                if inactive {
                    p.card.add_css_class("inactive");
                } else {
                    p.card.remove_css_class("inactive");
                }
            }
            self.refresh_window_title(&win);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Pane state: header text, working directory, focus
    // -----------------------------------------------------------------------------------------

    /// Rebuild the header from the template. (Path trimming to fit the pane width happens in the
    /// header widget itself, since it depends on the pane's size.)
    fn refresh_header(&self, pane: &Rc<TerminalTab>) {
        let info = self.effective_session(pane);
        {
            let cfg = self.cfg.borrow();
            let hc = &cfg.panes.header;
            let (session_text, badges) = session_labels(&cfg, &info);
            let cwd = pane.cwd.borrow().display().to_string();
            let title = pane.title.borrow();
            pane.header.widget().set_tooltip_text(Some(&cwd));
            pane.header.widget().update_property(&[gtk::accessible::Property::Label(&cwd)]);
            let home = glib::home_dir().display().to_string();
            pane.header.set_input(header::HeaderInput {
                template: hc.template.clone(),
                path: header::replace_path(&cwd, &hc.path_replacements, Some(&home)),
                dir: header::dir_name(&cwd).to_owned(),
                title: title.clone(),
                host: pane.host.borrow().clone(),
                user: glib::user_name().to_string_lossy().into_owned(),
                session: session_text,
                trim: hc.trim_path,
                ellipsis: hc.path_ellipsis.clone(),
            });
            pane.header.set_badges(&badges);
            pane.header.set_session_class(info.items.first().map(|i| i.kind.header_class()));
        }
        if let Some(win) = pane.win().filter(|_| self.is_win_active(pane)) {
            self.refresh_window_title(&win);
            self.refresh_status(&win);
        }
    }

    /// The process-tree detection plus "the shell reports another host" (OSC 7), restricted to
    /// the kinds the config enables.
    fn effective_session(&self, pane: &Rc<TerminalTab>) -> SessionInfo {
        let cfg = self.cfg.borrow();
        if !cfg.session.detect {
            return SessionInfo::default();
        }
        let mut info = pane.session.borrow().clone();
        let host = pane.host.borrow().clone();
        if !session::same_host(&host, &glib::host_name())
            && !info.has(Kind::Ssh)
            && !info.has(Kind::Remote)
        {
            info.items.push(session::Item {
                kind: Kind::Remote,
                tool: "remote".into(),
                target: host.clone(),
                host,
                user: String::new(),
            });
            info.items.sort_by_key(|i| i.kind);
        }
        info.items.retain(|i| cfg.session.kind(i.kind).enabled);
        info
    }

    /// The session changed: update the header (text + badges) and, if this window shows it, the
    /// frame colour.
    fn refresh_session(self: &Rc<Self>, pane: &Rc<TerminalTab>) {
        self.refresh_header(pane);
        if self.is_win_active(pane) {
            self.refresh_frames();
        }
    }

    fn is_active_anywhere(&self, pane: &Rc<TerminalTab>) -> bool {
        self.active.borrow().upgrade().is_some_and(|a| Rc::ptr_eq(&a, pane))
    }

    /// The shell reported a new working directory.
    fn on_pwd(self: &Rc<Self>, pane: &Rc<TerminalTab>, uri: &str) {
        let (host, path) = header::parse_uri(uri);
        let host_changed = *pane.host.borrow() != host;
        *pane.host.borrow_mut() = host;
        let Some(path) = path.map(PathBuf::from) else { return };
        let changed = *pane.cwd.borrow() != path;
        if changed {
            self.remember_dir(&path);
            *pane.cwd.borrow_mut() = path;
        }
        self.refresh_header(pane);
        if host_changed && self.is_win_active(pane) {
            self.refresh_frames(); // a different host can mean a different session colour
        }
        if !changed {
            return; // only re-resolve `.paua.yaml` when the directory actually changes
        }
        self.resolve_dir(pane);
        if self.is_win_active(pane) {
            self.refresh_frames();
        }
        if self.is_active_anywhere(pane) {
            self.watch_dir();
        }
    }

    /// Walk up from the pane's directory to find its `.paua.yaml` and load it.
    fn resolve_dir(&self, pane: &Rc<TerminalTab>) {
        let entry = config::find_dir_config(&pane.cwd.borrow()).map(|path| {
            let cfg = config::load_dir(&path).unwrap_or_else(|e| {
                self.toast(&format!("Config error: {e}"));
                DirConfig::default()
            });
            DirEntry { path, cfg }
        });
        *pane.dir.borrow_mut() = entry;
        self.load_vscode_color(pane);
    }

    /// Read the project colour from the nearest `.vscode/settings.json`. Problems with a file we
    /// don't own are only logged, never shown.
    fn load_vscode_color(&self, pane: &Rc<TerminalTab>) {
        let cfg = self.cfg.borrow();
        let color = if cfg.window.vscode_colors {
            config::find_vscode_settings(&pane.cwd.borrow()).and_then(|path| {
                config::load_vscode_color(&path, &cfg.window.vscode_color_keys)
                    .map_err(|e| eprintln!("vscode colour: {e}"))
                    .ok()
                    .flatten()
            })
        } else {
            None
        };
        *pane.vscode_color.borrow_mut() = color;
    }

    /// Re-read the contents of the pane's already-found `.paua.yaml` (no directory walk).
    fn reload_dir_entry(&self, pane: &Rc<TerminalTab>) {
        let mut dir = pane.dir.borrow_mut();
        if let Some(entry) = dir.as_mut() {
            match config::load_dir(&entry.path) {
                Ok(cfg) => entry.cfg = cfg,
                Err(e) => self.toast(&format!("Config error: {e}")),
            }
        }
    }

    fn set_active(self: &Rc<Self>, pane: &Rc<TerminalTab>) {
        let Some(group) = pane.pane.upgrade() else { return };
        if !group.selected_tab().is_some_and(|tab| Rc::ptr_eq(&tab, pane)) {
            return;
        }
        if self.is_active_anywhere(pane) && self.is_win_active(pane) {
            return;
        }
        group.focus_seq.set(self.next_focus_seq());
        *self.active.borrow_mut() = Rc::downgrade(pane);
        if self.cfg.borrow().watch {
            self.reload_dir_entry(pane);
        }
        self.load_vscode_color(pane); // cheap; picks up edits made while another pane had focus
        self.retarget_find(pane);
        self.update_pane_ui();
        self.refresh_frames();
        self.watch_dir();
    }

    // -----------------------------------------------------------------------------------------
    // Colours
    // -----------------------------------------------------------------------------------------

    /// Each window's frame colour = its active pane's directory override, else the user's default
    /// window colour.
    fn refresh_frames(&self) {
        let dark = adw::StyleManager::default().is_dark();
        let cfg = self.cfg.borrow();
        let base = cfg.window_color(dark);
        let underline = TabUnderline {
            height: cfg.panes.tabs.active_underline_height.min(12),
            color: cfg.panes.tabs.active_color.and_then(|tint| tint.resolve(dark)),
        };
        for win in self.all_wins() {
            let mut color = base;
            let mut session_color = None;
            if let Some(pane) = self.win_active(&win) {
                let info = self.effective_session(&pane);
                session_color = info
                    .items
                    .iter()
                    .find_map(|i| cfg.session.kind(i.kind).window_color.and_then(|c| c.resolve(dark)));
                let over = config::directory_color(
                    pane.dir.borrow().as_ref().map(|d| &d.cfg),
                    *pane.vscode_color.borrow(),
                    dark,
                );
                if let Some(c) = over {
                    color = c;
                }
            }
            // An active ssh/sudo/... session's colour wins: it is a warning, not a preference.
            win.frame.set(&win.window, session_color.unwrap_or(color), underline);
        }
    }

    /// Pane header background tint and text colour from the config, per light/dark mode.
    fn apply_header_css(&self) {
        let cfg = self.cfg.borrow();
        let hc = &cfg.panes.header;
        let tabs = &cfg.panes.tabs;
        let mut css = format!(
            "window.themed .pane-header-row.multiple-tabs .pane-header {{ min-width: {}px; }}\n",
            tabs.min_width.min(1000),
        );
        for (mode, dark) in [("dark", true), ("light", false)] {
            if let Some(color) = tabs.attention_color.and_then(|tint| tint.resolve(dark)) {
                css += &format!(
                    "window.themed.{mode} .pane-header-row.multiple-tabs .pane-header.needs-attention {{ \
                     box-shadow: inset -1px 0 rgba(128, 128, 128, 0.25), inset 0 -2px {}; }}\n",
                    color.css(),
                );
            }
            if let Some(bg) = hc.background.and_then(|b| b.resolve(dark)) {
                css += &format!("window.{mode} .pane-header {{ background-color: {}; }}\n", bg.css());
            }
            if let Some(fg) = hc.color.and_then(|c| c.resolve(dark)) {
                css += &format!(
                    "window.{mode} .pane-header, window.{mode} .pane-header .pane-title,\n\
                     window.{mode} .pane-header .pane-button {{ color: {}; }}\n\
                     window.{mode} .pane-header .pane-title {{ opacity: 1; }}\n",
                    fg.css()
                );
            }
        }
        // The bell flash: colour, and how long the fade takes.
        let ms = hc.bell.duration.0.as_millis();
        for (mode, dark) in [("dark", true), ("light", false)] {
            if let Some(c) = hc.bell.color.and_then(|c| c.resolve(dark)) {
                css += &format!("window.{mode} .pane-header .pane-bell {{ background-color: {}; }}\n", c.css());
            }
            css += &format!(
                "window.{mode} .pane-header .pane-bell {{ transition: opacity {ms}ms ease-out; }}\n\
                 window.{mode} .pane-header .pane-bell.on {{ opacity: 1; transition: none; }}\n"
            );
        }
        // The running-command timer chip.
        let ct = &hc.command_timer;
        for (mode, dark) in [("dark", true), ("light", false)] {
            for (class, bg, fg) in [
                ("", ct.background, ct.color),
                (".done", ct.done_background, ct.done_color),
                (".failed", ct.failed_background, ct.failed_color),
            ] {
                let mut decl = String::new();
                if let Some(b) = bg.and_then(|b| b.resolve(dark)) {
                    decl += &format!("background-color: {}; ", b.css());
                }
                if let Some(c) = fg.and_then(|c| c.resolve(dark)) {
                    decl += &format!("color: {}; ", c.css());
                }
                if !decl.is_empty() {
                    css += &format!("window.{mode} .pane-header .run-timer{class} {{ {decl}}}\n");
                }
            }
        }
        for kind in Kind::ALL {
            let kc = cfg.session.kind(kind);
            for (mode, dark) in [("dark", true), ("light", false)] {
                // The whole header line while this is the pane's top session.
                let sel = format!("window.{mode} .pane-header.{}", kind.header_class());
                if let Some(bg) = kc.header_background.and_then(|b| b.resolve(dark)) {
                    css += &format!("{sel} {{ background-color: {}; }}\n", bg.css());
                }
                if let Some(fg) = kc.header_color.and_then(|c| c.resolve(dark)) {
                    css += &format!(
                        "{sel}, {sel} .pane-title, {sel} .pane-button {{ color: {}; }}\n{sel} .pane-title {{ opacity: 1; }}\n",
                        fg.css()
                    );
                }
                let mut decl = String::new();
                if let Some(bg) = kc.background.and_then(|b| b.resolve(dark)) {
                    decl += &format!("background-color: {}; ", bg.css());
                }
                if let Some(fg) = kc.color.and_then(|c| c.resolve(dark)) {
                    decl += &format!("color: {}; ", fg.css());
                }
                if !decl.is_empty() {
                    css += &format!("window.{mode} .session-badge.{} {{ {decl}}}\n", kind.name());
                }
            }
        }
        // The pane cards (and the padding around the text) take the terminal theme's colours.
        for (mode, dark) in [("dark", true), ("light", false)] {
            let (t, _) = themes::resolve(&cfg.terminal, dark);
            css += &format!(
                "window.{mode} .pane {{ background-color: rgb({}, {}, {}); color: rgb({}, {}, {}); }}\n",
                t.bg.0, t.bg.1, t.bg.2, t.fg.0, t.fg.1, t.fg.2
            );
        }
        css += &statusbar::css(&cfg.status_bar);
        self.header_css.load_from_string(&css);
    }

    // -----------------------------------------------------------------------------------------
    // Live reload
    // -----------------------------------------------------------------------------------------

    /// Show a message in the window of the most recently focused pane (or the first window).
    fn toast(&self, msg: &str) {
        eprintln!("{msg}");
        let win = self
            .active
            .borrow()
            .upgrade()
            .and_then(|p| p.win())
            .or_else(|| self.wins.borrow().first().cloned());
        if let Some(win) = win {
            self.toast_in(&win, msg);
        }
    }

    fn toast_in(&self, win: &Rc<Win>, msg: &str) {
        let toast = adw::Toast::new(msg);
        toast.set_timeout(if msg.starts_with("Config error") { 6 } else { 2 });
        win.toasts.add_toast(toast);
    }

    /// Watch a config file (and, if it is a symbolic link, everything it points through) and call
    /// `on_change` for every event. The caller debounces.
    fn watch_file(path: &std::path::Path, on_change: impl Fn() + Clone + 'static) -> Vec<gio::FileMonitor> {
        config::watch_paths(path)
            .iter()
            .filter_map(|p| {
                let monitor = gio::File::for_path(p)
                    .monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
                    .ok()?;
                let on_change = on_change.clone();
                monitor.connect_changed(move |_, _, _, _| on_change());
                Some(monitor)
            })
            .collect()
    }

    fn setup_user_monitor(self: &Rc<Self>) {
        self.user_monitor.borrow_mut().clear();
        if !self.cfg.borrow().watch {
            return;
        }
        let weak = Rc::downgrade(self);
        let monitors = Self::watch_file(&config::user_config_path(), move || {
            let Some(app) = weak.upgrade() else { return };
            // Editors often write several events per save: debounce.
            let generation = app.user_gen.get().wrapping_add(1);
            app.user_gen.set(generation);
            let weak = Rc::downgrade(&app);
            glib::timeout_add_local_once(Duration::from_millis(250), move || {
                if let Some(app) = weak.upgrade() {
                    if app.user_gen.get() == generation {
                        app.reload_user(); // also re-creates the monitors (a link may now point elsewhere)
                    }
                }
            });
        });
        *self.user_monitor.borrow_mut() = monitors;
    }

    fn reload_user(self: &Rc<Self>) {
        let loaded = config::load_user().and_then(|cfg| {
            let bindings = Bindings::resolve(&cfg.shortcuts)?;
            Ok((cfg, bindings))
        });
        match loaded {
            Ok((cfg, bindings)) => {
                bindings.install(&self.gtk_app, Some(&self.shortcuts.borrow()));
                *self.shortcuts.borrow_mut() = Rc::new(bindings);
                *self.cfg.borrow_mut() = cfg;
                self.apply_config();
                self.toast("Config reloaded");
            }
            Err(e) => self.toast(&format!("Config error (keeping previous settings): {e}")),
        }
    }

    /// Apply the settings that can change at runtime. Window size/maximized are startup-only.
    fn apply_config(self: &Rc<Self>) {
        let panes = self.all_tabs();
        {
            let cfg = self.cfg.borrow();
            let font = cfg.font.pango_string();
            for pane in self.panes.borrow().iter() {
                pane.new_tab_button.set_icon_name(&cfg.panes.tabs.new_tab_icon);
                self.update_new_tab_tooltip(&pane.new_tab_button);
            }
            for p in &panes {
                p.term.set_font(&font, font_weights(&cfg), cfg.font.features.clone());
                p.term.set_inactive_style(cfg.panes.inactive.style(), cfg.panes.inactive.fade_ms);
                p.term.set_padding(cfg.panes.padding);
                let (bar, btn) = scroll_styles(&cfg);
                p.term.set_scroll_style(bar, btn);
                p.term.set_session_detect(cfg.session.detect, cfg.session.poll_ms);
                p.term.set_theme(&themes::resolve(&cfg.terminal, adw::StyleManager::default().is_dark()).0);
                p.term.set_find_style(cfg.search.style(adw::StyleManager::default().is_dark()));
                p.term.set_url_style(cfg.terminal.urls.style(adw::StyleManager::default().is_dark()));
                p.term.set_command_timer(cfg.panes.header.command_timer.settings());
                p.header.apply_style(&cfg.panes.header, &cfg.font.family);
            }
        }
        self.apply_header_css();
        if let (_, Some(warning)) = themes::resolve(&self.cfg.borrow().terminal, adw::StyleManager::default().is_dark()) {
            self.toast(&format!("Config error: {warning}"));
        }
        let shortcuts = self.shortcuts.borrow().clone();
        for win in self.all_wins() {
            win.status.configure(&self.cfg.borrow().status_bar);
            self.refresh_status(&win);
            win.frame.set_backdrop(self.cfg.borrow().window.inactive_dim);
            let cfg = self.cfg.borrow();
            win.search.set_defaults(cfg.search.case, cfg.search.regex);
            win.search.set_max_results(cfg.palette.max_results);
            win.search.set_shortcuts(shortcuts.clone());
            shortcuts.refresh_dialog(&win.window);
        }
        for p in &panes {
            self.refresh_header(p);
        }
        self.refresh_frames();
        self.setup_user_monitor(); // re-evaluates the `watch` flag
        self.watch_dir();
    }

    /// Watch the most recently focused pane's `.paua.yaml` (if any, and if watching is enabled).
    fn watch_dir(self: &Rc<Self>) {
        self.dir_monitor.borrow_mut().clear();
        if !self.cfg.borrow().watch {
            return;
        }
        let Some(path) = self
            .active
            .borrow()
            .upgrade()
            .and_then(|p| p.dir.borrow().as_ref().map(|d| d.path.clone()))
        else {
            return;
        };
        let weak = Rc::downgrade(self);
        let watched = path.clone();
        let monitors = Self::watch_file(&path, move || {
            let Some(app) = weak.upgrade() else { return };
            let generation = app.dir_gen.get().wrapping_add(1);
            app.dir_gen.set(generation);
            let weak = Rc::downgrade(&app);
            let path = watched.clone();
            glib::timeout_add_local_once(Duration::from_millis(250), move || {
                let Some(app) = weak.upgrade() else { return };
                if app.dir_gen.get() != generation {
                    return;
                }
                // Reload this file for every pane that uses it, then recolour.
                for pane in app.all_tabs() {
                    let mut dir = pane.dir.borrow_mut();
                    if let Some(entry) = dir.as_mut().filter(|e| e.path == path) {
                        match config::load_dir(&path) {
                            Ok(cfg) => entry.cfg = cfg,
                            Err(e) => app.toast(&format!("Config error: {e}")),
                        }
                    }
                }
                app.refresh_frames();
                app.watch_dir(); // a link may now point elsewhere
            });
        });
        *self.dir_monitor.borrow_mut() = monitors;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn wait_for(message: &str, ready: impl Fn() -> bool) {
        let context = glib::MainContext::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            context.iteration(false);
            if ready() {
                return;
            }
            assert!(Instant::now() < deadline, "{message}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    #[ignore = "needs a GTK display"]
    fn shortcut_actions_use_overrides_and_open_help() {
        adw::init().unwrap();
        let gtk_app = adw::Application::builder()
            .application_id("io.github.gregbacchus.Paua.ShortcutTest")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        gtk_app.register(None::<&gio::Cancellable>).unwrap();
        crate::install_actions(&gtk_app);
        let mut cfg: UserConfig = serde_yaml_ng::from_str(
            "shortcuts: { split_right: Ctrl+Shift+T, close_tab: [] }",
        ).unwrap();
        cfg.watch = false;
        cfg.terminal.shell = Some("/bin/sh".into());
        cfg.session.detect = false;
        let app = App::build_with_config(&gtk_app, cfg, None);
        let win = app.all_wins().remove(0);
        assert_eq!(gtk_app.accels_for_action("win.keyboard-shortcuts"), ["F1"]);
        assert!(gtk_app.accels_for_action("win.new-tab").is_empty());
        assert!(gtk_app.accels_for_action("win.close-tab").is_empty());
        assert_eq!(gtk_app.accels_for_action("win.split-right").len(), 1);
        let terminal = &app.win_active(&win).unwrap().term;
        let controllers = terminal.observe_controllers();
        assert!((0..controllers.n_items()).any(|i| {
            controllers.item(i).and_downcast::<gtk::EventControllerKey>()
                .and_then(|keys| keys.im_context())
                .is_some_and(|im| im.input_purpose() == gtk::InputPurpose::Terminal)
        }));
        let terminal_paste = gtk::accelerator_parse("<Control><Shift>Insert").unwrap();
        assert!(gtk_app.accels_for_action("win.paste").iter()
            .any(|key| gtk::accelerator_parse(key.as_str()) == Some(terminal_paste)));
        terminal.clipboard().set_text("clipboard-paste-test");
        terminal.display().primary_clipboard().set_text("primary-paste-test");
        for (action, expected) in [("win.paste", "clipboard-paste-test"), ("win.paste-primary", "primary-paste-test")] {
            assert!(gtk::prelude::WidgetExt::activate_action(&win.window, action, None).is_ok());
            wait_for(expected, || terminal.find(crate::terminal::FindOptions {
                query: expected.into(), ..Default::default()
            }).total == 1);
            terminal.find_clear();
        }
        assert!(gtk::prelude::WidgetExt::activate_action(&win.window, "win.keyboard-shortcuts", None).is_ok());
        wait_for("keyboard shortcuts dialog did not open", || win.window.visible_dialog().is_some());
        let dialog = win.window.visible_dialog().unwrap().downcast::<adw::PreferencesDialog>().unwrap();
        let old_page = dialog.visible_page().unwrap();
        app.shortcuts.borrow().refresh_dialog(&win.window);
        assert_ne!(dialog.visible_page(), Some(old_page));
        assert!(dialog.visible_page().is_some());
        dialog.close();
        let conflict = serde_yaml_ng::from_str("{ new_tab: F1, close_tab: F1 }").unwrap();
        assert!(Bindings::resolve(&conflict).is_err());
        win.window.close();
        APP.with(|slot| slot.borrow_mut().take());
    }

    #[test]
    #[ignore = "needs a display and launches test shells"]
    fn pane_tabs_keep_shells_alive_and_follow_their_pane() {
        adw::init().unwrap();
        let gtk_app = adw::Application::builder()
            .application_id("io.github.gregbacchus.Paua.TabTest")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        gtk_app.register(None::<&gio::Cancellable>).unwrap();
        crate::install_actions(&gtk_app);
        crate::load_css();
        let mut cfg = UserConfig::default();
        cfg.watch = false;
        cfg.window.width = config::Dim::Px(640.0);
        cfg.terminal.shell = Some("/bin/sh".into());
        cfg.session.detect = false;
        let app = App::build_with_config(&gtk_app, cfg, None);
        let win = app.all_wins().remove(0);
        let left = app.panes_in(&win).remove(0);
        let first = left.selected_tab().unwrap();
        assert!(!left.header_row.has_css_class("multiple-tabs"));

        first.term.type_text("sleep 0.1; printf 'hidden-%s\\n\\a' output\r");
        let add = &left.new_tab_button;
        add.emit_clicked();
        let second = left.selected_tab().unwrap();
        assert!(!Rc::ptr_eq(&first, &second));
        assert_eq!(*first.cwd.borrow(), *second.cwd.borrow());
        assert!(left.header_row.has_css_class("multiple-tabs"));
        wait_for("both tab headers should be visible in one row", || {
            let Some(a) = first.header.widget().compute_bounds(&left.header_row) else { return false };
            let Some(b) = second.header.widget().compute_bounds(&left.header_row) else { return false };
            let Some(plus) = add.compute_bounds(&left.header_row) else { return false };
            a.width() > 0.0 && b.width() > 0.0 && a.y() == b.y() && a.x() < b.x() && b.x() < plus.x()
        });
        assert!(first.header.widget().is_mapped());
        assert!(!first.term.is_mapped());
        assert!(!first.header.widget().has_css_class("selected"));
        assert!(second.header.widget().has_css_class("selected"));
        wait_for("the hidden terminal stopped receiving output", || {
            first.term.find(crate::terminal::FindOptions { query: "hidden-output".into(), ..Default::default() }).total == 1
        });
        wait_for("the hidden terminal's bell did not mark its header", || first.header.widget().has_css_class("needs-attention"));
        first.term.find_clear();
        first.term.type_text("printf '\\033]7;file://localhost/tmp\\007'\r");
        wait_for("the hidden tab's path did not update", || *first.cwd.borrow() == std::path::Path::new("/tmp"));
        first.header.copy.emit_clicked();
        let copied = glib::MainContext::default().block_on(first.term.clipboard().read_text_future()).unwrap();
        assert_eq!(copied.as_deref(), Some("/tmp"));
        assert!(Rc::ptr_eq(&left.selected_tab().unwrap(), &second));
        app.activate_tab(&first);
        assert!(!first.header.widget().has_css_class("needs-attention"));
        app.activate_tab(&second);

        app.split(&win, gtk::Orientation::Horizontal);
        let right = app.panes_in(&win).pop().unwrap();
        let third = right.selected_tab().unwrap();
        app.run_action(&win, "new_tab");
        let fourth = right.selected_tab().unwrap();
        assert!(Rc::ptr_eq(&left.selected_tab().unwrap(), &second));
        assert!(!right.reorder_tab(first.header.widget(), &third, false));
        assert!(right.reorder_tab(fourth.header.widget(), &third, false));
        assert_eq!(right.headers.first_child().as_ref(), Some(fourth.header.widget()));
        assert!(Rc::ptr_eq(&right.selected_tab().unwrap(), &fourth));

        win.search.open_find();
        app.run_action(&win, "next_tab");
        wait_for("find did not follow the selected terminal", || {
            App::find_target(&win).is_some_and(|tab| Rc::ptr_eq(&tab, &third))
        });

        first.term.type_text("exit\r");
        wait_for("exiting a hidden tab did not close it", || left.tabs.borrow().len() == 1);
        assert!(!left.header_row.has_css_class("multiple-tabs"));
        assert!(Rc::ptr_eq(&app.win_active(&win).unwrap(), &third));

        fourth.header.pop_out.emit_clicked();
        let moved = third.win().unwrap();
        assert!(!Rc::ptr_eq(&win, &moved));
        assert!(Rc::ptr_eq(&moved, &fourth.win().unwrap()));
        assert_eq!(app.panes_in(&moved).len(), 1);
        assert_eq!(right.tabs.borrow().len(), 2);
        assert!(Rc::ptr_eq(&right.selected_tab().unwrap(), &fourth));
        assert!(Rc::ptr_eq(&App::find_target(&win).unwrap(), &second));
        third.header.close.emit_clicked();
        assert!(Rc::ptr_eq(&right.selected_tab().unwrap(), &fourth));
        fourth.term.type_text("printf 'still-%s\\n' running\r");
        wait_for("the remaining tab lost its shell after pop-out", || {
            fourth.term.find(crate::terminal::FindOptions { query: "still-running".into(), ..Default::default() }).total == 1
        });
        app.run_action(&moved, "close_tab");
        wait_for("closing the last tab did not close its window", || app.all_wins().len() == 1);

        for _ in 0..5 {
            left.new_tab_button.emit_clicked();
        }
        let last = left.selected_tab().unwrap();
        wait_for("the selected tab should scroll into view when the row overflows", || {
            let Some(bounds) = last.header.widget().compute_bounds(&left.header_viewport) else { return false };
            left.header_viewport.hadjustment().is_some_and(|adjustment| adjustment.value() > 0.0)
                && bounds.width() > 0.0
                && bounds.x() >= -1.0
                && bounds.x() + bounds.width() <= left.header_viewport.width() as f32 + 1.0
        });
        win.window.close();
        APP.with(|slot| slot.borrow_mut().take());
    }
}
