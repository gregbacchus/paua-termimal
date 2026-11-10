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
use crate::frame::Frame;
use crate::header::{self, PaneHeader};
use crate::palette::{self, PaletteItem};
use crate::searchbox::SearchBox;
use crate::session::{self, Kind, SessionInfo};
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
    find_pane: RefCell<Weak<Pane>>,
    status: Rc<StatusBar>,
}

struct Pane {
    card: gtk::Box,
    term: TermView,
    header: PaneHeader,
    win: RefCell<Rc<Win>>,
    cwd: RefCell<PathBuf>,
    host: RefCell<String>,
    title: RefCell<String>,
    /// What the process tree says (ssh / sudo...); the shell's reported host is added on top.
    session: RefCell<SessionInfo>,
    dir: RefCell<Option<DirEntry>>,
    /// Project colour from the nearest `.vscode/settings.json` (read when the directory is
    /// entered and when the pane is focused; not watched).
    vscode_color: RefCell<Option<Rgb>>,
    /// Higher = focused more recently. The highest in a window is that window's active pane.
    focus_seq: Cell<u64>,
    /// Exit status of the last command (shells with OSC 133).
    last_exit: Cell<Option<i32>>,
}

pub struct App {
    gtk_app: adw::Application,
    wins: RefCell<Vec<Rc<Win>>>,
    next_win_id: Cell<u32>,
    cfg: RefCell<UserConfig>,
    panes: RefCell<Vec<Rc<Pane>>>,
    /// The most recently focused pane in any window (drives `.paua.yaml` file watching).
    active: RefCell<Weak<Pane>>,
    focus_counter: Cell<u64>,
    /// Recently visited directories, newest first (for the palette).
    recent_dirs: RefCell<Vec<PathBuf>>,
    /// 0 = follow the system, 1 = light, 2 = dark.
    scheme: Cell<usize>,
    /// Header colours from the config (generated CSS).
    header_css: gtk::CssProvider,
    user_monitor: RefCell<Option<gio::FileMonitor>>,
    dir_monitor: RefCell<Option<gio::FileMonitor>>,
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

/// `(label, window action)` sections of the title-bar menus. Every `win.*` action here is wired
/// to `App::run_action` in `create_window` (except `find` and `command`, which open the search box).
const MENUS: &[(&str, &[&[(&str, &str)]])] = &[
    (
        "_File",
        &[
            &[("_New Window", "win.new-window")],
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
            &[("_Find in Output", "win.find"), ("Command _Palette", "win.command")],
        ],
    ),
    ("_View", &[&[("Cycle _Colour Scheme", "win.cycle-scheme")]]),
    ("_Help", &[&[("_About Paua", "win.about")]]),
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
                    for p in app.panes.borrow().iter() {
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
        let first_pane = app.new_pane(&win, None);
        win.root.append(&first_pane.card);
        first_pane.focus_seq.set(app.next_focus_seq());
        *app.active.borrow_mut() = Rc::downgrade(&first_pane);

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
        first_pane.term.grab_focus();
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
        let search = Rc::new(SearchBox::new());
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
            // Ctrl+Shift+F / Ctrl+Shift+P (the `win.find` and `win.command` actions).
            let (w1, w2) = (Rc::downgrade(&win), Rc::downgrade(&win));
            window.add_action_entries([
                gio::ActionEntry::builder("find")
                    .activate(move |_, _, _| {
                        if let Some(win) = w1.upgrade() {
                            win.search.open_find();
                        }
                    })
                    .build(),
                gio::ActionEntry::builder("command")
                    .activate(move |_, _, _| {
                        if let Some(win) = w2.upgrade() {
                            win.search.open_command();
                        }
                    })
                    .build(),
            ]);
            // The other menu entries: `win.new-window` runs `run_action("new_window")`, and so on.
            for (action, name) in [
                ("new-window", "new_window"),
                ("split-right", "split_right"),
                ("split-down", "split_down"),
                ("pop-out", "pop_out"),
                ("close-pane", "close_pane"),
                ("copy", "copy"),
                ("paste", "paste"),
                ("copy-path", "copy_path"),
                ("clear", "clear"),
                ("cycle-scheme", "cycle_scheme"),
                ("open-config", "open_config"),
                ("reload-config", "reload_config"),
                ("about", "about"),
            ] {
                let (app, w) = (Rc::downgrade(self), Rc::downgrade(&win));
                window.add_action_entries([gio::ActionEntry::builder(action)
                    .activate(move |_, _, _| {
                        if let (Some(app), Some(win)) = (app.upgrade(), w.upgrade()) {
                            app.run_action(&win, name);
                        }
                    })
                    .build()]);
            }
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
                    app.panes.borrow_mut().retain(|p| !Rc::ptr_eq(&p.win.borrow(), &win));
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
        }
    }

    /// A rounded card holding a header line and one terminal, started in `cwd` (or the process
    /// cwd). The caller puts `card` into a window's tree.
    fn new_pane(self: &Rc<Self>, win: &Rc<Win>, cwd: Option<PathBuf>) -> Rc<Pane> {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["pane"])
            .overflow(gtk::Overflow::Hidden) // clip children to the rounded corners
            .hexpand(true)
            .vexpand(true)
            .build();
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

        let pane = Rc::new(Pane {
            card: card.clone(),
            term: term.clone(),
            header,
            win: RefCell::new(win.clone()),
            cwd: RefCell::new(cwd),
            host: RefCell::new(glib::host_name().to_string()),
            title: RefCell::default(),
            session: RefCell::default(),
            dir: RefCell::new(None),
            vscode_color: RefCell::new(None),
            focus_seq: Cell::new(0),
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
                    if app.is_win_active(&pane) {
                        let win = pane.win.borrow().clone();
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
                    app.close_pane(&pane);
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
        let on_click = |button: &gtk::Button, f: fn(&Rc<App>, &Rc<Pane>)| {
            let (app, pane) = (app.clone(), weak_pane.clone());
            button.connect_clicked(move |_| {
                if let (Some(app), Some(pane)) = (app.upgrade(), pane.upgrade()) {
                    f(&app, &pane);
                }
            });
        };
        on_click(&pane.header.copy, App::copy_path);
        on_click(&pane.header.pop_out, App::pop_out);
        on_click(&pane.header.close, App::close_pane);

        self.panes.borrow_mut().push(pane.clone());
        card.append(pane.header.widget());
        card.append(&term);
        pane
    }

    // -----------------------------------------------------------------------------------------
    // Find in output (the title-bar search box)
    // -----------------------------------------------------------------------------------------

    /// The box was opened: it searches the window's active pane.
    fn begin_find(&self, win: &Rc<Win>) {
        *win.find_pane.borrow_mut() = self.win_active(win).map_or_else(Weak::new, |p| Rc::downgrade(&p));
    }

    fn find_target(win: &Rc<Win>) -> Option<Rc<Pane>> {
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
    fn on_find_status(&self, pane: &Rc<Pane>, status: &crate::terminal::FindStatus) {
        let win = pane.win.borrow().clone();
        if Self::find_target(&win).is_some_and(|t| Rc::ptr_eq(&t, pane)) {
            win.search.set_status(status);
        }
    }

    /// The user focused another pane while a find is open: move the find there.
    fn retarget_find(&self, pane: &Rc<Pane>) {
        let win = pane.win.borrow().clone();
        if !win.search.is_open() || Self::find_target(&win).is_some_and(|t| Rc::ptr_eq(&t, pane)) {
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

    /// Bring a pane (and its window) to the front and focus it.
    fn goto_pane(&self, pane: &Rc<Pane>) {
        pane.win.borrow().window.present();
        Self::focus_later(&pane.term);
    }

    /// A new window with one pane, started in `cwd`.
    fn new_window_at(self: &Rc<Self>, cwd: PathBuf) {
        let win = self.create_window(false);
        let pane = self.new_pane(&win, Some(cwd));
        win.root.append(&pane.card);
        pane.focus_seq.set(self.next_focus_seq());
        *self.active.borrow_mut() = Rc::downgrade(&pane);
        self.update_pane_ui();
        self.refresh_frames();
        win.window.present();
        Self::focus_later(&pane.term);
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

        // App-level commands.
        let mut command = |title: &str, hint: &str, f: fn(&Rc<App>, &Rc<Win>)| {
            let (app, w) = (app.clone(), w.clone());
            items.push(PaletteItem::new("Command", title, hint, move || {
                if let (Some(a), Some(w)) = (app.upgrade(), w.upgrade()) {
                    f(&a, &w);
                }
            }));
        };
        command("Find in output", "Ctrl+Shift+F", |_, w| w.search.open_find());
        command("Split right", "new pane to the right", |a, w| a.split(w, gtk::Orientation::Horizontal));
        command("Split down", "new pane below", |a, w| a.split(w, gtk::Orientation::Vertical));
        command("Open pane in new window", "move the focused pane", |a, w| {
            if let Some(p) = a.win_active(w) {
                a.pop_out(&p);
            }
        });
        command("Close pane", "ends its shell", |a, w| {
            if let Some(p) = a.win_active(w) {
                a.close_pane(&p);
            }
        });
        command("Clear terminal", "screen and scrollback of this pane", |a, w| {
            if let Some(p) = a.win_active(w) {
                p.term.clear();
            }
        });
        command("Copy working directory", "to the clipboard", |a, w| {
            if let Some(p) = a.win_active(w) {
                a.copy_path(&p);
            }
        });
        command("New window", "in the current directory", |a, w| {
            let cwd = a.win_active(w).map(|p| p.cwd.borrow().clone()).unwrap_or_default();
            a.new_window_at(cwd);
        });
        command("Cycle colour scheme", "system / light / dark", |a, _| a.cycle_scheme());
        command("Reload config", "re-read config.yaml", |a, _| a.reload_user());
        command("Open config file", "in your default editor", |a, _| a.open_config_file());

        // Your own commands: typed into the focused pane.
        if let Some(pane) = &active {
            for c in &cfg.palette.commands {
                let (term, text) = (pane.term.clone(), format!("{}\r", c.run));
                let sub = c.subtitle.clone().unwrap_or_else(|| c.run.clone());
                items.push(PaletteItem::new("Command", c.title.clone(), sub, move || term.type_text(&text)));
            }
        }

        // Other panes, in any window.
        for p in self.panes.borrow().iter() {
            if active.as_ref().is_some_and(|a| Rc::ptr_eq(a, p)) {
                continue;
            }
            let text = p.header.full_text();
            let title = if text.trim().is_empty() { p.cwd.borrow().display().to_string() } else { text };
            let (app, pane) = (app.clone(), p.clone());
            items.push(PaletteItem::new("Pane", format!("Go to {title}"), "", move || {
                if let Some(a) = app.upgrade() {
                    a.goto_pane(&pane);
                }
            }));
        }

        // `cd` to recent directories, and `ssh` to configured hosts, in the focused pane.
        if let Some(pane) = &active {
            if cfg.palette.directories {
                let home = glib::home_dir().display().to_string();
                let here = pane.cwd.borrow().clone();
                for dir in self.recent_dirs.borrow().iter().filter(|d| **d != here) {
                    let shown = crate::header::replace_prefix(
                        &dir.display().to_string(),
                        &cfg.panes.header.path_replacements,
                        Some(&home),
                    );
                    let (term, text) =
                        (pane.term.clone(), format!("cd {}\r", palette::shell_quote(&dir.display().to_string())));
                    items.push(PaletteItem::new("Directory", format!("cd {shown}"), "", move || term.type_text(&text)));
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
                            let shown = header::replace_prefix(&full, &cfg.panes.header.path_replacements, Some(&home));
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
            path: header::replace_prefix(&cwd_text, &cfg.panes.header.path_replacements, Some(&home)),
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
            "cycle_scheme" => self.cycle_scheme(),
            "about" => Self::show_about(win),
            "reload_config" => self.reload_user(),
            "open_config" => self.open_config_file(),
            "clear" | "close_pane" | "pop_out" | "copy_path" | "copy" | "paste" if pane.is_none() => {}
            "copy" => pane.unwrap().term.copy(),
            "paste" => pane.unwrap().term.paste_clipboard(),
            "clear" => pane.unwrap().term.clear(),
            "close_pane" => self.close_pane(&pane.unwrap()),
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

    /// Copy the pane's real working directory (not the shortened display form).
    fn copy_path(self: &Rc<Self>, pane: &Rc<Pane>) {
        let path = pane.cwd.borrow().display().to_string();
        pane.term.clipboard().set_text(&path);
        let win = pane.win.borrow().clone();
        self.toast_in(&win, "Path copied");
    }

    fn panes_in(&self, win: &Rc<Win>) -> Vec<Rc<Pane>> {
        self.panes
            .borrow()
            .iter()
            .filter(|p| Rc::ptr_eq(&p.win.borrow(), win))
            .cloned()
            .collect()
    }

    /// The most recently focused pane in `win`.
    fn win_active(&self, win: &Rc<Win>) -> Option<Rc<Pane>> {
        self.panes_in(win).into_iter().max_by_key(|p| p.focus_seq.get())
    }

    fn is_win_active(&self, pane: &Rc<Pane>) -> bool {
        let win = pane.win.borrow().clone();
        self.win_active(&win).is_some_and(|a| Rc::ptr_eq(&a, pane))
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
        let card = target.card.clone();
        let Some(parent) = card.parent() else { return };

        let (w, h) = (card.width(), card.height());
        let new_pane = self.new_pane(win, Some(target.cwd.borrow().clone()));
        let paned = Self::new_paned(orientation);
        paned.set_position(if orientation == gtk::Orientation::Horizontal { w / 2 } else { h / 2 });

        // Put the new Paned where the old card was, with the card and the new pane inside it.
        // The card has to be detached from its old parent before it can join the new one.
        let placeholder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        Self::replace_child(&parent, card.upcast_ref(), placeholder.upcast_ref());
        paned.set_start_child(Some(&card));
        paned.set_end_child(Some(&new_pane.card));
        Self::replace_child(&parent, placeholder.upcast_ref(), paned.upcast_ref());

        self.update_pane_ui();
        Self::focus_later(&new_pane.term);
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
        GtkWindowExt::set_focus(&win.window, None::<&gtk::Widget>);
        paned.set_start_child(None::<&gtk::Widget>);
        paned.set_end_child(None::<&gtk::Widget>);
        Self::replace_child(&grandparent, paned.upcast_ref(), &sibling);
        true
    }

    fn focus_later(term: &TermView) {
        let term = term.clone();
        glib::idle_add_local_once(move || {
            term.grab_focus();
        });
    }

    /// Close a pane (its shell exited, or the user clicked close). A sibling takes over the freed
    /// space; closing a window's last pane closes that window (and the app, if it was the last).
    fn close_pane(self: &Rc<Self>, pane: &Rc<Pane>) {
        let win = pane.win.borrow().clone();
        if self.detach_card(pane) {
            self.panes.borrow_mut().retain(|p| !Rc::ptr_eq(p, pane));
            if let Some(next) = self.win_active(&win) {
                Self::focus_later(&next.term);
            }
            self.update_pane_ui();
            self.refresh_frames();
            self.watch_dir();
            return;
        }

        // The window's only pane: close the window (its close handler tidies up the rest).
        self.panes.borrow_mut().retain(|p| !Rc::ptr_eq(p, pane));
        win.window.close();
    }

    /// Move a pane, with its running shell, into a new full window. Offered only while its current
    /// window would keep at least one other pane.
    fn pop_out(self: &Rc<Self>, pane: &Rc<Pane>) {
        let from = pane.win.borrow().clone();
        if self.panes_in(&from).len() < 2 || !self.detach_card(pane) {
            return;
        }

        let win = self.create_window(false);
        *pane.win.borrow_mut() = win.clone();
        win.root.append(&pane.card);

        pane.focus_seq.set(self.next_focus_seq());
        *self.active.borrow_mut() = Rc::downgrade(pane);
        if let Some(next) = self.win_active(&from) {
            Self::focus_later(&next.term);
        }

        self.refresh_header(pane);
        self.update_pane_ui();
        self.refresh_frames();
        self.watch_dir();
        win.window.present();
        Self::focus_later(&pane.term);
    }

    /// Per window: dim every pane except that window's active one (only when it has several),
    /// enable the pop-out button only where it makes sense, and refresh the window title.
    fn update_pane_ui(&self) {
        for win in self.all_wins() {
            self.refresh_status(&win);
            let panes = self.panes_in(&win);
            let active = self.win_active(&win);
            let multiple = panes.len() > 1;
            for p in &panes {
                let inactive = multiple && !active.as_ref().is_some_and(|a| Rc::ptr_eq(a, p));
                p.term.set_dimmed(inactive);
                if inactive {
                    p.card.add_css_class("inactive");
                } else {
                    p.card.remove_css_class("inactive");
                }
                p.header.pop_out.set_sensitive(multiple);
            }
            self.refresh_window_title(&win);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Pane state: header text, working directory, focus
    // -----------------------------------------------------------------------------------------

    /// Rebuild the header from the template. (Path trimming to fit the pane width happens in the
    /// header widget itself, since it depends on the pane's size.)
    fn refresh_header(&self, pane: &Rc<Pane>) {
        let info = self.effective_session(pane);
        {
            let cfg = self.cfg.borrow();
            let hc = &cfg.panes.header;
            let (session_text, badges) = session_labels(&cfg, &info);
            let cwd = pane.cwd.borrow().display().to_string();
            let home = glib::home_dir().display().to_string();
            pane.header.set_input(header::HeaderInput {
                template: hc.template.clone(),
                path: header::replace_prefix(&cwd, &hc.path_replacements, Some(&home)),
                dir: header::dir_name(&cwd).to_owned(),
                title: pane.title.borrow().clone(),
                host: pane.host.borrow().clone(),
                user: glib::user_name().to_string_lossy().into_owned(),
                session: session_text,
                trim: hc.trim_path,
                ellipsis: hc.path_ellipsis.clone(),
            });
            pane.header.set_badges(&badges);
            pane.header.set_session_class(info.items.first().map(|i| i.kind.header_class()));
        }
        let win = pane.win.borrow().clone();
        self.refresh_window_title(&win);
        self.refresh_status(&win);
    }

    /// The process-tree detection plus "the shell reports another host" (OSC 7), restricted to
    /// the kinds the config enables.
    fn effective_session(&self, pane: &Rc<Pane>) -> SessionInfo {
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
    fn refresh_session(self: &Rc<Self>, pane: &Rc<Pane>) {
        self.refresh_header(pane);
        if self.is_win_active(pane) {
            self.refresh_frames();
        }
    }

    fn is_active_anywhere(&self, pane: &Rc<Pane>) -> bool {
        self.active.borrow().upgrade().is_some_and(|a| Rc::ptr_eq(&a, pane))
    }

    /// The shell reported a new working directory.
    fn on_pwd(self: &Rc<Self>, pane: &Rc<Pane>, uri: &str) {
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
    fn resolve_dir(&self, pane: &Rc<Pane>) {
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
    fn load_vscode_color(&self, pane: &Rc<Pane>) {
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
    fn reload_dir_entry(&self, pane: &Rc<Pane>) {
        let mut dir = pane.dir.borrow_mut();
        if let Some(entry) = dir.as_mut() {
            match config::load_dir(&entry.path) {
                Ok(cfg) => entry.cfg = cfg,
                Err(e) => self.toast(&format!("Config error: {e}")),
            }
        }
    }

    fn set_active(self: &Rc<Self>, pane: &Rc<Pane>) {
        if self.is_active_anywhere(pane) && self.is_win_active(pane) {
            return;
        }
        pane.focus_seq.set(self.next_focus_seq());
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
        let base = self.cfg.borrow().window_color(dark);
        for win in self.all_wins() {
            let mut color = base;
            let mut session_color = None;
            if let Some(pane) = self.win_active(&win) {
                let info = self.effective_session(&pane);
                let cfg = self.cfg.borrow();
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
            win.frame.set(&win.window, session_color.unwrap_or(color));
        }
    }

    /// Pane header background tint and text colour from the config, per light/dark mode.
    fn apply_header_css(&self) {
        let cfg = self.cfg.borrow();
        let hc = &cfg.panes.header;
        let mut css = String::new();
        for (mode, dark) in [("dark", true), ("light", false)] {
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
            .map(|p| p.win.borrow().clone())
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

    fn setup_user_monitor(self: &Rc<Self>) {
        *self.user_monitor.borrow_mut() = None;
        if !self.cfg.borrow().watch {
            return;
        }
        let file = gio::File::for_path(config::user_config_path());
        let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        else {
            return;
        };
        let weak = Rc::downgrade(self);
        monitor.connect_changed(move |_, _, _, _| {
            let Some(app) = weak.upgrade() else { return };
            // Editors often write several events per save: debounce.
            let generation = app.user_gen.get().wrapping_add(1);
            app.user_gen.set(generation);
            let weak = Rc::downgrade(&app);
            glib::timeout_add_local_once(Duration::from_millis(250), move || {
                if let Some(app) = weak.upgrade() {
                    if app.user_gen.get() == generation {
                        app.reload_user();
                    }
                }
            });
        });
        *self.user_monitor.borrow_mut() = Some(monitor);
    }

    fn reload_user(self: &Rc<Self>) {
        match config::load_user() {
            Ok(cfg) => {
                *self.cfg.borrow_mut() = cfg;
                self.apply_config();
                self.toast("Config reloaded");
            }
            Err(e) => self.toast(&format!("Config error (keeping previous settings): {e}")),
        }
    }

    /// Apply the settings that can change at runtime. Window size/maximized are startup-only.
    fn apply_config(self: &Rc<Self>) {
        let panes: Vec<Rc<Pane>> = self.panes.borrow().clone();
        {
            let cfg = self.cfg.borrow();
            let font = cfg.font.pango_string();
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
        for win in self.all_wins() {
            win.status.configure(&self.cfg.borrow().status_bar);
            self.refresh_status(&win);
            win.frame.set_backdrop(self.cfg.borrow().window.inactive_dim);
            let cfg = self.cfg.borrow();
            win.search.set_defaults(cfg.search.case, cfg.search.regex);
            win.search.set_max_results(cfg.palette.max_results);
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
        *self.dir_monitor.borrow_mut() = None;
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
        let Ok(monitor) = gio::File::for_path(&path)
            .monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        else {
            return;
        };
        let weak = Rc::downgrade(self);
        monitor.connect_changed(move |_, _, _, _| {
            let Some(app) = weak.upgrade() else { return };
            let generation = app.dir_gen.get().wrapping_add(1);
            app.dir_gen.set(generation);
            let weak = Rc::downgrade(&app);
            let path = path.clone();
            glib::timeout_add_local_once(Duration::from_millis(250), move || {
                let Some(app) = weak.upgrade() else { return };
                if app.dir_gen.get() != generation {
                    return;
                }
                // Reload this file for every pane that uses it, then recolour.
                for pane in app.panes.borrow().iter() {
                    let mut dir = pane.dir.borrow_mut();
                    if let Some(entry) = dir.as_mut().filter(|e| e.path == path) {
                        match config::load_dir(&path) {
                            Ok(cfg) => entry.cfg = cfg,
                            Err(e) => app.toast(&format!("Config error: {e}")),
                        }
                    }
                }
                app.refresh_frames();
            });
        });
        *self.dir_monitor.borrow_mut() = Some(monitor);
    }
}
