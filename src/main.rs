mod app;
mod cmdtimer;
mod config;
mod exec;
mod frame;
mod git;
mod header;
mod install;
mod palette;
mod searchbox;
mod setup;
mod session;
mod statusbar;
mod template;
mod terminal;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

pub const APP_ID: &str = "io.github.gregbacchus.Paua";

const USAGE: &str = "Paua, a colourful terminal.

Usage: paua [setup | install | uninstall | --help]

  setup        first-run wizard: finds your Nerd Fonts, asks which to use for the terminal and for
               the window decorations, writes ~/.config/paua/config.yaml, and offers to add Paua
               to the GNOME overview
                 --yes, -y   take the suggestions without asking
                 --print     show the config instead of writing it
  install      add the desktop entry and icons to ~/.local/share (shows Paua in the GNOME
               overview and gives the dock its icon); offers `setup` if there is no config yet
                 --yes, -y   run `setup` without asking     --no-config   don't offer it
  uninstall    remove the desktop entry and icons again
";

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(cmd) = args.first().map(String::as_str) {
        let flags = &args[1..];
        let known = |allowed: &[&str]| flags.iter().all(|f| allowed.contains(&f.as_str()));
        let usage_error = || {
            eprintln!("{USAGE}");
            glib::ExitCode::from(2)
        };
        let has = |names: &[&str]| flags.iter().any(|f| names.contains(&f.as_str()));
        match cmd {
            "setup" if known(&["--yes", "-y", "--print"]) => {
                return glib::ExitCode::from(setup::run(setup::Options {
                    assume_yes: has(&["--yes", "-y"]),
                    print_only: has(&["--print"]),
                    offer_install: true,
                }) as u8);
            }
            "install" if known(&["--yes", "-y", "--no-config"]) => {
                return glib::ExitCode::from(install::install(install::InstallOptions {
                    assume_yes: has(&["--yes", "-y"]),
                    no_config: has(&["--no-config"]),
                }) as u8);
            }
            "uninstall" if known(&[]) => return glib::ExitCode::from(install::uninstall() as u8),
            "help" | "--help" | "-h" => {
                print!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            "setup" | "install" | "uninstall" => return usage_error(),
            _ => {} // anything else goes to GTK (its own options)
        }
    }

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE) // sandbox: allow several instances
        .build();

    app.connect_startup(|app| {
        load_icons();
        load_css();
        install_actions(app);
    });
    app.connect_activate(build_ui);

    app.run()
}

/// Make the app icon findable by name. For development we point GTK at ./assets/icons; after
/// `paua install` (or in a package) it is found in the user's / system icon directories.
fn load_icons() {
    let display = gdk::Display::default().expect("no display");
    gtk::IconTheme::for_display(&display)
        .add_search_path(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons"));
    gtk::Window::set_default_icon_name(APP_ID);
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn install_actions(app: &adw::Application) {
    app.add_action_entries([gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| app.quit())
        .build()]);
    app.set_accels_for_action("app.quit", &["<Ctrl>q"]);
    app.set_accels_for_action("win.new-window", &["<Ctrl><Shift>n"]);
    app.set_accels_for_action("win.close-pane", &["<Ctrl><Shift>w"]);
    app.set_accels_for_action("win.copy", &["<Ctrl><Shift>c"]);
    app.set_accels_for_action("win.paste", &["<Ctrl><Shift>v"]);
    app.set_accels_for_action("win.find", &["<Ctrl><Shift>f"]);
    app.set_accels_for_action("win.command", &["<Ctrl><Shift>p"]);
}

fn build_ui(gtk_app: &adw::Application) {
    app::App::build(gtk_app);
}
