//! `paua setup`: a short first-run wizard. It looks for installed Nerd Fonts, asks which to use for
//! the terminal and for the window decorations (pane headers, status bar), writes a *minimal*
//! `config.yaml` with just those choices, and offers to add the desktop entry (`paua install`).
//!
//! The config deliberately holds only the choices: writing every default would freeze today's
//! defaults into the file so later improvements never reach the user.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::exec;

/// Prefer these when installed (the combination in `examples/config.yaml`).
const PREFERRED_TERMINAL: &str = "VictorMono Nerd Font Propo";
const PREFERRED_UI: &str = "Ubuntu Nerd Font";
/// A family every GTK/Pango system resolves to its default monospace font.
const SYSTEM_MONOSPACE: &str = "Monospace";

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Don't ask: take the suggestions (and keep an existing config).
    pub assume_yes: bool,
    /// Print the config instead of writing it.
    pub print_only: bool,
    /// Offer to run `paua install` at the end (false when `install` itself started the wizard).
    pub offer_install: bool,
    /// Download the missing Nerd Fonts without asking (`--yes` alone never downloads anything).
    pub download_fonts: bool,
}

/// What the wizard needs from the system: which fonts are installed, and downloading fonts.
pub trait FontOps {
    fn installed_families(&self) -> Vec<String>;
    fn install(&self, keys: &[&str], out: &mut dyn Write) -> Result<(), String>;
}

struct RealFonts;

impl FontOps for RealFonts {
    fn installed_families(&self) -> Vec<String> {
        installed_families()
    }

    fn install(&self, keys: &[&str], out: &mut dyn Write) -> Result<(), String> {
        crate::fonts::install_named(keys, out)?;
        crate::fonts::refresh_font_cache(&crate::fonts::fonts_dir());
        Ok(())
    }
}

/// Offer to download the Nerd Fonts Paua suggests, when they aren't installed. Never downloads
/// without the user's say-so: a yes at the prompt, or `--download-fonts`.
fn offer_fonts(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    ops: &dyn FontOps,
    opts: Options,
    families: &mut Vec<String>,
) -> io::Result<()> {
    use crate::fonts;
    let missing: Vec<&fonts::Family> = fonts::SETUP_OFFER
        .iter()
        .filter_map(|k| fonts::find(k))
        .filter(|f| !fonts::is_installed(f, families))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = missing.iter().map(|f| f.key).collect();
    if opts.assume_yes && !opts.download_fonts {
        writeln!(out, "Missing Nerd Fonts: {}. `paua fonts install {}` downloads them.", names.join(", "), names.join(" "))?;
        return Ok(());
    }
    let mb: f32 = missing.iter().map(|f| f.mb).sum();
    writeln!(out, "\nThese Nerd Fonts aren't installed yet:")?;
    for f in &missing {
        writeln!(out, "  {:<11} {} (~{} MB)", f.key, f.label, f.mb)?;
    }
    writeln!(
        out,
        "They are downloaded from the official release at github.com/ryanoasis/nerd-fonts (about {mb:.0} MB),\n\
         checked against its SHA-256 sums and installed in ~/.local/share/fonts/NerdFonts.\n{}",
        crate::fonts::LICENCE_NOTE
    )?;
    if opts.download_fonts || confirm(input, out, "Download and install them now?", true)? {
        match ops.install(&names, out) {
            Ok(()) => *families = ops.installed_families(),
            Err(e) => writeln!(out, "Couldn't install the fonts: {e}\nYou can try again with `paua fonts install {}`.", names.join(" "))?,
        }
    }
    Ok(())
}

/// Installed Nerd Font families, by variant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NerdFonts {
    /// `... Nerd Font Propo`: icons keep their natural width.
    pub propo: Vec<String>,
    /// `... Nerd Font Mono`: every icon squeezed into one cell.
    pub mono: Vec<String>,
    /// `... Nerd Font`: icons may be wider than a cell.
    pub plain: Vec<String>,
}

impl NerdFonts {
    pub fn is_empty(&self) -> bool {
        self.propo.is_empty() && self.mono.is_empty() && self.plain.is_empty()
    }

    /// Candidates for the terminal (a fixed-width grid): Propo first, then Mono.
    fn terminal_candidates(&self) -> Vec<String> {
        self.propo.iter().chain(&self.mono).cloned().collect()
    }

    /// Candidates for the chrome (proportional text): plain first, then Propo.
    fn ui_candidates(&self) -> Vec<String> {
        self.plain.iter().chain(&self.propo).cloned().collect()
    }

    pub fn suggest_terminal(&self) -> Option<String> {
        let c = self.terminal_candidates();
        c.iter().find(|f| *f == PREFERRED_TERMINAL).or(c.first()).cloned()
    }

    pub fn suggest_ui(&self) -> Option<String> {
        let c = self.ui_candidates();
        c.iter().find(|f| *f == PREFERRED_UI).or(c.first()).cloned()
    }
}

/// Every installed font family (from `fc-list`), or nothing if fontconfig isn't available.
pub fn installed_families() -> Vec<String> {
    let mut cmd = Command::new("fc-list");
    cmd.args([":", "family"]);
    match exec::run(cmd, Duration::from_secs(10)) {
        Ok(o) if o.success => parse_families(&o.stdout),
        _ => Vec::new(),
    }
}

/// `fc-list : family` prints one line per face, each `Family[,Alias...]`.
pub fn parse_families(out: &str) -> Vec<String> {
    let mut v: Vec<String> = out
        .lines()
        .flat_map(|l| l.split(','))
        .map(|f| f.trim().to_owned())
        .filter(|f| !f.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

pub fn nerd_fonts(families: &[String]) -> NerdFonts {
    let mut n = NerdFonts::default();
    // "Symbols Nerd Font" is icons only, not a font to write text in.
    for f in families.iter().filter(|f| f.contains("Nerd Font") && !f.starts_with("Symbols")) {
        if f.ends_with(" Propo") {
            n.propo.push(f.clone());
        } else if f.ends_with(" Mono") {
            n.mono.push(f.clone());
        } else if f.ends_with("Nerd Font") {
            n.plain.push(f.clone());
        }
    }
    n
}

/// What the user picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choices {
    /// The terminal font family.
    pub terminal: String,
    /// The font for pane headers and the status bar; `None` = the system UI font.
    pub ui: Option<String>,
}

fn yaml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The minimal config file for the choices.
pub fn config_yaml(c: &Choices) -> String {
    let mut s = String::from(
        "# Paua configuration, written by `paua setup`.\n\
         # Every option is optional and the rest keep their defaults. All options are documented in\n\
         # docs/config/README.md; examples/config.yaml is a complete worked example.\n\n\
         font: # the terminal\n",
    );
    s += &format!("  family: {}\n", yaml_str(&c.terminal));
    if let Some(ui) = &c.ui {
        s += &format!(
            "\npanes:\n  header:\n    font:\n      family: {f}\n\nstatus_bar:\n  font:\n    family: {f}\n",
            f = yaml_str(ui)
        );
    }
    s
}

// ---------------------------------------------------------------------------------------------
// Prompts (generic over the streams so they can be tested with scripted input)
// ---------------------------------------------------------------------------------------------

/// Show a numbered list and read the answer: Enter takes `default`, a number picks that entry, and
/// (with `allow_text`) anything else is taken as a font family name. End of input takes `default`.
fn ask_choice(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    title: &str,
    options: &[(String, Option<String>)],
    default: usize,
    allow_text: bool,
) -> io::Result<Option<String>> {
    writeln!(out, "\n{title}")?;
    for (i, (label, _)) in options.iter().enumerate() {
        let mark = if i == default { " (default)" } else { "" };
        writeln!(out, "  {}. {label}{mark}", i + 1)?;
    }
    let hint = if allow_text { "number, or type a font name" } else { "number" };
    loop {
        write!(out, "Choose [{}] ({hint}): ", default + 1)?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(out)?;
            return Ok(options[default].1.clone());
        }
        let line = line.trim();
        if line.is_empty() {
            return Ok(options[default].1.clone());
        }
        if let Ok(n) = line.parse::<usize>() {
            if (1..=options.len()).contains(&n) {
                return Ok(options[n - 1].1.clone());
            }
            writeln!(out, "Please enter a number from 1 to {}.", options.len())?;
            continue;
        }
        if allow_text {
            return Ok(Some(line.to_owned()));
        }
        writeln!(out, "Please enter a number from 1 to {}.", options.len())?;
    }
}

/// `[Y/n]` question. End of input and Enter take `default_yes`.
fn confirm(input: &mut dyn BufRead, out: &mut dyn Write, question: &str, default_yes: bool) -> io::Result<bool> {
    write!(out, "{question} [{}]: ", if default_yes { "Y/n" } else { "y/N" })?;
    out.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        writeln!(out)?;
        return Ok(default_yes);
    }
    Ok(match line.trim().to_ascii_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    })
}

/// Terminal and UI font choices, with the suggestions as the defaults.
fn choose_fonts(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    fonts: &NerdFonts,
    families: &[String],
    assume_yes: bool,
) -> io::Result<Choices> {
    if fonts.is_empty() {
        writeln!(out, "No Nerd Fonts found (checked with `fc-list`).")?;
        writeln!(
            out,
            "Nerd Fonts add the icons used in pane headers, the status bar and git status. `paua fonts list`\n\
             shows the ones Paua can download for you (`paua fonts install VictorMono Ubuntu`); or get any\n\
             from https://www.nerdfonts.com/font-downloads, unzip into ~/.local/share/fonts and run\n\
             `fc-cache -f`. Then run `paua setup` again. See docs/config/nerd-fonts.md."
        )?;
        let options = vec![
            ("Use the system monospace font for now".to_owned(), Some(SYSTEM_MONOSPACE.to_owned())),
            (format!("Keep the default, {PREFERRED_TERMINAL} (I'll install it)"), Some(PREFERRED_TERMINAL.to_owned())),
        ];
        let terminal = if assume_yes {
            options[0].1.clone()
        } else {
            ask_choice(input, out, "Which font should the terminal use?", &options, 0, true)?
        };
        return Ok(Choices { terminal: terminal.unwrap_or_else(|| SYSTEM_MONOSPACE.to_owned()), ui: None });
    }

    let total = fonts.propo.len() + fonts.mono.len() + fonts.plain.len();
    writeln!(out, "Found {total} Nerd Font famil{}.", if total == 1 { "y" } else { "ies" })?;
    if fonts.suggest_terminal().as_deref() != Some(PREFERRED_TERMINAL) || fonts.suggest_ui().as_deref() != Some(PREFERRED_UI) {
        writeln!(
            out,
            "Tip: `paua fonts list` shows more Nerd Fonts to download (e.g. {PREFERRED_TERMINAL}, {PREFERRED_UI});\n\
             or get any from https://www.nerdfonts.com/font-downloads. Then run `paua setup` again."
        )?;
    }
    let warn_unknown = |out: &mut dyn Write, f: &str| -> io::Result<()> {
        if !families.iter().any(|x| x == f) {
            writeln!(out, "Note: {f:?} isn't among the installed fonts; it will fall back to another font.")?;
        }
        Ok(())
    };

    // Terminal
    let candidates = fonts.terminal_candidates();
    let suggested = fonts.suggest_terminal();
    let mut options: Vec<(String, Option<String>)> = candidates
        .iter()
        .take(15)
        .map(|f| {
            let note = if f.ends_with(" Propo") { "  (recommended for terminals)" } else { "" };
            (format!("{f}{note}"), Some(f.clone()))
        })
        .collect();
    options.push(("System monospace font (no icons)".into(), Some(SYSTEM_MONOSPACE.into())));
    let default = suggested
        .as_ref()
        .and_then(|s| options.iter().position(|(_, v)| v.as_ref() == Some(s)))
        .unwrap_or(0);
    let terminal = if assume_yes {
        options[default].1.clone()
    } else {
        ask_choice(input, out, "Which font should the terminal use?", &options, default, true)?
    };
    let terminal = terminal.unwrap_or_else(|| SYSTEM_MONOSPACE.to_owned());
    warn_unknown(out, &terminal)?;

    // Window decorations
    let ui_candidates = fonts.ui_candidates();
    let mut options: Vec<(String, Option<String>)> = ui_candidates
        .iter()
        .take(15)
        .map(|f| (f.clone(), Some(f.clone())))
        .collect();
    options.push(("System UI font (no icons)".into(), None));
    let default = fonts
        .suggest_ui()
        .and_then(|s| options.iter().position(|(_, v)| v.as_ref() == Some(&s)))
        .unwrap_or(options.len() - 1);
    let ui = if assume_yes {
        options[default].1.clone()
    } else {
        ask_choice(
            input,
            out,
            "Which font should the pane headers and status bar use?",
            &options,
            default,
            true,
        )?
    };
    if let Some(f) = &ui {
        warn_unknown(out, f)?;
    }
    Ok(Choices { terminal, ui })
}

/// The whole wizard. `families` are the installed fonts, `config_path` where the config goes, and
/// `install` runs the desktop-entry installation.
pub fn run_with(
    opts: Options,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    ops: &dyn FontOps,
    config_path: &Path,
    install: &dyn Fn() -> i32,
) -> io::Result<i32> {
    writeln!(out, "Paua setup")?;
    let mut families = ops.installed_families();
    if !opts.print_only {
        offer_fonts(input, out, ops, opts, &mut families)?;
    }
    let fonts = nerd_fonts(&families);
    let choices = choose_fonts(input, out, &fonts, &families, opts.assume_yes)?;
    let yaml = config_yaml(&choices);

    if opts.print_only {
        writeln!(out, "\n--- config.yaml ---\n{yaml}")?;
        return Ok(0);
    }

    // An existing config is never overwritten without a backup.
    let existing = std::fs::read_to_string(config_path).is_ok_and(|t| !t.trim().is_empty());
    let write = if existing {
        writeln!(out, "\nA config already exists at {}.", config_path.display())?;
        if opts.assume_yes {
            writeln!(out, "Keeping it. Run `paua setup` without --yes to replace it.")?;
            false
        } else {
            let replace = confirm(input, out, "Back it up and write the new one?", false)?;
            if replace {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let backup = config_path.with_extension(format!("yaml.bak.{stamp}"));
                std::fs::rename(config_path, &backup)?;
                writeln!(out, "Backed up to {}.", backup.display())?;
            }
            replace
        }
    } else {
        true
    };
    if write {
        if let Some(dir) = config_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(config_path, &yaml)?;
        writeln!(out, "\nWrote {}", config_path.display())?;
        writeln!(out, "The terminal uses {:?}.", choices.terminal)?;
        if let Some(ui) = &choices.ui {
            writeln!(out, "Pane headers and the status bar use {ui:?}.")?;
        }
    }

    if opts.offer_install {
        writeln!(out)?;
        if opts.assume_yes || confirm(input, out, "Add Paua to the GNOME overview and dock (`paua install`)?", true)? {
            let code = install();
            if code != 0 {
                return Ok(code);
            }
        }
    }
    writeln!(out, "\nAll set. Run `paua` to start. Every option is documented in docs/config/README.md.")?;
    Ok(0)
}

/// `paua setup` on the real terminal.
pub fn run(opts: Options) -> i32 {
    let stdin = io::stdin();
    if !opts.assume_yes && !stdin.is_terminal() {
        eprintln!("paua setup is interactive. Use --yes to accept the suggestions, or --print to see the config.");
        return 2;
    }
    let result = run_with(
        opts,
        &mut stdin.lock(),
        &mut io::stdout(),
        &RealFonts,
        &crate::config::user_config_path(),
        &|| crate::install::install(crate::install::InstallOptions { assume_yes: true, no_config: true }),
    );
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("paua: setup failed: {e}");
            1
        }
    }
}

/// After `paua install`: if there is no config yet, offer the wizard (only when attached to a
/// terminal, or with `--yes`).
pub fn offer_after_install(assume_yes: bool) {
    if crate::config::user_config_path().exists() {
        return;
    }
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    if !assume_yes && !interactive {
        return;
    }
    let run_it = assume_yes || {
        println!();
        confirm(&mut io::stdin().lock(), &mut io::stdout(), "No config file yet. Run `paua setup` to pick fonts?", true)
            .unwrap_or(false)
    };
    if run_it {
        let _ = run(Options { assume_yes, print_only: false, offer_install: false, download_fonts: false });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fams(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    const INSTALLED: &[&str] = &[
        "DejaVu Sans",
        "Ubuntu Nerd Font",
        "Ubuntu Nerd Font Mono",
        "Ubuntu Nerd Font Propo",
        "VictorMono Nerd Font",
        "VictorMono Nerd Font Mono",
        "VictorMono Nerd Font Propo",
        "Symbols Nerd Font Mono",
    ];

    #[test]
    fn fc_list_output_is_parsed() {
        let out = "DejaVu Sans,DejaVu Sans Book\nUbuntu Nerd Font,Ubuntu Nerd Font Medium\nDejaVu Sans\n\n";
        assert_eq!(parse_families(out), fams(&["DejaVu Sans", "DejaVu Sans Book", "Ubuntu Nerd Font", "Ubuntu Nerd Font Medium"]));
    }

    #[test]
    fn nerd_fonts_are_classified_and_suggested() {
        let n = nerd_fonts(&fams(INSTALLED));
        assert_eq!(n.propo, fams(&["Ubuntu Nerd Font Propo", "VictorMono Nerd Font Propo"]));
        assert_eq!(n.mono, fams(&["Ubuntu Nerd Font Mono", "VictorMono Nerd Font Mono"]));
        assert_eq!(n.plain, fams(&["Ubuntu Nerd Font", "VictorMono Nerd Font"]));
        assert_eq!(n.suggest_terminal().as_deref(), Some("VictorMono Nerd Font Propo"));
        assert_eq!(n.suggest_ui().as_deref(), Some("Ubuntu Nerd Font"));
        // Without the preferred ones, the first suitable family is suggested.
        let other = nerd_fonts(&fams(&["Hack Nerd Font Mono", "Hack Nerd Font Propo", "Hack Nerd Font"]));
        assert_eq!(other.suggest_terminal().as_deref(), Some("Hack Nerd Font Propo"));
        assert_eq!(other.suggest_ui().as_deref(), Some("Hack Nerd Font"));
        assert!(nerd_fonts(&fams(&["DejaVu Sans"])).is_empty());
    }

    #[test]
    fn the_config_is_valid_and_minimal() {
        let yaml = config_yaml(&Choices { terminal: "My \"Font\" Propo".into(), ui: Some("Ubuntu Nerd Font".into()) });
        let c: crate::config::UserConfig = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(c.font.family, "My \"Font\" Propo");
        assert_eq!(c.panes.header.font.family.as_deref(), Some("Ubuntu Nerd Font"));
        assert_eq!(c.status_bar.font.family.as_deref(), Some("Ubuntu Nerd Font"));
        let plain = config_yaml(&Choices { terminal: "Monospace".into(), ui: None });
        let c: crate::config::UserConfig = serde_yaml_ng::from_str(&plain).unwrap();
        assert_eq!(c.font.family, "Monospace");
        assert!(c.status_bar.font.family.is_none() && !plain.contains("status_bar"));
    }

    /// Fonts without touching the system: reports `families`, and after an install reports
    /// `after_install` (and records what was asked for).
    struct FakeFonts {
        families: Vec<String>,
        after_install: Vec<String>,
        installed: std::cell::RefCell<Vec<String>>,
        fail: bool,
    }

    impl FakeFonts {
        fn new(families: &[&str]) -> Self {
            Self { families: fams(families), after_install: fams(families), installed: Default::default(), fail: false }
        }
    }

    impl FontOps for FakeFonts {
        fn installed_families(&self) -> Vec<String> {
            if self.installed.borrow().is_empty() { self.families.clone() } else { self.after_install.clone() }
        }

        fn install(&self, keys: &[&str], _out: &mut dyn Write) -> Result<(), String> {
            if self.fail {
                return Err("no network".into());
            }
            self.installed.borrow_mut().extend(keys.iter().map(|k| (*k).to_owned()));
            Ok(())
        }
    }

    fn run_script(input: &str, families: &[&str], path: &Path, opts: Options) -> (i32, String, i32) {
        run_script_with(input, &FakeFonts::new(families), path, opts)
    }

    fn run_script_with(input: &str, fonts: &FakeFonts, path: &Path, opts: Options) -> (i32, String, i32) {
        let installs = std::cell::Cell::new(0);
        let mut out = Vec::new();
        let code = run_with(opts, &mut input.as_bytes(), &mut out, fonts, path, &|| {
            installs.set(installs.get() + 1);
            0
        })
        .unwrap();
        (code, String::from_utf8(out).unwrap(), installs.get())
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("paua-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn enter_takes_the_suggestions_and_installs() {
        let dir = temp("enter");
        let path = dir.join("paua/config.yaml");
        let opts = Options { offer_install: true, ..Default::default() };
        let (code, out, installs) = run_script("\n\n\n", INSTALLED, &path, opts);
        assert_eq!(code, 0, "{out}");
        assert_eq!(installs, 1);
        let c: crate::config::UserConfig = serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(c.font.family, "VictorMono Nerd Font Propo");
        assert_eq!(c.status_bar.font.family.as_deref(), Some("Ubuntu Nerd Font"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn numbers_and_typed_names_are_accepted() {
        let dir = temp("pick");
        let path = dir.join("config.yaml");
        // terminal: option 1 (Ubuntu Nerd Font Propo); chrome: a typed name; no install
        let (code, out, installs) = run_script("1\nCascadia Code NF\nn\n", INSTALLED, &path, Options { offer_install: true, ..Default::default() });
        assert_eq!((code, installs), (0, 0), "{out}");
        assert!(out.contains("isn't among the installed fonts"));
        let c: crate::config::UserConfig = serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(c.font.family, "Ubuntu Nerd Font Propo");
        assert_eq!(c.panes.header.font.family.as_deref(), Some("Cascadia Code NF"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_nerd_fonts_it_explains_and_uses_system_fonts() {
        let dir = temp("none");
        let path = dir.join("config.yaml");
        let (code, out, _) = run_script("n\n\n", &["DejaVu Sans"], &path, Options::default()); // decline the download
        assert_eq!(code, 0);
        assert!(out.contains("No Nerd Fonts found") && out.contains("nerd-fonts.md"));
        let c: crate::config::UserConfig = serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(c.font.family, "Monospace");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_config_is_kept_or_backed_up() {
        let dir = temp("existing");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "watch: false\n").unwrap();
        // --yes keeps it
        let (_, out, _) = run_script("", INSTALLED, &path, Options { assume_yes: true, ..Default::default() });
        assert!(out.contains("Keeping it"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "watch: false\n");
        // Enter on the "replace?" question (default no) keeps it too
        run_script("\n\n\n", INSTALLED, &path, Options::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "watch: false\n");
        // "y" backs up and replaces
        let (_, out, _) = run_script("\n\ny\n", INSTALLED, &path, Options::default());
        assert!(out.contains("Backed up to"), "{out}");
        assert!(std::fs::read_to_string(&path).unwrap().contains("VictorMono Nerd Font Propo"));
        let backups: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".bak."))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read_to_string(backups[0].path()).unwrap(), "watch: false\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn print_mode_and_yes_mode_do_not_prompt_or_write_as_asked() {
        let dir = temp("print");
        let path = dir.join("config.yaml");
        let (code, out, _) = run_script("", INSTALLED, &path, Options { assume_yes: true, print_only: true, ..Default::default() });
        assert_eq!(code, 0);
        assert!(out.contains("--- config.yaml ---") && out.contains("VictorMono Nerd Font Propo"));
        assert!(!path.exists());
        let (_, _, installs) = run_script("", INSTALLED, &path, Options { assume_yes: true, offer_install: true, ..Default::default() });
        assert!(path.exists() && installs == 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_fonts_are_offered_for_download_and_used_afterwards() {
        let dir = temp("offer");
        let path = dir.join("config.yaml");
        let mut fonts = FakeFonts::new(&["DejaVu Sans"]);
        fonts.after_install = fams(INSTALLED);
        // Enter takes the default (download); then Enter for each font question.
        let (code, out, _) = run_script_with("\n\n\n", &fonts, &path, Options::default());
        assert_eq!(code, 0, "{out}");
        assert_eq!(*fonts.installed.borrow(), vec!["VictorMono", "Ubuntu", "Symbols"]);
        assert!(out.contains("github.com/ryanoasis/nerd-fonts") && out.contains("SHA-256") && out.contains("licence"));
        let c: crate::config::UserConfig = serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(c.font.family, "VictorMono Nerd Font Propo"); // the freshly installed font is suggested
        assert_eq!(c.status_bar.font.family.as_deref(), Some("Ubuntu Nerd Font"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_downloaded_without_consent() {
        let dir = temp("consent");
        let path = dir.join("config.yaml");
        // declined
        let fonts = FakeFonts::new(&["DejaVu Sans"]);
        run_script_with("n\n\n", &fonts, &path, Options::default());
        assert!(fonts.installed.borrow().is_empty());
        // --yes alone never downloads, but says how
        let fonts = FakeFonts::new(&["DejaVu Sans"]);
        let (_, out, _) = run_script_with("", &fonts, &path, Options { assume_yes: true, print_only: true, ..Default::default() });
        assert!(fonts.installed.borrow().is_empty());
        let (_, out2, _) = run_script_with("", &fonts, &path, Options { assume_yes: true, ..Default::default() });
        assert!(fonts.installed.borrow().is_empty() && out2.contains("paua fonts install"), "{out} {out2}");
        // --download-fonts is the explicit opt-in for scripts
        let fonts = FakeFonts::new(&["DejaVu Sans"]);
        run_script_with("", &fonts, &path, Options { assume_yes: true, download_fonts: true, print_only: false, offer_install: false });
        assert_eq!(fonts.installed.borrow().len(), 3);
        // when everything is present, nothing is offered
        let fonts = FakeFonts::new(INSTALLED);
        let (_, out, _) = run_script_with("\n\n", &fonts, &temp("present").join("c.yaml"), Options::default());
        assert!(!out.contains("aren't installed yet"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_download_falls_back_gracefully() {
        let dir = temp("fail");
        let path = dir.join("config.yaml");
        let mut fonts = FakeFonts::new(&["DejaVu Sans"]);
        fonts.fail = true;
        let (code, out, _) = run_script_with("\n\n", &fonts, &path, Options::default());
        assert_eq!(code, 0);
        assert!(out.contains("Couldn't install the fonts: no network") && out.contains("No Nerd Fonts found"));
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
