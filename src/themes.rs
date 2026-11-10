//! Terminal colour themes: the foreground, background, cursor, selection and the ANSI palette.
//!
//! A theme is chosen per light/dark mode (`terminal.theme`), comes from the built-in list, from a
//! file in `~/.config/paua/themes/` (Paua YAML, or the Ghostty theme format) or from
//! `~/.config/ghostty/themes/`, and is then adjusted by `terminal.colors`.

use std::path::{Path, PathBuf};

use crate::config::{self, ColorsConfig};
use crate::terminal::Rgb;

/// The default theme: the colours the app has always used (and the emulator's default palette).
pub const DEFAULT_THEME: &str = "paua";

#[derive(Debug, Clone, PartialEq)]
pub struct ThemeColors {
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Option<Rgb>,
    pub selection_bg: Option<Rgb>,
    /// Palette entries this theme sets (index 0-255); the rest keep the emulator's defaults.
    pub palette: Vec<(u8, Rgb)>,
}

/// A built-in theme as plain data.
struct Builtin {
    name: &'static str,
    fg: &'static str,
    bg: &'static str,
    cursor: &'static str,
    selection: &'static str,
    /// 0-7 normal, 8-15 bright.
    ansi: [&'static str; 16],
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "dracula",
        fg: "#f8f8f2",
        bg: "#282a36",
        cursor: "#f8f8f2",
        selection: "#44475a",
        ansi: [
            "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2", "#6272a4",
            "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
        ],
    },
    Builtin {
        name: "nord",
        fg: "#d8dee9",
        bg: "#2e3440",
        cursor: "#d8dee9",
        selection: "#434c5e",
        ansi: [
            "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0", "#4c566a",
            "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4",
        ],
    },
    Builtin {
        name: "solarized-dark",
        fg: "#839496",
        bg: "#002b36",
        cursor: "#93a1a1",
        selection: "#073642",
        ansi: [
            "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5", "#002b36",
            "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
        ],
    },
    Builtin {
        name: "solarized-light",
        fg: "#657b83",
        bg: "#fdf6e3",
        cursor: "#586e75",
        selection: "#eee8d5",
        ansi: [
            "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5", "#002b36",
            "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
        ],
    },
    Builtin {
        name: "gruvbox-dark",
        fg: "#ebdbb2",
        bg: "#282828",
        cursor: "#ebdbb2",
        selection: "#504945",
        ansi: [
            "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984", "#928374",
            "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
        ],
    },
    Builtin {
        name: "gruvbox-light",
        fg: "#3c3836",
        bg: "#fbf1c7",
        cursor: "#3c3836",
        selection: "#d5c4a1",
        ansi: [
            "#fbf1c7", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#7c6f64", "#928374",
            "#9d0006", "#79740e", "#b57614", "#076678", "#8f3f71", "#427b58", "#3c3836",
        ],
    },
    Builtin {
        name: "tokyo-night",
        fg: "#c0caf5",
        bg: "#1a1b26",
        cursor: "#c0caf5",
        selection: "#283457",
        ansi: [
            "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6", "#414868",
            "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
        ],
    },
    Builtin {
        name: "catppuccin-mocha",
        fg: "#cdd6f4",
        bg: "#1e1e2e",
        cursor: "#f5e0dc",
        selection: "#585b70",
        ansi: [
            "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de", "#585b70",
            "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
        ],
    },
    Builtin {
        name: "catppuccin-latte",
        fg: "#4c4f69",
        bg: "#eff1f5",
        cursor: "#dc8a78",
        selection: "#acb0be",
        ansi: [
            "#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#acb0be", "#6c6f85",
            "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#bcc0cc",
        ],
    },
    Builtin {
        name: "one-dark",
        fg: "#abb2bf",
        bg: "#282c34",
        cursor: "#528bff",
        selection: "#3e4451",
        ansi: [
            "#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf", "#5c6370",
            "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff",
        ],
    },
    Builtin {
        name: "one-light",
        fg: "#383a42",
        bg: "#fafafa",
        cursor: "#526fff",
        selection: "#e5e5e6",
        ansi: [
            "#383a42", "#e45649", "#50a14f", "#c18401", "#0184bc", "#a626a4", "#0997b3", "#fafafa", "#4f525e",
            "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff",
        ],
    },
];

/// Names that mean "the `-dark` or `-light` variant, whichever suits the colour scheme".
const FAMILIES: &[&str] = &["solarized", "gruvbox", "one"];

/// The theme names to show to users (family names count once), for docs and error messages.
pub fn builtin_names() -> Vec<&'static str> {
    let mut v = vec![DEFAULT_THEME, "catppuccin", "dracula", "gruvbox", "nord", "one", "solarized", "tokyo-night"];
    v.sort_unstable();
    v
}

fn rgb(s: &str) -> Rgb {
    config::parse_hex(s).expect("built-in theme colours are valid")
}

fn from_builtin(b: &Builtin) -> ThemeColors {
    ThemeColors {
        fg: rgb(b.fg),
        bg: rgb(b.bg),
        cursor: Some(rgb(b.cursor)),
        selection_bg: Some(rgb(b.selection)),
        palette: b.ansi.iter().enumerate().map(|(i, c)| (i as u8, rgb(c))).collect(),
    }
}

/// The emulator's own 16 default colours, which are the `paua` theme's palette.
fn default_palette16() -> &'static [Rgb; 16] {
    static PALETTE: std::sync::OnceLock<[Rgb; 16]> = std::sync::OnceLock::new();
    PALETTE.get_or_init(|| {
        let mut out = [Rgb(0, 0, 0); 16];
        let opts = libghostty_vt::TerminalOptions { cols: 2, rows: 1, max_scrollback: 0 };
        if let Ok(p) = libghostty_vt::Terminal::new(opts).and_then(|t| t.default_color_palette()) {
            for (o, c) in out.iter_mut().zip(p.0.iter()) {
                *o = Rgb::from(*c);
            }
        }
        out
    })
}

/// `paua`: the app's own look. Text and background as the pane cards have always been, with the
/// emulator's standard 16 colours spelled out (so previews and `terminal.colors` see them).
fn paua(dark: bool) -> ThemeColors {
    let (fg, bg) = if dark { (Rgb(0xd4, 0xd4, 0xd8), Rgb(0x1e, 0x1e, 0x22)) } else { (Rgb(0x2e, 0x2e, 0x33), Rgb(0xfb, 0xfb, 0xfc)) };
    let palette = default_palette16().iter().enumerate().map(|(i, c)| (i as u8, *c)).collect();
    ThemeColors { fg, bg, cursor: None, selection_bg: None, palette }
}

/// A built-in theme by name, resolving `solarized` to `solarized-dark` / `solarized-light` for the
/// colour scheme (and `catppuccin` to mocha / latte).
pub fn builtin(name: &str, dark: bool) -> Option<ThemeColors> {
    let n = name.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    if n == DEFAULT_THEME {
        return Some(paua(dark));
    }
    let exact = |n: &str| BUILTINS.iter().find(|b| b.name == n).map(from_builtin);
    if let Some(t) = exact(&n) {
        return Some(t);
    }
    if n == "catppuccin" {
        return exact(if dark { "catppuccin-mocha" } else { "catppuccin-latte" });
    }
    if FAMILIES.contains(&n.as_str()) {
        return exact(&format!("{n}-{}", if dark { "dark" } else { "light" }));
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Theme files
// ---------------------------------------------------------------------------------------------

/// A theme file in the Ghostty format: `key = value` lines with `background`, `foreground`,
/// `cursor-color`, `selection-background` and `palette = N=#rrggbb`. `#` starts a comment.
pub fn parse_ghostty_theme(text: &str, base: ThemeColors) -> ThemeColors {
    let mut t = base;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        let colour = |v: &str| config::parse_hex(v.trim_matches('"')).ok();
        match key {
            "background" => t.bg = colour(value).unwrap_or(t.bg),
            "foreground" => t.fg = colour(value).unwrap_or(t.fg),
            "cursor-color" => t.cursor = colour(value).or(t.cursor),
            "selection-background" => t.selection_bg = colour(value).or(t.selection_bg),
            "palette" => {
                if let Some((n, c)) = value.split_once('=') {
                    if let (Ok(n), Some(c)) = (n.trim().parse::<u8>(), colour(c.trim())) {
                        set_palette(&mut t.palette, n, c);
                    }
                }
            }
            _ => {}
        }
    }
    t
}

fn set_palette(palette: &mut Vec<(u8, Rgb)>, index: u8, colour: Rgb) {
    match palette.iter_mut().find(|(i, _)| *i == index) {
        Some(e) => e.1 = colour,
        None => palette.push((index, colour)),
    }
}

/// Apply `terminal.colors` (or a Paua YAML theme file, which has the same keys) on top of `base`.
pub fn apply_colors(colors: &ColorsConfig, base: ThemeColors, dark: bool) -> ThemeColors {
    let mut t = base;
    if let Some(c) = colors.foreground.and_then(|c| c.resolve(dark)) {
        t.fg = c;
    }
    if let Some(c) = colors.background.and_then(|c| c.resolve(dark)) {
        t.bg = c;
    }
    if let Some(c) = colors.cursor.and_then(|c| c.resolve(dark)) {
        t.cursor = Some(c);
    }
    if let Some(c) = colors.selection_background.and_then(|c| c.resolve(dark)) {
        t.selection_bg = Some(c);
    }
    for (i, c) in &colors.palette {
        if let Some(c) = c.resolve(dark) {
            set_palette(&mut t.palette, *i, c);
        }
    }
    t
}

/// Where the installed collection (`paua themes install`) lives.
pub fn collection_dir() -> PathBuf {
    gtk::glib::user_data_dir().join("paua").join("themes")
}

/// Where theme files are looked for, in order: your own (`~/.config/paua/themes`), the installed
/// collection (`~/.local/share/paua/themes`), Ghostty's user themes, and the themes of a system
/// installation of Ghostty.
pub fn theme_dirs() -> Vec<PathBuf> {
    let cfg = gtk::glib::user_config_dir();
    let mut dirs = vec![cfg.join("paua").join("themes"), collection_dir(), cfg.join("ghostty").join("themes")];
    for d in gtk::glib::system_data_dirs() {
        dirs.push(d.join("ghostty").join("themes"));
    }
    dirs
}

/// File names in `dir` that `name` can mean: exactly, with `.yaml` / `.yml`, or (when none of those
/// exist) a file whose name differs only in case, so `catppuccin mocha` finds `Catppuccin Mocha`.
fn candidates(dir: &Path, name: &str) -> Vec<String> {
    let exact = [name.to_owned(), format!("{name}.yaml"), format!("{name}.yml")];
    if exact.iter().any(|c| dir.join(c).is_file()) {
        return exact.to_vec();
    }
    let lower = name.to_lowercase();
    let mut found = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            let stem = f.strip_suffix(".yaml").or_else(|| f.strip_suffix(".yml")).unwrap_or(&f).to_lowercase();
            if stem == lower {
                found.push(f);
            }
        }
    }
    found.sort();
    found
}

/// The names of the theme files in `dirs` (no duplicates, case-insensitively sorted).
pub fn installed_names(dirs: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            if f.starts_with('.') || !e.path().is_file() {
                continue;
            }
            let stem = f.strip_suffix(".yaml").or_else(|| f.strip_suffix(".yml")).unwrap_or(&f).to_owned();
            if !names.iter().any(|n| n.eq_ignore_ascii_case(&stem)) {
                names.push(stem);
            }
        }
    }
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// A theme from a file named `name`, `name.yaml` or `name.yml` in `dirs` (first match wins).
/// `.yaml` / `.yml` files use the `terminal.colors` keys; any other file is read as a Ghostty theme.
pub fn from_files(name: &str, dark: bool, dirs: &[PathBuf]) -> Result<Option<ThemeColors>, String> {
    // A name is a file name, never a path.
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Ok(None);
    }
    for dir in dirs {
        for candidate in candidates(dir, name) {
            let path = dir.join(&candidate);
            if !path.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let base = paua(dark);
            return Ok(Some(if candidate.ends_with(".yaml") || candidate.ends_with(".yml") {
                let colors: ColorsConfig = serde_yaml_ng::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                apply_colors(&colors, base, dark)
            } else {
                parse_ghostty_theme(&text, base)
            }));
        }
    }
    Ok(None)
}

/// The theme for the colour scheme `dark`, with `terminal.colors` applied. An unknown or broken
/// theme gives the default theme and a message for the user.
pub fn resolve(cfg: &config::TerminalConfig, dark: bool) -> (ThemeColors, Option<String>) {
    resolve_in(cfg, dark, &theme_dirs())
}

pub fn resolve_in(cfg: &config::TerminalConfig, dark: bool, dirs: &[PathBuf]) -> (ThemeColors, Option<String>) {
    let name = cfg.theme.name_for(dark);
    let mut warning = None;
    let base = match builtin(name, dark) {
        Some(t) => t,
        None => match from_files(name, dark, dirs) {
            Ok(Some(t)) => t,
            Ok(None) => {
                warning = Some(format!(
                    "Unknown terminal theme {name:?} (built in: {}; or a file in {})",
                    builtin_names().join(", "),
                    dirs.first().map_or(Path::new("themes"), |d| d.as_path()).display()
                ));
                paua(dark)
            }
            Err(e) => {
                warning = Some(format!("Terminal theme: {e}"));
                paua(dark)
            }
        },
    };
    (apply_colors(&cfg.colors, base, dark), warning)
}

// ---------------------------------------------------------------------------------------------
// `paua themes`
// ---------------------------------------------------------------------------------------------

const COLLECTION_REPO: &str = "https://github.com/mbadolato/iTerm2-Color-Schemes";

fn print_swatches(name: &str, t: &ThemeColors) {
    let bg = |c: Rgb| format!("\x1b[48;2;{};{};{}m", c.0, c.1, c.2);
    let fg = |c: Rgb| format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2);
    println!("{name}");
    println!("  {}{} The quick brown fox jumps over the lazy dog \x1b[0m", bg(t.bg), fg(t.fg));
    let palette = |i: u8| t.palette.iter().find(|(n, _)| *n == i).map(|(_, c)| *c);
    for row in [0u8, 8] {
        let mut line = String::from("  ");
        for i in row..row + 8 {
            match palette(i) {
                Some(c) => line += &format!("{}  {:>2} \x1b[0m", bg(c), i),
                None => line += &format!("\x1b[2m {i:>2}  \x1b[0m"),
            }
        }
        println!("{line}");
    }
}

/// Download the community collection (Ghostty-format files from the iTerm2-Color-Schemes project,
/// ~900 themes, ~4 MB) with a sparse `git clone` and copy it to [`collection_dir`].
fn install_collection() -> i32 {
    use crate::exec;
    use std::process::Command;
    use std::time::Duration;

    let target = collection_dir();
    let work = gtk::glib::user_cache_dir().join("paua").join(format!("themes-download-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    if let Err(e) = std::fs::create_dir_all(&work) {
        eprintln!("paua: {e}");
        return 1;
    }
    println!("Downloading the theme collection from {COLLECTION_REPO} ...");
    let git = |args: &[&str], dir: &Path, secs: u64| -> Result<(), String> {
        let mut c = Command::new("git");
        c.args(args).current_dir(dir).env("GIT_TERMINAL_PROMPT", "0");
        match exec::run(c, Duration::from_secs(secs)) {
            Ok(o) if o.success => Ok(()),
            Ok(o) => Err(exec::first_line(&o.stderr)),
            Err(e) => Err(format!("git: {e}")),
        }
    };
    let result = (|| -> Result<usize, String> {
        git(&["clone", "-q", "--depth", "1", "--filter=blob:none", "--sparse", COLLECTION_REPO, "repo"], &work, 180)?;
        let repo = work.join("repo");
        git(&["sparse-checkout", "set", "ghostty"], &repo, 120)?;
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        let mut n = 0;
        for e in std::fs::read_dir(repo.join("ghostty")).map_err(|e| e.to_string())?.flatten() {
            if e.path().is_file() {
                std::fs::copy(e.path(), target.join(e.file_name())).map_err(|e| e.to_string())?;
                n += 1;
            }
        }
        // Keep the licence with the files.
        if let Some(parent) = target.parent() {
            let _ = std::fs::copy(repo.join("LICENSE"), parent.join("themes-LICENSE"));
            let _ = std::fs::write(
                parent.join("themes-README.txt"),
                format!("The themes in this folder come from {COLLECTION_REPO} (MIT licence, see themes-LICENSE).\nRe-run `paua themes install` to update them.\n"),
            );
        }
        Ok(n)
    })();
    let _ = std::fs::remove_dir_all(&work);
    match result {
        Ok(n) => {
            println!("Installed {n} themes into {}.", target.display());
            println!("Use one with `terminal: {{ theme: \"Catppuccin Mocha\" }}`; list them with `paua themes list`.");
            0
        }
        Err(e) => {
            eprintln!("paua: couldn't download the themes: {e}\n(needs `git` and network access)");
            1
        }
    }
}

pub const THEMES_USAGE: &str = "Usage: paua themes <command>

  list [text]        built-in and installed themes (only those containing `text`, if given)
  preview <name>     show a theme's colours in this terminal (add --light for the light scheme)
  install            download ~900 community themes (from iTerm2-Color-Schemes, Ghostty format)
                     into ~/.local/share/paua/themes
";

/// `paua themes ...`
pub fn cli(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("list") => {
            let filter = args.get(1).map(|s| s.to_lowercase());
            let ok = |n: &str| filter.as_ref().is_none_or(|f| n.to_lowercase().contains(f));
            let dirs = theme_dirs();
            let installed: Vec<String> = installed_names(&dirs).into_iter().filter(|n| ok(n)).collect();
            println!("Built in:");
            for n in builtin_names().into_iter().filter(|n| ok(n)) {
                println!("  {n}");
            }
            println!("\nInstalled ({}):", installed.len());
            for n in &installed {
                println!("  {n}");
            }
            if installed.is_empty() && filter.is_none() {
                println!("  (none yet: `paua themes install` downloads ~900 of them)");
            }
            0
        }
        Some("preview") => {
            let Some(name) = args.get(1) else {
                eprint!("{THEMES_USAGE}");
                return 2;
            };
            let dark = !args.iter().any(|a| a == "--light");
            let found = builtin(name, dark).or_else(|| from_files(name, dark, &theme_dirs()).ok().flatten());
            match found {
                Some(t) => {
                    print_swatches(name, &t);
                    0
                }
                None => {
                    eprintln!("paua: no theme called {name:?} (see `paua themes list`)");
                    1
                }
            }
        }
        Some("install") => install_collection(),
        _ => {
            eprint!("{THEMES_USAGE}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_theme_is_valid() {
        for b in BUILTINS {
            let t = from_builtin(b); // panics on a malformed colour
            assert_eq!(t.palette.len(), 16, "{}", b.name);
        }
        for n in builtin_names() {
            assert!(builtin(n, true).is_some() && builtin(n, false).is_some(), "{n}");
        }
    }

    #[test]
    fn families_follow_the_colour_scheme() {
        assert_eq!(builtin("solarized", true).unwrap().bg, rgb("#002b36"));
        assert_eq!(builtin("solarized", false).unwrap().bg, rgb("#fdf6e3"));
        assert_eq!(builtin("Catppuccin", false).unwrap().bg, rgb("#eff1f5"));
        assert_eq!(builtin("gruvbox-light", true).unwrap().bg, rgb("#fbf1c7")); // explicit wins
        assert_eq!(builtin("dracula", false).unwrap().bg, rgb("#282a36")); // single-variant: both modes
        assert_eq!(builtin("tokyo night", true).unwrap().bg, rgb("#1a1b26"));
        assert!(builtin("nope", true).is_none());
        // paua is the app's own look: the emulator's standard 16 colours, spelled out.
        let p = builtin("paua", true).unwrap().palette;
        assert_eq!(p.len(), 16);
        assert!(p.iter().any(|(_, c)| *c != Rgb(0, 0, 0)), "palette was read from the emulator");
        assert_eq!(builtin("paua", false).unwrap().bg, Rgb(0xfb, 0xfb, 0xfc));
    }

    #[test]
    fn ghostty_theme_files_are_parsed() {
        let text = "# a comment\nbackground = #101010\nforeground=\"#eeeeee\"\ncursor-color = #ff00ff\nselection-background = #333333\npalette = 1=#ff0000\npalette = 4 = #0000ff\npalette = 300=#123456\nbogus line\n";
        let t = parse_ghostty_theme(text, paua(true));
        assert_eq!((t.bg, t.fg), (Rgb(16, 16, 16), Rgb(238, 238, 238)));
        assert_eq!(t.cursor, Some(Rgb(255, 0, 255)));
        assert_eq!(t.selection_bg, Some(Rgb(51, 51, 51)));
        let at = |i: u8| t.palette.iter().find(|(n, _)| *n == i).map(|(_, c)| *c);
        assert_eq!((at(1), at(4)), (Some(Rgb(255, 0, 0)), Some(Rgb(0, 0, 255))));
        assert_eq!(at(2), paua(true).palette.iter().find(|(n, _)| *n == 2).map(|(_, c)| *c)); // untouched entries keep the default
        assert_eq!(t.palette.len(), 16); // 300 is not a palette index
    }

    #[test]
    fn user_theme_files_are_found_and_overrides_win() {
        let dir = std::env::temp_dir().join(format!("paua-themes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mine.yaml"), "background: \"#111111\"\npalette: { 2: \"#00ff00\" }\n").unwrap();
        std::fs::write(dir.join("gh"), "background = #222222\n").unwrap();
        let dirs = vec![dir.clone()];
        let cfg = |yaml: &str| -> config::TerminalConfig { serde_yaml_ng::from_str(yaml).unwrap() };

        let (t, w) = resolve_in(&cfg("theme: mine"), true, &dirs);
        assert!(w.is_none());
        assert_eq!(t.bg, Rgb(17, 17, 17));
        assert!(t.palette.contains(&(2, Rgb(0, 255, 0))));
        let (t, _) = resolve_in(&cfg("theme: gh"), true, &dirs);
        assert_eq!(t.bg, Rgb(34, 34, 34));
        // terminal.colors adjusts whatever the theme says
        let (t, _) = resolve_in(&cfg("theme: nord\ncolors: { background: \"#000000\", palette: { 1: \"#ff0000\" } }"), true, &dirs);
        assert_eq!(t.bg, Rgb(0, 0, 0));
        assert!(t.palette.contains(&(1, Rgb(255, 0, 0))) && t.palette.contains(&(2, rgb("#a3be8c"))));
        // unknown theme: default look plus a message
        let (t, w) = resolve_in(&cfg("theme: nonexistent"), true, &dirs);
        assert_eq!(t, paua(true));
        assert!(w.unwrap().contains("nonexistent"));
        // names are file names, never paths
        assert_eq!(from_files("../mine", true, &dirs).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn light_and_dark_can_differ() {
        let cfg: config::TerminalConfig = serde_yaml_ng::from_str("theme: { dark: tokyo-night, light: catppuccin-latte }").unwrap();
        assert_eq!(resolve_in(&cfg, true, &[]).0.bg, rgb("#1a1b26"));
        assert_eq!(resolve_in(&cfg, false, &[]).0.bg, rgb("#eff1f5"));
        // only one given: the other mode keeps the default theme
        let cfg: config::TerminalConfig = serde_yaml_ng::from_str("theme: { dark: nord }").unwrap();
        assert_eq!(resolve_in(&cfg, true, &[]).0.bg, rgb("#2e3440"));
        assert_eq!(resolve_in(&cfg, false, &[]).0, paua(false));
    }
}
