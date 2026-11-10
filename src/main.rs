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
mod session;
mod statusbar;
mod template;
mod terminal;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

pub const APP_ID: &str = "io.github.gregbacchus.Paua";

const USAGE: &str = "Paua, a colourful terminal.

Usage: paua [install | uninstall | --help]

  install      add the desktop entry and icons to ~/.local/share (shows Paua in the
               GNOME overview and gives the dock its icon)
  uninstall    remove them again
";

fn main() -> glib::ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("install") => return glib::ExitCode::from(install::install() as u8),
        Some("uninstall") => return glib::ExitCode::from(install::uninstall() as u8),
        Some("help" | "--help" | "-h") => {
            print!("{USAGE}");
            return glib::ExitCode::SUCCESS;
        }
        _ => {}
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
    app.set_accels_for_action("win.find", &["<Ctrl><Shift>f"]);
    app.set_accels_for_action("win.command", &["<Ctrl><Shift>p"]);
}

fn build_ui(gtk_app: &adw::Application) {
    app::App::build(gtk_app);
}
