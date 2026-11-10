//! User config (`~/.config/paua/config.yaml`) and per-directory config (`.paua.yaml`).
//!
//! Every field is optional; missing fields fall back to defaults. Unknown fields are ignored so
//! older/newer config files keep loading.

use std::path::{Path, PathBuf};

use gtk::glib;
use serde::{Deserialize, Deserializer, de::Error as _};

use crate::terminal::Rgb;

/// Built-in window (frame) colours, used when no config sets one.
pub const DEFAULT_WINDOW_DARK: Rgb = Rgb(0x4a, 0x2f, 0x80);
pub const DEFAULT_WINDOW_LIGHT: Rgb = Rgb(0x7a, 0x56, 0xc2);

pub const DIR_CONFIG_NAME: &str = ".paua.yaml";

// ---------------------------------------------------------------------------------------------
// Value types
// ---------------------------------------------------------------------------------------------

/// A length: `1100`, `1100px` (pixels) or `80%` (of the monitor).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dim {
    Px(f64),
    Percent(f64),
}

impl Dim {
    pub fn resolve(self, total: i32) -> i32 {
        match self {
            Dim::Px(p) => p.round() as i32,
            Dim::Percent(p) => (f64::from(total) * p / 100.0).round() as i32,
        }
    }

    fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (num, pct) = match (s.strip_suffix('%'), s.strip_suffix("px")) {
            (Some(n), _) => (n, true),
            (_, Some(n)) => (n, false),
            _ => (s, false),
        };
        let v: f64 = num
            .trim()
            .parse()
            .map_err(|_| format!("invalid size {s:?} (use e.g. 1100, 1100px or 80%)"))?;
        if v <= 0.0 {
            return Err(format!("size must be positive, got {s:?}"));
        }
        Ok(if pct { Dim::Percent(v) } else { Dim::Px(v) })
    }
}

impl<'de> Deserialize<'de> for Dim {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(f64),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Num(n) if n > 0.0 => Ok(Dim::Px(n)),
            Raw::Num(n) => Err(D::Error::custom(format!("size must be positive, got {n}"))),
            Raw::Str(s) => Dim::parse(&s).map_err(D::Error::custom),
        }
    }
}

/// `#rrggbb` / `#rgb` (the `#` is optional).
fn parse_hex(s: &str) -> Result<Rgb, String> {
    let h = s.trim().trim_start_matches('#');
    let bad = || format!("invalid colour {s:?} (use #rrggbb)");
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| bad());
    match h.len() {
        6 if h.is_ascii() => Ok(Rgb(byte(0)?, byte(2)?, byte(4)?)),
        3 if h.is_ascii() => {
            let n = |i: usize| u8::from_str_radix(&h[i..=i], 16).map(|v| v * 17).map_err(|_| bad());
            Ok(Rgb(n(0)?, n(1)?, n(2)?))
        }
        _ => Err(bad()),
    }
}

/// `#rrggbb`, `#rrggbbaa` (or the 3/4 digit short forms) -> colour + alpha (0..1).
fn parse_rgba(s: &str) -> Result<Tint, String> {
    let h = s.trim().trim_start_matches('#');
    let bad = || format!("invalid colour {s:?} (use #rrggbb or #rrggbbaa)");
    if !h.is_ascii() {
        return Err(bad());
    }
    let pair = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| bad());
    let nib = |i: usize| u8::from_str_radix(&h[i..=i], 16).map(|v| v * 17).map_err(|_| bad());
    let (r, g, b, a) = match h.len() {
        6 => (pair(0)?, pair(2)?, pair(4)?, 255),
        8 => (pair(0)?, pair(2)?, pair(4)?, pair(6)?),
        3 => (nib(0)?, nib(1)?, nib(2)?, 255),
        4 => (nib(0)?, nib(1)?, nib(2)?, nib(3)?),
        _ => return Err(bad()),
    };
    Ok(Tint { rgb: Rgb(r, g, b), alpha: f32::from(a) / 255.0 })
}

/// A colour with transparency, e.g. a tint laid over a background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tint {
    pub rgb: Rgb,
    /// 0 = invisible, 1 = opaque.
    pub alpha: f32,
}

impl Tint {
    pub fn css(&self) -> String {
        format!("rgba({}, {}, {}, {:.3})", self.rgb.0, self.rgb.1, self.rgb.2, self.alpha)
    }
}

/// Parse `"#rrggbb"` or `{ dark: ..., light: ... }`, using `parse` for each colour string.
fn deserialize_themed<'de, D: Deserializer<'de>, T>(
    d: D,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<(Option<T>, Option<T>), D::Error>
where
    T: Copy,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        One(String),
        Both {
            #[serde(default)]
            dark: Option<String>,
            #[serde(default)]
            light: Option<String>,
        },
    }
    let parse = |s: &str| parse(s).map_err(D::Error::custom);
    match Raw::deserialize(d)? {
        Raw::One(s) => {
            let c = parse(&s)?;
            Ok((Some(c), Some(c)))
        }
        Raw::Both { dark, light } => {
            Ok((dark.as_deref().map(parse).transpose()?, light.as_deref().map(parse).transpose()?))
        }
    }
}

/// A colour, optionally different for light and dark mode:
/// `"#4a2f80"` or `{ dark: "#4a2f80", light: "#7a56c2" }`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ThemedColor {
    pub dark: Option<Rgb>,
    pub light: Option<Rgb>,
}

impl ThemedColor {
    /// The colour for the given mode, falling back to the other mode's colour if unset.
    pub fn resolve(&self, dark: bool) -> Option<Rgb> {
        if dark {
            self.dark.or(self.light)
        } else {
            self.light.or(self.dark)
        }
    }
}

impl<'de> Deserialize<'de> for ThemedColor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (dark, light) = deserialize_themed(d, parse_hex)?;
        Ok(ThemedColor { dark, light })
    }
}

/// Like [`ThemedColor`] but each colour may carry alpha (`#rrggbbaa`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ThemedTint {
    pub dark: Option<Tint>,
    pub light: Option<Tint>,
}

impl ThemedTint {
    pub fn resolve(&self, dark: bool) -> Option<Tint> {
        if dark {
            self.dark.or(self.light)
        } else {
            self.light.or(self.dark)
        }
    }
}

impl<'de> Deserialize<'de> for ThemedTint {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (dark, light) = deserialize_themed(d, parse_rgba)?;
        Ok(ThemedTint { dark, light })
    }
}

/// A font weight: `100`..`1000` or a name (`thin`, `light`, `normal`, `medium`, `semibold`, `bold`,
/// `extrabold`, `black`...). Variable fonts honour any number; static fonts snap to the nearest
/// installed weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontWeight(pub u32);

impl FontWeight {
    fn parse(s: &str) -> Result<Self, String> {
        let name = s.trim().to_ascii_lowercase().replace(['-', '_', ' '], "");
        let n = match name.as_str() {
            "thin" | "hairline" => 100,
            "extralight" | "ultralight" => 200,
            "light" => 300,
            "semilight" => 350,
            "book" => 380,
            "normal" | "regular" => 400,
            "medium" => 500,
            "semibold" | "demibold" => 600,
            "bold" => 700,
            "extrabold" | "ultrabold" => 800,
            "heavy" | "black" => 900,
            "ultraheavy" | "extrablack" => 1000,
            other => other
                .parse::<u32>()
                .map_err(|_| format!("invalid font weight {s:?} (use 100-1000 or a name like light, medium, bold)"))?,
        };
        Self::checked(n)
    }

    fn checked(n: u32) -> Result<Self, String> {
        if (100..=1000).contains(&n) {
            Ok(Self(n))
        } else {
            Err(format!("font weight must be 100-1000, got {n}"))
        }
    }
}

impl<'de> Deserialize<'de> for FontWeight {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(u32),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Num(n) => Self::checked(n).map_err(D::Error::custom),
            Raw::Str(s) => Self::parse(&s).map_err(D::Error::custom),
        }
    }
}

impl<'de> Deserialize<'de> for crate::terminal::Padding {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            All(f32),
            Sides {
                x: Option<f32>,
                y: Option<f32>,
                top: Option<f32>,
                right: Option<f32>,
                bottom: Option<f32>,
                left: Option<f32>,
            },
        }
        let p = match Raw::deserialize(d)? {
            Raw::All(v) => Self { top: v, right: v, bottom: v, left: v },
            Raw::Sides { x, y, top, right, bottom, left } => Self {
                top: top.or(y).unwrap_or(0.0),
                right: right.or(x).unwrap_or(0.0),
                bottom: bottom.or(y).unwrap_or(0.0),
                left: left.or(x).unwrap_or(0.0),
            },
        };
        for v in [p.top, p.right, p.bottom, p.left] {
            if !(0.0..=200.0).contains(&v) {
                return Err(D::Error::custom(format!("padding must be 0-200 px, got {v}")));
            }
        }
        Ok(p)
    }
}

// ---------------------------------------------------------------------------------------------
// User config
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct UserConfig {
    /// Reload this file (and the active `.paua.yaml`) when it changes on disk.
    pub watch: bool,
    pub font: FontConfig,
    pub window: WindowConfig,
    pub panes: PanesConfig,
    pub terminal: TerminalConfig,
    /// Detecting ssh / sudo / containers / other machines in a pane's shell.
    pub session: SessionConfig,
    /// Find in output (the title-bar search box).
    pub search: SearchConfig,
    /// The command palette (the same box, `>` mode).
    pub palette: PaletteConfig,
    /// The bar at the bottom of each window.
    pub status_bar: StatusBarConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct PaletteConfig {
    /// Most results shown at once.
    pub max_results: usize,
    /// Offer `cd` to recently visited directories.
    pub directories: bool,
    /// How many recent directories to remember (this run only).
    pub recent_dirs: usize,
    /// Offer `ssh <host>` for the hosts in `~/.ssh/config`.
    pub ssh_hosts: bool,
    /// Your own entries: each types `run` (and Enter) into the focused pane.
    pub commands: Vec<CustomCommand>,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
pub struct CustomCommand {
    pub title: String,
    /// Typed into the focused pane followed by Enter.
    pub run: String,
    /// Shown dimmed on the right (default: the command itself).
    #[serde(default)]
    pub subtitle: Option<String>,
}

impl Default for PaletteConfig {
    fn default() -> Self {
        Self { max_results: 40, directories: true, recent_dirs: 25, ssh_hosts: true, commands: Vec::new() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct SearchConfig {
    /// `smart` (case-insensitive unless the query has an uppercase letter), `sensitive` or
    /// `insensitive`. The box's "Aa" toggle forces `sensitive`.
    pub case: crate::terminal::CaseMode,
    /// Start with the regular-expression toggle on.
    pub regex: bool,
    /// Highlight colours for matches and for the current match (`#rrggbbaa`, or `{ dark, light }`).
    pub match_color: Option<ThemedTint>,
    pub current_color: Option<ThemedTint>,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self { case: crate::terminal::CaseMode::Smart, regex: false, match_color: None, current_color: None }
    }
}

impl SearchConfig {
    pub fn style(&self, dark: bool) -> crate::terminal::FindStyle {
        let d = crate::terminal::FindStyle::default();
        crate::terminal::FindStyle {
            matched: self.match_color.and_then(|c| c.resolve(dark)).unwrap_or(d.matched),
            current: self.current_color.and_then(|c| c.resolve(dark)).unwrap_or(d.current),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct SessionConfig {
    /// Look at the process running in each pane (Linux `/proc`) to detect sessions.
    pub detect: bool,
    /// How often to look, in milliseconds.
    pub poll_ms: u32,
    /// Show a small badge for each detected session in the pane header.
    pub badges: bool,
    /// Per-kind settings. Kinds: `root`, `sudo`, `ssh`, `container`, `remote` (another host
    /// reported by the shell, or mosh/telnet), `multiplexer` (tmux/screen/zellij).
    pub root: SessionKindConfig,
    pub sudo: SessionKindConfig,
    pub ssh: SessionKindConfig,
    pub container: SessionKindConfig,
    pub remote: SessionKindConfig,
    pub multiplexer: SessionKindConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct SessionKindConfig {
    /// Detect and show this kind at all.
    pub enabled: bool,
    /// Badge text. Placeholders: `{tool}` (ssh, sudo, docker...), `{target}` (`user@host`, the
    /// container, the user sudo switches to), `{host}`, `{user}`. Surrounding spaces are trimmed.
    pub label: String,
    /// Prepended to the label, e.g. a Nerd Font icon.
    pub icon: String,
    /// Badge colours (override the built-in ones): `#rrggbb[aa]` or `{ dark, light }`.
    pub background: Option<ThemedTint>,
    pub color: Option<ThemedTint>,
    /// Tint and text colour of the pane's whole header line while this session is the pane's most
    /// important one (priority: root, sudo, ssh, container, remote, multiplexer). Built-in subtle
    /// tints exist for most kinds; these override them. `#rrggbbaa` for translucency.
    pub header_background: Option<ThemedTint>,
    pub header_color: Option<ThemedTint>,
    /// Recolour the whole window frame while this session is active in the window's focused pane.
    pub window_color: Option<ThemedColor>,
}

impl SessionKindConfig {
    fn with_label(label: &str) -> Self {
        Self {
            enabled: true,
            label: label.to_owned(),
            icon: String::new(),
            background: None,
            color: None,
            header_background: None,
            header_color: None,
            window_color: None,
        }
    }
}

impl Default for SessionKindConfig {
    fn default() -> Self {
        Self::with_label("{tool} {target}")
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            detect: true,
            poll_ms: 1000,
            badges: true,
            root: SessionKindConfig::with_label("root"),
            sudo: SessionKindConfig::with_label("{tool} {target}"),
            ssh: SessionKindConfig::with_label("ssh {target}"),
            container: SessionKindConfig::with_label("{tool} {target}"),
            remote: SessionKindConfig::with_label("remote {target}"),
            multiplexer: SessionKindConfig::with_label("{tool}"),
        }
    }
}

impl SessionConfig {
    pub fn kind(&self, kind: crate::session::Kind) -> &SessionKindConfig {
        use crate::session::Kind;
        match kind {
            Kind::Root => &self.root,
            Kind::Sudo => &self.sudo,
            Kind::Ssh => &self.ssh,
            Kind::Container => &self.container,
            Kind::Remote => &self.remote,
            Kind::Multiplexer => &self.multiplexer,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    /// Size in points.
    pub size: f32,
    /// OpenType features, e.g. `"-calt"` to turn ligatures off. Unset = font defaults (on).
    pub features: Option<String>,
    /// Regular text weight: 100-1000 or a name (`light`, `medium`...). Unset = normal (400).
    pub weight: Option<FontWeight>,
    /// Weight of bold text. Unset = bold (700), or 200 heavier than `weight` if that is higher.
    pub bold_weight: Option<FontWeight>,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct WindowConfig {
    /// Startup only (not applied on live reload).
    pub maximized: bool,
    /// Startup only. `1100`, `1100px` or `80%` of the monitor.
    pub width: Dim,
    pub height: Dim,
    /// Default window (frame) colour; a directory's `.paua.yaml` can override it.
    pub color: Option<ThemedColor>,
    /// How much the frame (title bar, status bar, borders) darkens while the window is unfocused:
    /// 0 = not at all, 1 = black.
    pub inactive_dim: f32,
    /// Also take a directory's window colour from the nearest VS Code `.vscode/settings.json`
    /// (Peacock and hand-set colours). A `.paua.yaml` colour takes precedence. Read when you enter
    /// the directory (and when the pane is focused); the file is not watched.
    pub vscode_colors: bool,
    /// Which settings to try, in order. A key is looked up at the top level of the file (like
    /// `peacock.color`) and inside `workbench.colorCustomizations` (like `titleBar.activeBackground`).
    pub vscode_color_keys: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct PanesConfig {
    /// How panes other than the focused one look (only when there is more than one pane).
    pub inactive: InactiveConfig,
    /// The line above each terminal: text, font, alignment.
    pub header: HeaderConfig,
    /// Space between a pane's edge and its text, in pixels (the pane background fills it).
    /// A number for all sides, or `{ top, right, bottom, left }` / `{ x, y }`. Default 0.
    pub padding: crate::terminal::Padding,
}

/// Where the command timer chip sits in the pane header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimerPosition {
    /// Before the text (and any badges).
    #[serde(alias = "start")]
    Left,
    /// Next to the buttons.
    #[default]
    #[serde(alias = "end")]
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeaderAlign {
    #[serde(alias = "left")]
    Start,
    #[default]
    Center,
    #[serde(alias = "right")]
    End,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct HeaderConfig {
    /// Text template. Placeholders: `{path}` (working directory, after `path_replacements`),
    /// `{dir}` (its last component), `{title}` (title set by the program, OSC 0/2), `{host}`,
    /// `{user}`. `{a|b}` uses the first non-empty one, e.g. `{title|path}`.
    pub template: String,
    /// `start` (alias `left`), `center` or `end` (alias `right`).
    pub align: HeaderAlign,
    pub font: HeaderFont,
    /// Rewrite the start of the path. First match wins; matching is by whole path component.
    /// `from` may use `~` or `$HOME`. Plain prefixes, not regular expressions.
    pub path_replacements: Vec<PathReplacement>,
    /// When the text is too wide for the pane, shorten the path from its middle, whole segments
    /// at a time (`~/a/b/c/d` -> `~/a/…/d`).
    pub trim_path: bool,
    /// What replaces the removed segments.
    pub path_ellipsis: String,
    /// Header background, laid over the pane background: `"#rrggbb"`, `"#rrggbbaa"` (translucent
    /// tint) or `{ dark: ..., light: ... }`. Unset = no tint.
    pub background: Option<ThemedTint>,
    /// Header text and icon colour (same formats). Unset = the normal text colour, dimmed.
    pub color: Option<ThemedTint>,
    /// Icon names (any icon in the theme, or ours: `pop-out-symbolic`, `pop-out-pip-symbolic`).
    pub pop_out_icon: String,
    pub close_icon: String,
    pub copy_icon: String,
    /// A running-command timer in the header.
    pub command_timer: CommandTimerConfig,
    /// Flash the header when the program rings the terminal bell (BEL).
    pub bell: BellConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct BellConfig {
    pub enabled: bool,
    /// Flash colour (`#rrggbbaa`, or `{ dark, light }`). Unset = a soft white (dark) / black (light).
    pub color: Option<ThemedTint>,
    /// How long the flash takes to fade out (`400ms`, `1s`...).
    pub duration: Span,
}

impl Default for BellConfig {
    fn default() -> Self {
        Self { enabled: true, color: None, duration: Span(std::time::Duration::from_millis(500)) }
    }
}

/// A duration written as seconds (`2`, `1.5`) or with a unit: `500ms`, `2s`, `1.5m`, `1h`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span(pub std::time::Duration);

impl Span {
    fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim().to_ascii_lowercase();
        let (num, unit) = if let Some(n) = s.strip_suffix("ms") {
            (n, 0.001)
        } else if let Some(n) = s.strip_suffix('s') {
            (n, 1.0)
        } else if let Some(n) = s.strip_suffix('m') {
            (n, 60.0)
        } else if let Some(n) = s.strip_suffix('h') {
            (n, 3600.0)
        } else {
            (s.as_str(), 1.0)
        };
        let v: f64 = num
            .trim()
            .parse()
            .map_err(|_| format!("invalid duration {s:?} (use e.g. 2, 500ms, 2s, 1.5m, 1h)"))?;
        if !v.is_finite() || v < 0.0 || v * unit > 86_400.0 {
            return Err(format!("duration out of range: {s:?}"));
        }
        Ok(Self(std::time::Duration::from_secs_f64(v * unit)))
    }
}

impl<'de> Deserialize<'de> for Span {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(f64),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Num(n) => Self::parse(&n.to_string()).map_err(D::Error::custom),
            Raw::Str(s) => Self::parse(&s).map_err(D::Error::custom),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct CommandTimerConfig {
    pub enabled: bool,
    /// `left` (before the text) or `right` (next to the buttons).
    pub position: TimerPosition,
    /// Show the timer once a command has run this long (`2s`, `500ms`, `1m`...).
    pub after: Span,
    /// After a long command ends, keep showing "took 12s" for this long (`0` = don't).
    pub linger: Span,
    /// Don't time full-screen programs (vim, less, htop...).
    pub ignore_alt_screen: bool,
    /// Program names that are never timed.
    pub ignore: Vec<String>,
    /// Add the exit status to the "took" message when the shell reports a failure (needs shell
    /// integration, e.g. nushell).
    pub show_exit_status: bool,
    /// Put before the text, e.g. a Nerd Font stopwatch glyph.
    pub icon: String,
    /// Extra space (px) after the icon. Nerd Font glyphs often overflow their cell; raise this if
    /// the icon touches the time.
    pub icon_gap: f32,
    /// Chip colours (`#rrggbbaa`, or `{ dark, light }`): while running, and after it finished.
    pub background: Option<ThemedTint>,
    pub color: Option<ThemedTint>,
    pub done_background: Option<ThemedTint>,
    pub done_color: Option<ThemedTint>,
    /// ...and after a command that reported a non-zero exit status.
    pub failed_background: Option<ThemedTint>,
    pub failed_color: Option<ThemedTint>,
}

impl Default for CommandTimerConfig {
    fn default() -> Self {
        let d = crate::cmdtimer::TimerConfig::default();
        Self {
            enabled: d.enabled,
            position: TimerPosition::Right,
            after: Span(d.threshold),
            linger: Span(d.linger),
            ignore_alt_screen: d.ignore_alt_screen,
            ignore: d.ignore,
            show_exit_status: d.show_exit_status,
            icon: String::new(),
            icon_gap: 5.0,
            background: None,
            color: None,
            done_background: None,
            done_color: None,
            failed_background: None,
            failed_color: None,
        }
    }
}

impl CommandTimerConfig {
    pub fn settings(&self) -> crate::cmdtimer::TimerConfig {
        crate::cmdtimer::TimerConfig {
            enabled: self.enabled,
            threshold: self.after.0,
            linger: self.linger.0,
            ignore_alt_screen: self.ignore_alt_screen,
            ignore: self.ignore.clone(),
            show_exit_status: self.show_exit_status,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct HeaderFont {
    /// Unset = the terminal font family (so Nerd Font icons work).
    pub family: Option<String>,
    /// Points.
    pub size: f32,
    /// 100-1000 or a name. Unset = normal.
    pub weight: Option<FontWeight>,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
pub struct PathReplacement {
    pub from: String,
    pub to: String,
}

impl Default for HeaderConfig {
    fn default() -> Self {
        Self {
            template: "{path}".into(),
            align: HeaderAlign::Center,
            font: HeaderFont::default(),
            path_replacements: vec![PathReplacement { from: "$HOME".into(), to: "~".into() }],
            trim_path: true,
            path_ellipsis: "\u{2026}".into(),
            background: None,
            color: None,
            pop_out_icon: "pop-out-symbolic".into(),
            close_icon: "window-close-symbolic".into(),
            copy_icon: "edit-copy-symbolic".into(),
            command_timer: CommandTimerConfig::default(),
            bell: BellConfig::default(),
        }
    }
}

impl Default for HeaderFont {
    fn default() -> Self {
        Self { family: None, size: 9.5, weight: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct InactiveConfig {
    /// Fade towards the pane background: 0 = none, 1 = fully hidden.
    pub dim: f32,
    /// Saturation multiplier: 1 = unchanged, 0 = greyscale, 2 = double.
    pub saturation: f32,
    /// Lightness multiplier: 1 = unchanged, 0.8 = darker.
    pub lightness: f32,
    /// Hue rotation in degrees (-360..360).
    pub hue_shift: f32,
    /// Transition duration in milliseconds (0 = instant).
    pub fade_ms: u32,
}

impl Default for InactiveConfig {
    fn default() -> Self {
        Self { dim: 0.35, saturation: 1.0, lightness: 1.0, hue_shift: 0.0, fade_ms: 150 }
    }
}

impl InactiveConfig {
    /// Clamped to sane ranges so a typo can't blank the pane.
    pub fn style(&self) -> crate::terminal::InactiveStyle {
        crate::terminal::InactiveStyle {
            dim: self.dim.clamp(0.0, 1.0),
            saturation: self.saturation.clamp(0.0, 4.0),
            lightness: self.lightness.clamp(0.0, 4.0),
            hue_shift: self.hue_shift.clamp(-360.0, 360.0),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct TerminalConfig {
    /// Applies to terminals created after the change.
    pub scrollback: usize,
    pub shell: Option<String>,
    /// `auto` / `always` / `never`, or a map of options (see `ScrollbarConfig`). It only exists
    /// while there is scrollback.
    #[serde(deserialize_with = "scrollbar_or_mode")]
    pub scrollbar: ScrollbarConfig,
    /// `true` / `false`, or a map of options (see `ScrollButtonConfig`): a small round button that
    /// jumps back to the latest output while scrolled up.
    #[serde(deserialize_with = "button_or_bool")]
    pub scroll_to_bottom_button: ScrollButtonConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct ScrollbarConfig {
    /// `auto` (fades in while scrolling or when the pointer is at the right edge), `always`, `never`.
    pub mode: crate::terminal::ScrollbarMode,
    /// Thumb width at rest and when the pointer is near, in px.
    pub width: f32,
    pub hover_width: f32,
    /// Gap between the thumb and the pane's right edge, in px.
    pub margin: f32,
    /// Smallest thumb height, in px.
    pub min_thumb: f32,
    /// Thumb colour (`#rrggbb` / `#rrggbbaa`, or `{ dark, light }`). Unset = the pane's text colour.
    pub color: Option<ThemedTint>,
    /// Opacity at rest, while hovered, and of the track shown while hovered (0..1).
    pub opacity: f32,
    pub hover_opacity: f32,
    pub track_opacity: f32,
    /// How long it stays visible after scrolling (`auto` mode), and how long fades take.
    pub idle_ms: u32,
    pub fade_ms: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct ScrollButtonConfig {
    pub enabled: bool,
    /// Diameter in px, and the gap from the pane's right/bottom edges.
    pub size: f32,
    pub margin_right: f32,
    pub margin_bottom: f32,
    /// Circle and chevron colour. Unset = the pane's text colour.
    pub color: Option<ThemedTint>,
    /// Circle fill opacity, normal and hovered; chevron opacity, normal and hovered (0..1).
    pub background_opacity: f32,
    pub hover_background_opacity: f32,
    pub icon_opacity: f32,
    pub hover_icon_opacity: f32,
}

impl Default for ScrollbarConfig {
    fn default() -> Self {
        let d = crate::terminal::ScrollbarStyle::default();
        Self {
            mode: d.mode,
            width: d.width,
            hover_width: d.hover_width,
            margin: d.margin,
            min_thumb: d.min_thumb,
            color: None,
            opacity: d.opacity,
            hover_opacity: d.hover_opacity,
            track_opacity: d.track_opacity,
            idle_ms: d.idle_ms,
            fade_ms: d.fade_ms,
        }
    }
}

impl Default for ScrollButtonConfig {
    fn default() -> Self {
        let d = crate::terminal::ButtonStyle::default();
        Self {
            enabled: d.enabled,
            size: d.size,
            margin_right: d.margin_right,
            margin_bottom: d.margin_bottom,
            color: None,
            background_opacity: d.bg_opacity,
            hover_background_opacity: d.bg_hover_opacity,
            icon_opacity: d.icon_opacity,
            hover_icon_opacity: d.icon_hover_opacity,
        }
    }
}

impl ScrollbarConfig {
    /// Resolved for the current light/dark mode and clamped to sane ranges.
    pub fn style(&self, dark: bool) -> crate::terminal::ScrollbarStyle {
        let unit = |v: f32| v.clamp(0.0, 1.0);
        crate::terminal::ScrollbarStyle {
            mode: self.mode,
            width: self.width.clamp(1.0, 40.0),
            hover_width: self.hover_width.clamp(1.0, 40.0),
            margin: self.margin.clamp(0.0, 40.0),
            min_thumb: self.min_thumb.clamp(4.0, 400.0),
            color: self.color.and_then(|c| c.resolve(dark)),
            opacity: unit(self.opacity),
            hover_opacity: unit(self.hover_opacity),
            track_opacity: unit(self.track_opacity),
            idle_ms: self.idle_ms.min(60_000),
            fade_ms: self.fade_ms.min(5_000),
        }
    }
}

impl ScrollButtonConfig {
    pub fn style(&self, dark: bool) -> crate::terminal::ButtonStyle {
        let unit = |v: f32| v.clamp(0.0, 1.0);
        crate::terminal::ButtonStyle {
            enabled: self.enabled,
            size: self.size.clamp(12.0, 80.0),
            margin_right: self.margin_right.clamp(0.0, 200.0),
            margin_bottom: self.margin_bottom.clamp(0.0, 200.0),
            color: self.color.and_then(|c| c.resolve(dark)),
            bg_opacity: unit(self.background_opacity),
            bg_hover_opacity: unit(self.hover_background_opacity),
            icon_opacity: unit(self.icon_opacity),
            icon_hover_opacity: unit(self.hover_icon_opacity),
        }
    }
}

fn scrollbar_or_mode<'de, D: Deserializer<'de>>(d: D) -> Result<ScrollbarConfig, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Mode(crate::terminal::ScrollbarMode),
        Full(ScrollbarConfig),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::Mode(mode) => ScrollbarConfig { mode, ..ScrollbarConfig::default() },
        Raw::Full(c) => c,
    })
}

fn button_or_bool<'de, D: Deserializer<'de>>(d: D) -> Result<ScrollButtonConfig, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Enabled(bool),
        Full(ScrollButtonConfig),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::Enabled(enabled) => ScrollButtonConfig { enabled, ..ScrollButtonConfig::default() },
        Raw::Full(c) => c,
    })
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            watch: true,
            font: FontConfig::default(),
            window: WindowConfig::default(),
            panes: PanesConfig::default(),
            terminal: TerminalConfig::default(),
            session: SessionConfig::default(),
            search: SearchConfig::default(),
            palette: PaletteConfig::default(),
            status_bar: StatusBarConfig::default(),
        }
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "VictorMono Nerd Font Propo".into(),
            size: 11.0,
            features: None,
            weight: None,
            bold_weight: None,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            maximized: false,
            width: Dim::Px(1100.0),
            height: Dim::Px(700.0),
            color: None,
            inactive_dim: 0.12,
            vscode_colors: true,
            vscode_color_keys: vec![
                "peacock.color".into(),
                "titleBar.activeBackground".into(),
                "statusBar.background".into(),
            ],
        }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback: 10_000,
            shell: None,
            scrollbar: ScrollbarConfig::default(),
            scroll_to_bottom_button: ScrollButtonConfig::default(),
        }
    }
}

impl FontConfig {
    /// Pango font description string, e.g. `VictorMono Nerd Font Propo 11`.
    pub fn pango_string(&self) -> String {
        format!("{} {}", self.family, self.size)
    }
}

impl UserConfig {
    /// The window colour for the given mode (config value or the built-in default).
    pub fn window_color(&self, dark: bool) -> Rgb {
        self.window
            .color
            .and_then(|c| c.resolve(dark))
            .unwrap_or(if dark { DEFAULT_WINDOW_DARK } else { DEFAULT_WINDOW_LIGHT })
    }
}

pub fn user_config_path() -> PathBuf {
    glib::user_config_dir().join("paua").join("config.yaml")
}

/// YAML `\u` escapes are a single UTF-16 code unit, so glyphs above U+FFFF (most Nerd Font
/// Material Design icons) have to be written `\U000F0109`. Icon cheatsheets and JSON write them as
/// a surrogate pair instead (`\udb80\udd09`), which the YAML parser rejects. Convert such pairs
/// to the `\U` form first. An escaped backslash (`\\udb80`) is left alone, and so is a lone
/// surrogate (which stays an error).
pub fn normalize_surrogate_escapes(src: &str) -> std::borrow::Cow<'_, str> {
    if !src.contains("\\u") {
        return std::borrow::Cow::Borrowed(src);
    }
    let chars: Vec<char> = src.chars().collect();
    let hex4 = |at: usize| -> Option<u32> {
        let h: String = chars.get(at..at + 4)?.iter().collect();
        (h.len() == 4 && h.chars().all(|c| c.is_ascii_hexdigit())).then(|| u32::from_str_radix(&h, 16).ok())?
    };
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            if chars.get(i + 1) == Some(&'\\') {
                out.push_str("\\\\"); // an escaped backslash: not the start of an escape
                i += 2;
                continue;
            }
            if chars.get(i + 1) == Some(&'u') && chars.get(i + 6) == Some(&'\\') && chars.get(i + 7) == Some(&'u') {
                if let (Some(hi @ 0xD800..=0xDBFF), Some(lo @ 0xDC00..=0xDFFF)) = (hex4(i + 2), hex4(i + 8)) {
                    let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                    out.push_str(&format!("\\U{cp:08X}"));
                    i += 12;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    std::borrow::Cow::Owned(out)
}

/// Load the user config. A missing file gives defaults and no error; an unreadable or invalid
/// file gives `Err` so the caller can keep the previous config and tell the user.
pub fn load_user() -> Result<UserConfig, String> {
    let path = user_config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) if text.trim().is_empty() => Ok(UserConfig::default()),
        Ok(text) => serde_yaml_ng::from_str(&normalize_surrogate_escapes(&text))
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UserConfig::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

// ---------------------------------------------------------------------------------------------
// Directory config
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct DirConfig {
    pub window: DirWindow,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct DirWindow {
    pub color: Option<ThemedColor>,
}

/// Walk up from `start` and return the nearest `.paua.yaml`.
pub fn find_dir_config(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|d| d.join(DIR_CONFIG_NAME))
        .find(|p| p.is_file())
}

// ---------------------------------------------------------------------------------------------
// VS Code project colour (Peacock, or colours set by hand)
// ---------------------------------------------------------------------------------------------

/// Walk up from `start` and return the nearest `.vscode/settings.json`.
pub fn find_vscode_settings(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|d| d.join(".vscode").join("settings.json"))
        .find(|p| p.is_file())
}

/// VS Code settings files are JSON with comments and trailing commas. Remove both (outside
/// strings) so a normal JSON parser can read them.
pub fn strip_jsonc(src: &str) -> String {
    let src = src.trim_start_matches('\u{feff}');
    // Pass 1: comments.
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let (mut in_str, mut esc) = (false, false);
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match (c, chars.peek().copied()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    // Pass 2: trailing commas before `}` / `]`.
    let chars: Vec<char> = out.chars().collect();
    let mut res = String::with_capacity(chars.len());
    let (mut in_str, mut esc) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
        if in_str {
            res.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        if c == '"' {
            in_str = true;
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        res.push(c);
    }
    res
}

/// A CSS-ish colour as VS Code settings hold them: `#rrggbb[aa]`, `#rgb`, `rgb(r, g, b)`, or a
/// basic colour name. Alpha is ignored.
pub fn parse_css_colour(s: &str) -> Option<Rgb> {
    let s = s.trim();
    if s.starts_with('#') || s.chars().all(|c| c.is_ascii_hexdigit()) && matches!(s.len(), 3 | 6) {
        return parse_rgba(s).ok().map(|t| t.rgb);
    }
    if let Some(inner) = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
    {
        let parts: Vec<u8> = inner
            .split([',', ' ', '/'])
            .filter(|p| !p.is_empty())
            .take(3)
            .filter_map(|p| p.parse::<f32>().ok().map(|v| v.clamp(0.0, 255.0).round() as u8))
            .collect();
        return (parts.len() == 3).then(|| Rgb(parts[0], parts[1], parts[2]));
    }
    let named = match s.to_ascii_lowercase().as_str() {
        "red" => (0xdc, 0x14, 0x3c),
        "green" => (0x2e, 0x8b, 0x57),
        "blue" => (0x1e, 0x6f, 0xd9),
        "orange" => (0xff, 0x8c, 0x00),
        "yellow" => (0xe6, 0xb8, 0x00),
        "purple" => (0x80, 0x3c, 0xb4),
        "pink" => (0xe8, 0x5a, 0x9a),
        "teal" => (0x00, 0x80, 0x80),
        "cyan" => (0x00, 0xa8, 0xc8),
        "brown" => (0x8b, 0x5a, 0x2b),
        "black" => (0x10, 0x10, 0x10),
        "white" => (0xf4, 0xf4, 0xf4),
        "gray" | "grey" => (0x80, 0x80, 0x80),
        _ => return None,
    };
    Some(Rgb(named.0, named.1, named.2))
}

/// The first of `keys` that holds a usable colour in a VS Code `settings.json`. Each key is tried
/// at the top level (`peacock.color`) and inside `workbench.colorCustomizations`
/// (`titleBar.activeBackground`). `Ok(None)` if none of them is present.
pub fn load_vscode_color(path: &Path, keys: &[String]) -> Result<Option<Rgb>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&strip_jsonc(&text)).map_err(|e| format!("{}: {e}", path.display()))?;
    let customizations = value.get("workbench.colorCustomizations");
    Ok(keys.iter().find_map(|key| {
        value
            .get(key.as_str())
            .or_else(|| customizations.and_then(|c| c.get(key.as_str())))
            .and_then(|v| v.as_str())
            .and_then(parse_css_colour)
    }))
}

/// The window colour a directory asks for: its `.paua.yaml` colour if set, otherwise the VS Code
/// project colour (so our own config always takes precedence).
pub fn directory_color(dir: Option<&DirConfig>, vscode: Option<Rgb>, dark: bool) -> Option<Rgb> {
    dir.and_then(|d| d.window.color).and_then(|c| c.resolve(dark)).or(vscode)
}

pub fn load_dir(path: &Path) -> Result<DirConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(DirConfig::default());
    }
    serde_yaml_ng::from_str(&normalize_surrogate_escapes(&text)).map_err(|e| format!("{}: {e}", path.display()))
}

// ---------------------------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    Left,
    Right,
}

/// When an `exec` widget re-runs (besides the first time and `interval`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Refresh {
    /// When the pane's directory changes (default).
    #[default]
    Cwd,
    /// Also after every command (needs shell integration: OSC 133).
    Prompt,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct WhenConfig {
    /// Show only if one of these exists in the pane's directory or any parent.
    pub files: Vec<String>,
    /// Hide if any of these exists there.
    pub not_files: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
pub struct MenuEntry {
    pub label: String,
    /// What it does: see `WidgetConfig::action`.
    pub action: String,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct WidgetConfig {
    /// Identifies the widget (and names its CSS class `sb-<id>`).
    pub id: String,
    pub side: Side,
    /// When space runs out the lowest priority goes first (default 50).
    pub priority: i32,
    /// A built-in: `clock`, `git` or `dirs` (recent directories menu).
    pub builtin: Option<String>,
    /// Text template (see README): `{path}` `{dir}` `{host}` `{user}` `{title}` `{session}`
    /// `{exit}` and, for `exec`, `{output}`.
    pub text: Option<String>,
    /// A shell command whose first output line becomes `{output}`. Runs in a background thread in
    /// the focused pane's directory and is killed after `timeout`.
    pub exec: Option<String>,
    pub icon: String,
    pub tooltip: Option<String>,
    /// Clock: `strftime` format (default `%H:%M`).
    pub format: Option<String>,
    /// Makes it a button: `split_right`, `split_down`, `find`, `palette`, `clear`, `close_pane`,
    /// `pop_out`, `copy_path`, `new_window`, `cycle_scheme`, `reload_config`, `open_config`,
    /// `type:<text>` (typed into the pane + Enter) or `exec:<command>` (background, result as a toast).
    pub action: Option<String>,
    /// Makes it a menu.
    pub menu: Vec<MenuEntry>,
    /// Only show in some directories.
    pub when: Option<WhenConfig>,
    pub refresh: Refresh,
    /// Also re-run every so often (`exec` / `git`).
    pub interval: Option<Span>,
    /// Keep an `exec` result for this long per directory (default 30s).
    pub cache: Option<Span>,
    /// Kill the command after this long (default `status_bar.timeout`).
    pub timeout: Option<Span>,
    /// Hide an `exec` widget while it has no output (also when it failed or timed out).
    pub hide_empty: bool,
    pub color: Option<ThemedTint>,
    pub background: Option<ThemedTint>,
}

impl Default for WidgetConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            side: Side::Left,
            priority: 50,
            builtin: None,
            text: None,
            exec: None,
            icon: String::new(),
            tooltip: None,
            format: None,
            action: None,
            menu: Vec::new(),
            when: None,
            refresh: Refresh::Cwd,
            interval: None,
            cache: None,
            timeout: None,
            hide_empty: true,
            color: None,
            background: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct GitIcons {
    pub ahead: String,
    pub behind: String,
    /// Shown (instead of the separate ahead/behind icons) when the branch is both.
    pub diverged: String,
    pub staged: String,
    pub modified: String,
    pub deleted: String,
    pub renamed: String,
    pub untracked: String,
    pub conflicted: String,
    pub stashed: String,
}

impl Default for GitIcons {
    fn default() -> Self {
        let s = |t: &str| t.to_owned();
        Self {
            ahead: s("↑"),
            behind: s("↓"),
            diverged: s("⇅"),
            staged: s("●"),
            modified: s("✚"),
            deleted: s("✖"),
            renamed: s("»"),
            untracked: s("?"),
            conflicted: s("!"),
            stashed: s("≡"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct GitConfig {
    /// Before the branch name (a Nerd Font glyph such as `` looks nice).
    pub icon: String,
    /// Placeholders: `{branch}` `{upstream}` `{ahead}` `{behind}` `{staged}` `{modified}` `{deleted}`
    /// `{renamed}` `{untracked}` `{conflicted}` `{stashed}` `{status}` (all the icons with
    /// counts) and `{dirty}` (`*` when there are changes).
    pub format: String,
    pub icons: GitIcons,
    /// Show counts next to the icons in `{status}`.
    pub counts: bool,
    /// Refresh this often while the window is focused (besides on directory change and after commands).
    pub interval: Span,
    /// Kill `git status` after this long.
    pub timeout: Span,
    /// The sync button (pull, then push) next to the branch.
    pub sync_button: bool,
    /// Its icon: a theme icon name (`view-refresh-symbolic`, `emblem-synchronizing-symbolic`...) or
    /// text such as a Nerd Font glyph. Empty = none.
    pub sync_icon: String,
    /// Give up on fetch / pull / push after this long.
    pub network_timeout: Span,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            icon: "⎇".into(),
            format: "{branch}{?dirty:*} {status}".into(),
            icons: GitIcons::default(),
            counts: true,
            interval: Span(std::time::Duration::from_secs(5)),
            timeout: Span(std::time::Duration::from_secs(3)),
            sync_button: true,
            sync_icon: "view-refresh-symbolic".into(),
            network_timeout: Span(std::time::Duration::from_secs(60)),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct StatusFont {
    pub family: Option<String>,
    /// Points. Unset = 0.9 x the UI font size.
    pub size: Option<f32>,
    /// 100-1000 or a name (`bold`, `semibold`...).
    pub weight: Option<FontWeight>,
    pub italic: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))] // tests catch typos in the examples
#[serde(default)]
pub struct StatusBarConfig {
    pub enabled: bool,
    /// Default kill time for `exec` widgets.
    pub timeout: Span,
    /// The bar's text. Anything unset keeps the system UI font.
    pub font: StatusFont,
    pub git: GitConfig,
    /// Replaces the default set when present.
    pub widgets: Vec<WidgetConfig>,
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        let git = WidgetConfig { id: "git".into(), builtin: Some("git".into()), priority: 80, ..Default::default() };
        let dirs = WidgetConfig {
            id: "dirs".into(),
            builtin: Some("dirs".into()),
            side: Side::Right,
            priority: 30,
            icon: "⌂".into(),
            ..Default::default()
        };
        let clock = WidgetConfig {
            id: "clock".into(),
            builtin: Some("clock".into()),
            side: Side::Right,
            priority: 20,
            ..Default::default()
        };
        Self {
            enabled: true,
            timeout: Span(std::time::Duration::from_secs(2)),
            font: StatusFont::default(),
            git: GitConfig::default(),
            widgets: vec![git, dirs, clock],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_comments_and_trailing_commas_are_tolerated() {
        let body = r##"{
  // a line comment with "quotes" and a trailing comma below
  "editor.fontSize": 14, /* block */
  "url": "http://example.com//not-a-comment",
  "peacock.color": "#42b883", // Vue green
  "list": [1, 2, 3,],
  "obj": { "a": "x,]", },
}"##;
        let src = format!("\u{feff}{body}"); // files may start with a byte-order mark
        let v: serde_json::Value = serde_json::from_str(&strip_jsonc(&src)).unwrap();
        assert_eq!(v["peacock.color"], "#42b883");
        assert_eq!(v["url"], "http://example.com//not-a-comment");
        assert_eq!(v["obj"]["a"], "x,]");
    }

    #[test]
    fn colour_strings_as_vscode_settings_hold_them() {
        assert_eq!(parse_css_colour("#42b883"), Some(Rgb(0x42, 0xb8, 0x83)));
        assert_eq!(parse_css_colour("#15202b99"), Some(Rgb(0x15, 0x20, 0x2b))); // alpha dropped
        assert_eq!(parse_css_colour("#abc"), Some(Rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_css_colour("rgb(10, 20, 30)"), Some(Rgb(10, 20, 30)));
        assert!(parse_css_colour("Red").is_some());
        assert_eq!(parse_css_colour("not a colour"), None);
    }

    #[test]
    fn reads_peacock_or_workbench_colours_up_the_tree_with_paua_precedence() {
        let keys = UserConfig::default().window.vscode_color_keys;
        let root = std::env::temp_dir().join(format!("paua-vscode-{}", std::process::id()));
        let deep = root.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(root.join(".vscode")).unwrap();
        let settings = root.join(".vscode/settings.json");

        // Peacock's own key, in a file with comments and a trailing comma.
        std::fs::write(&settings, "{\n  // hi\n  \"peacock.color\": \"#832561\",\n}\n").unwrap();
        let found = find_vscode_settings(&deep).expect("walks up to the project root");
        assert_eq!(found, settings);
        assert_eq!(load_vscode_color(&found, &keys).unwrap(), Some(Rgb(0x83, 0x25, 0x61)));

        // No peacock key: the title bar colour that Peacock (or you) put in colorCustomizations.
        std::fs::write(
            &settings,
            r##"{ "workbench.colorCustomizations": { "statusBar.background": "#222222", "titleBar.activeBackground": "#15202b" } }"##,
        )
        .unwrap();
        assert_eq!(load_vscode_color(&found, &keys).unwrap(), Some(Rgb(0x15, 0x20, 0x2b))); // key order
        // The order is configurable.
        let by_status = vec!["statusBar.background".to_string()];
        assert_eq!(load_vscode_color(&found, &by_status).unwrap(), Some(Rgb(0x22, 0x22, 0x22)));

        // Nothing relevant -> None; missing file or broken JSON -> error.
        std::fs::write(&settings, "{ \"editor.tabSize\": 2 }").unwrap();
        assert_eq!(load_vscode_color(&found, &keys).unwrap(), None);
        assert!(load_vscode_color(&root.join("nope.json"), &keys).is_err());
        std::fs::write(&settings, "{ not json").unwrap();
        assert!(load_vscode_color(&found, &keys).is_err());
        let _ = std::fs::remove_dir_all(&root);

        // Precedence: .paua.yaml colour wins; otherwise the VS Code colour; otherwise nothing.
        let mine: DirConfig = serde_yaml_ng::from_str("window: { color: '#112233' }").unwrap();
        let empty = DirConfig::default();
        let vs = Some(Rgb(1, 2, 3));
        assert_eq!(directory_color(Some(&mine), vs, true), Some(Rgb(0x11, 0x22, 0x33)));
        assert_eq!(directory_color(Some(&empty), vs, true), vs);
        assert_eq!(directory_color(None, vs, false), vs);
        assert_eq!(directory_color(None, None, true), None);
    }

    #[test]
    fn parses_full_user_config() {
        let cfg: UserConfig = serde_yaml_ng::from_str(
            r##"
watch: false
font: { family: Fira Code, size: 12.5, features: "-calt" }
window:
  maximized: true
  width: 80%
  height: 900px
  color: { dark: "#102030", light: "#abc" }
terminal: { scrollback: 500 }
panes: { inactive: { dim: 0.5, saturation: 0.4, hue_shift: -20, fade_ms: 0 } }
"##,
        )
        .unwrap();
        assert!(!cfg.watch && cfg.window.maximized);
        assert_eq!(cfg.window.width, Dim::Percent(80.0));
        assert_eq!(cfg.window.height, Dim::Px(900.0));
        assert_eq!(cfg.font.pango_string(), "Fira Code 12.5");
        assert_eq!(cfg.window_color(true), Rgb(0x10, 0x20, 0x30));
        assert_eq!(cfg.window_color(false), Rgb(0xaa, 0xbb, 0xcc));
        assert_eq!(cfg.terminal.scrollback, 500);
        let i = cfg.panes.inactive;
        assert_eq!((i.dim, i.saturation, i.lightness, i.hue_shift, i.fade_ms), (0.5, 0.4, 1.0, -20.0, 0));
    }

    #[test]
    fn parses_header_config() {
        let cfg: UserConfig = serde_yaml_ng::from_str(
            r##"
panes:
  header:
    template: "{title|dir}"
    align: right
    font: { family: Inter, size: 10 }
    path_replacements:
      - { from: /home/greg, to: "\uf015" }
      - { from: /srv, to: "srv:" }
    trim_path: false
    path_ellipsis: "..."
    background: { dark: "#ffffff14", light: "#00000010" }
    color: "#c0c0c8"
    pop_out_icon: focus-windows-symbolic
"##,
        )
        .unwrap();
        let h = cfg.panes.header;
        assert_eq!(h.template, "{title|dir}");
        assert_eq!(h.align, HeaderAlign::End);
        assert_eq!((h.font.family.as_deref(), h.font.size), (Some("Inter"), 10.0));
        assert_eq!(h.path_replacements.len(), 2);
        assert!(!h.trim_path && h.path_ellipsis == "...");
        let bg = h.background.unwrap();
        assert_eq!(bg.resolve(true).unwrap().rgb, Rgb(255, 255, 255));
        assert!((bg.resolve(true).unwrap().alpha - 20.0 / 255.0).abs() < 1e-6);
        assert_eq!(bg.resolve(false).unwrap().rgb, Rgb(0, 0, 0));
        assert_eq!(h.color.unwrap().resolve(false).unwrap().alpha, 1.0);
        assert_eq!(h.pop_out_icon, "focus-windows-symbolic");
        assert_eq!(h.close_icon, "window-close-symbolic");
        assert_eq!(h.copy_icon, "edit-copy-symbolic");
        assert_eq!(h.path_replacements[0].to, "\u{f015}");

        // Defaults: centred, "{path}", $HOME -> ~.
        let d = UserConfig::default().panes.header;
        assert_eq!((d.align, d.template.as_str()), (HeaderAlign::Center, "{path}"));
        assert!(d.trim_path && d.path_ellipsis == "\u{2026}");
        assert_eq!((d.path_replacements[0].from.as_str(), d.path_replacements[0].to.as_str()), ("$HOME", "~"));
    }

    #[test]
    fn parses_font_weights_by_name_and_number() {
        let cfg: UserConfig = serde_yaml_ng::from_str(
            "font: { weight: Medium, bold_weight: 800 }\npanes: { header: { font: { weight: semi-bold } } }",
        )
        .unwrap();
        assert_eq!(cfg.font.weight, Some(FontWeight(500)));
        assert_eq!(cfg.font.bold_weight, Some(FontWeight(800)));
        assert_eq!(cfg.panes.header.font.weight, Some(FontWeight(600)));
        assert_eq!(UserConfig::default().font.weight, None);
        assert!(serde_yaml_ng::from_str::<UserConfig>("font: { weight: 50 }").is_err());
        assert!(serde_yaml_ng::from_str::<UserConfig>("font: { weight: chunky }").is_err());
    }

    #[test]
    fn parses_padding_and_window_inactive_dim() {
        use crate::terminal::Padding;
        let p = |y: &str| serde_yaml_ng::from_str::<UserConfig>(y).unwrap().panes.padding;
        assert_eq!(UserConfig::default().panes.padding, Padding::default());
        assert_eq!(p("panes: { padding: 6 }"), Padding { top: 6.0, right: 6.0, bottom: 6.0, left: 6.0 });
        assert_eq!(
            p("panes: { padding: { x: 8, y: 4, top: 0 } }"),
            Padding { top: 0.0, right: 8.0, bottom: 4.0, left: 8.0 }
        );
        assert!(serde_yaml_ng::from_str::<UserConfig>("panes: { padding: -3 }").is_err());
        let c: UserConfig = serde_yaml_ng::from_str("window: { inactive_dim: 0.3 }").unwrap();
        assert_eq!(c.window.inactive_dim, 0.3);
        assert_eq!(UserConfig::default().window.inactive_dim, 0.12);
    }

    #[test]
    fn parses_scrollbar_options() {
        use crate::terminal::ScrollbarMode;
        let c: UserConfig =
            serde_yaml_ng::from_str("terminal: { scrollbar: always, scroll_to_bottom_button: false }").unwrap();
        assert_eq!(c.terminal.scrollbar.mode, ScrollbarMode::Always);
        assert!(!c.terminal.scroll_to_bottom_button.enabled);
        let d = UserConfig::default().terminal;
        assert_eq!(d.scrollbar.mode, ScrollbarMode::Auto);
        assert!(d.scroll_to_bottom_button.enabled);
        assert!(serde_yaml_ng::from_str::<UserConfig>("terminal: { scrollbar: sometimes }").is_err());

        // Full maps: widths, colours, opacities, timings; clamped on use.
        let c: UserConfig = serde_yaml_ng::from_str(
            r##"
terminal:
  scrollbar:
    mode: always
    width: 6
    hover_width: 12
    color: { dark: "#88c0ffcc", light: "#2050a0" }
    opacity: 0.5
    idle_ms: 3000
  scroll_to_bottom_button: { size: 34, color: "#ffffff", background_opacity: 0.3 }
"##,
        )
        .unwrap();
        let (dark, light) = (c.terminal.scrollbar.style(true), c.terminal.scrollbar.style(false));
        assert_eq!((dark.width, dark.hover_width, dark.idle_ms, dark.mode), (6.0, 12.0, 3000, ScrollbarMode::Always));
        assert_eq!(dark.color.unwrap().rgb, Rgb(0x88, 0xc0, 0xff));
        assert!((dark.color.unwrap().alpha - 0.8).abs() < 0.01);
        assert_eq!(light.color.unwrap().rgb, Rgb(0x20, 0x50, 0xa0));
        let b = c.terminal.scroll_to_bottom_button.style(true);
        assert!(b.enabled && b.size == 34.0 && b.bg_opacity == 0.3);
        // Absurd values are clamped, not trusted.
        let wild: UserConfig =
            serde_yaml_ng::from_str("terminal: { scrollbar: { width: 9000, opacity: 7 } }").unwrap();
        let s = wild.terminal.scrollbar.style(true);
        assert!(s.width <= 40.0 && s.opacity <= 1.0);
    }

    #[test]
    fn surrogate_pair_escapes_are_accepted_for_icons() {
        let yaml = "panes: { header: { command_timer: { icon: \"\\udb80\\udd09 \" } } }";
        // As written, the YAML parser rejects the pair...
        assert!(serde_yaml_ng::from_str::<UserConfig>(yaml).is_err());
        // ...after normalising it is U+F0109, the same as the `\U` spelling.
        let c: UserConfig = serde_yaml_ng::from_str(&normalize_surrogate_escapes(yaml)).unwrap();
        assert_eq!(c.panes.header.command_timer.icon, "\u{f0109} ");
        let big: UserConfig =
            serde_yaml_ng::from_str("panes: { header: { command_timer: { icon: \"\\U000F0109 \" } } }").unwrap();
        assert_eq!(big.panes.header.command_timer.icon, c.panes.header.command_timer.icon);

        let n = |s: &str| normalize_surrogate_escapes(s).into_owned();
        // Ordinary escapes, BMP icons and text without escapes are untouched.
        assert_eq!(n("\"\\uf015 and \\u00e9\""), "\"\\uf015 and \\u00e9\"");
        assert_eq!(n("plain: text"), "plain: text");
        // An escaped backslash followed by u... is literal text, not an escape.
        assert_eq!(n("\"\\\\udb80\\udd09\""), "\"\\\\udb80\\udd09\"");
        // A lone high surrogate is left alone (and remains an error), upper- or lower-case hex.
        assert_eq!(n("\"\\udb80 x\""), "\"\\udb80 x\"");
        assert_eq!(n("\"\\uDB80\\uDD09\""), "\"\\U000F0109\"");
        // Several pairs in one string.
        assert_eq!(n("\"\\udb80\\udd09\\ud83d\\ude00\""), "\"\\U000F0109\\U0001F600\"");
        // Pairs in a config file are fine through the loaders' helper too.
        assert!(serde_yaml_ng::from_str::<DirConfig>(&normalize_surrogate_escapes("window: { color: '#123456' }")).is_ok());
    }

    #[test]
    fn parses_status_bar_widgets() {
        let yaml = r#"
status_bar:
  timeout: 1s
  git: { counts: false, icons: { staged: "+" } }
  widgets:
    - { id: node, side: right, priority: 7, exec: "node -v", when: { files: [package.json] }, interval: 10s }
    - { id: menu, menu: [ { label: Split, action: split_right } ] }
"#;
        let c: UserConfig = serde_yaml_ng::from_str(yaml).unwrap();
        let sb = &c.status_bar;
        assert_eq!(sb.widgets.len(), 2);
        assert_eq!((sb.widgets[0].side, sb.widgets[0].priority), (Side::Right, 7));
        assert_eq!(sb.widgets[0].when.as_ref().unwrap().files, vec!["package.json"]);
        assert_eq!(sb.widgets[1].menu[0].action, "split_right");
        assert!(sb.widgets[1].hide_empty && sb.git.icons.staged == "+" && !sb.git.counts);
        assert_eq!(UserConfig::default().status_bar.widgets.len(), 3);
    }

    /// The shipped examples are documentation: they must load with the real types, and with unknown
    /// keys rejected (the structs deny them in test builds), so a typo or a removed option fails here.
    #[test]
    fn the_example_configs_are_valid() {
        let text = normalize_surrogate_escapes(include_str!("../examples/config.yaml"));
        let c: UserConfig = serde_yaml_ng::from_str(&text).unwrap_or_else(|e| panic!("examples/config.yaml: {e}"));
        assert_eq!(c.font.family, "VictorMono Nerd Font Propo");
        assert_eq!(c.panes.header.font.family.as_deref(), Some("Ubuntu Nerd Font"));
        assert_eq!(c.status_bar.font.family.as_deref(), Some("Ubuntu Nerd Font"));
        let d = normalize_surrogate_escapes(include_str!("../examples/paua.yaml"));
        let d: DirConfig = serde_yaml_ng::from_str(&d).unwrap_or_else(|e| panic!("examples/paua.yaml: {e}"));
        assert!(d.window.color.is_some());
    }

    // -----------------------------------------------------------------------------------------
    // Documentation coverage: docs/config/*.md must describe exactly the options that exist.
    // -----------------------------------------------------------------------------------------

    /// The field names of a config struct, read from serde's "unknown field ..., expected one of
    /// ..." error (test builds reject unknown fields).
    fn field_names<T: serde::de::DeserializeOwned>() -> Vec<String> {
        let err = serde_yaml_ng::from_str::<T>("{ __nope__: 1 }")
            .err()
            .expect("config structs deny unknown fields in test builds")
            .to_string();
        let after = err.split("expected").nth(1).unwrap_or("");
        after.split('`').skip(1).step_by(2).map(str::to_owned).collect()
    }

    /// Every config option as a dotted path (`panes.header.bell.duration`; `[]` = each list entry).
    /// When you add a config struct, add it here (see AGENTS.md).
    fn all_option_paths() -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut add = |prefix: &str, fields: Vec<String>| {
            for f in fields {
                out.push(if prefix.is_empty() { f } else { format!("{prefix}.{f}") });
            }
        };
        add("", field_names::<UserConfig>());
        add("font", field_names::<FontConfig>());
        add("window", field_names::<WindowConfig>());
        add("panes", field_names::<PanesConfig>());
        add("panes.inactive", field_names::<InactiveConfig>());
        add("panes.header", field_names::<HeaderConfig>());
        add("panes.header.font", field_names::<HeaderFont>());
        add("panes.header.path_replacements[]", field_names::<PathReplacement>());
        add("panes.header.command_timer", field_names::<CommandTimerConfig>());
        add("panes.header.bell", field_names::<BellConfig>());
        add("terminal", field_names::<TerminalConfig>());
        add("terminal.scrollbar", field_names::<ScrollbarConfig>());
        add("terminal.scroll_to_bottom_button", field_names::<ScrollButtonConfig>());
        add("session", field_names::<SessionConfig>());
        for kind in ["root", "sudo", "ssh", "container", "remote", "multiplexer"] {
            add(&format!("session.{kind}"), field_names::<SessionKindConfig>());
        }
        add("search", field_names::<SearchConfig>());
        add("palette", field_names::<PaletteConfig>());
        add("palette.commands[]", field_names::<CustomCommand>());
        add("status_bar", field_names::<StatusBarConfig>());
        add("status_bar.font", field_names::<StatusFont>());
        add("status_bar.git", field_names::<GitConfig>());
        add("status_bar.git.icons", field_names::<GitIcons>());
        add("status_bar.widgets[]", field_names::<WidgetConfig>());
        add("status_bar.widgets[].when", field_names::<WhenConfig>());
        add("status_bar.widgets[].menu[]", field_names::<MenuEntry>());
        out
    }

    fn docs_text() -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        let mut files = vec![root.join("README.md")];
        for e in std::fs::read_dir(root.join("config")).expect("docs/config exists") {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "md") {
                files.push(p);
            }
        }
        files.iter().map(|f| std::fs::read_to_string(f).unwrap()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn docs_cover_every_config_option() {
        let docs = docs_text();
        let missing: Vec<String> =
            all_option_paths().into_iter().filter(|p| !docs.contains(&format!("`{p}`"))).collect();
        assert!(
            missing.is_empty(),
            "options missing from docs/config/*.md (document each as `path` in a table row): {missing:#?}"
        );
    }

    #[test]
    fn docs_do_not_describe_options_that_do_not_exist() {
        let known: std::collections::HashSet<String> = all_option_paths().into_iter().collect();
        let roots = ["watch", "font", "window", "panes", "terminal", "session", "search", "palette", "status_bar"];
        let mut unknown = Vec::new();
        for line in docs_text().lines().filter(|l| l.starts_with("| `")) {
            let Some(path) = line[3..].split('`').next() else { continue };
            let root = path.split(['.', '[']).next().unwrap_or("");
            if roots.contains(&root) && !known.contains(path) {
                unknown.push(path.to_owned());
            }
        }
        assert!(unknown.is_empty(), "documented options that don't exist (renamed or removed?): {unknown:#?}");
    }

    #[test]
    fn parses_status_font() {
        let c: UserConfig =
            serde_yaml_ng::from_str("status_bar: { font: { family: Foo, size: 10.5, weight: semibold, italic: true } }").unwrap();
        let f = &c.status_bar.font;
        assert_eq!((f.family.as_deref(), f.size, f.weight.map(|w| w.0), f.italic), (Some("Foo"), Some(10.5), Some(600), true));
        let css = crate::statusbar::css(&c.status_bar);
        assert!(css.contains("font-family: \"Foo\"") && css.contains("font-size: 10.5pt") && css.contains("font-weight: 600"));
    }

    #[test]
    fn parses_bell_config() {
        let c: UserConfig = serde_yaml_ng::from_str("panes: { header: { bell: { duration: 250ms, color: \"#ff0000aa\" } } }").unwrap();
        assert_eq!(c.panes.header.bell.duration.0, std::time::Duration::from_millis(250));
        assert!(c.panes.header.bell.color.is_some() && c.panes.header.bell.enabled);
    }

    #[test]
    fn parses_timer_position() {
        let c: UserConfig = serde_yaml_ng::from_str("panes: { header: { command_timer: { position: left } } }").unwrap();
        assert_eq!(c.panes.header.command_timer.position, TimerPosition::Left);
        assert_eq!(UserConfig::default().panes.header.command_timer.position, TimerPosition::Right);
    }

    #[test]
    fn parses_command_timer_config() {
        use std::time::Duration;
        let d = UserConfig::default().panes.header.command_timer;
        assert!(d.enabled && d.ignore_alt_screen);
        assert_eq!((d.after.0, d.linger.0), (Duration::from_secs(2), Duration::from_secs(4)));
        assert!(d.ignore.iter().any(|i| i == "ssh"));

        let c: UserConfig = serde_yaml_ng::from_str(
            r##"
panes:
  header:
    command_timer: { after: 500ms, linger: 0, ignore: [vim], icon: "T ", background: "#ffaa0033" }
"##,
        )
        .unwrap();
        let t = &c.panes.header.command_timer;
        assert_eq!((t.after.0, t.linger.0), (Duration::from_millis(500), Duration::ZERO));
        assert_eq!(t.ignore, vec!["vim".to_string()]);
        assert_eq!(t.settings().threshold, Duration::from_millis(500));
        for (text, want) in [("5", 5.0), ("1.5m", 90.0), ("2h", 7200.0), ("250ms", 0.25), ("2s", 2.0)] {
            assert_eq!(Span::parse(text).unwrap().0.as_secs_f64(), want, "{text}");
        }
        assert!(Span::parse("soon").is_err() && Span::parse("-1s").is_err() && Span::parse("999h").is_err());
        assert!(serde_yaml_ng::from_str::<UserConfig>("panes: { header: { command_timer: { after: nope } } }").is_err());
    }

    #[test]
    fn parses_palette_config() {
        let d = UserConfig::default().palette;
        assert!(d.directories && d.ssh_hosts && d.commands.is_empty() && d.max_results == 40);
        let c: UserConfig = serde_yaml_ng::from_str(
            r##"
palette:
  max_results: 12
  ssh_hosts: false
  commands:
    - { title: "Run tests", run: "cargo test" }
    - { title: "Git status", run: "git status -sb", subtitle: "in this repo" }
"##,
        )
        .unwrap();
        assert_eq!((c.palette.max_results, c.palette.ssh_hosts, c.palette.directories), (12, false, true));
        assert_eq!(c.palette.commands.len(), 2);
        assert_eq!(c.palette.commands[0].run, "cargo test");
        assert_eq!(c.palette.commands[1].subtitle.as_deref(), Some("in this repo"));
        assert!(serde_yaml_ng::from_str::<UserConfig>("palette: { commands: [{ title: 'x' }] }").is_err());
    }

    #[test]
    fn parses_search_config() {
        use crate::terminal::CaseMode;
        let d = UserConfig::default().search;
        assert_eq!((d.case, d.regex), (CaseMode::Smart, false));
        let c: UserConfig = serde_yaml_ng::from_str(
            "search: { case: sensitive, regex: true, current_color: { dark: '#ff0000cc', light: '#aa0000' } }",
        )
        .unwrap();
        assert_eq!((c.search.case, c.search.regex), (CaseMode::Sensitive, true));
        let dark = c.search.style(true);
        assert_eq!(dark.current.rgb, Rgb(255, 0, 0));
        assert_eq!(dark.matched, crate::terminal::FindStyle::default().matched); // unset: built-in
        assert!(serde_yaml_ng::from_str::<UserConfig>("search: { case: whatever }").is_err());
    }

    #[test]
    fn parses_session_config() {
        use crate::session::Kind;
        let d = UserConfig::default().session;
        assert!(d.detect && d.badges && d.poll_ms == 1000);
        assert_eq!(d.kind(Kind::Ssh).label, "ssh {target}");
        assert!(d.kind(Kind::Root).window_color.is_none());

        let c: UserConfig = serde_yaml_ng::from_str(
            r##"
session:
  poll_ms: 500
  ssh: { label: "{host}", icon: "\uf817 ", window_color: "#1d4f91" }
  root: { window_color: { dark: "#7a1f1f", light: "#c43b3b" }, background: "#ff000040", header_background: "#ff202030", header_color: "#ffb0b0" }
  multiplexer: { enabled: false }
"##,
        )
        .unwrap();
        assert_eq!(c.session.poll_ms, 500);
        assert_eq!(c.session.ssh.label, "{host}");
        assert_eq!(c.session.ssh.icon, "\u{f817} ");
        assert_eq!(c.session.ssh.window_color.unwrap().resolve(true), Some(Rgb(0x1d, 0x4f, 0x91)));
        assert_eq!(c.session.root.window_color.unwrap().resolve(false), Some(Rgb(0xc4, 0x3b, 0x3b)));
        assert_eq!(c.session.root.label, "{tool} {target}"); // untouched fields keep the *struct* default
        assert!(!c.session.multiplexer.enabled);
        let root = &c.session.root;
        assert_eq!(root.header_background.unwrap().resolve(true).unwrap().rgb, Rgb(255, 0x20, 0x20));
        assert_eq!(root.header_color.unwrap().resolve(false).unwrap().rgb, Rgb(255, 0xb0, 0xb0));
        assert!(c.session.ssh.header_background.is_none()); // unset = the built-in tint
    }

    #[test]
    fn defaults_fill_missing_fields() {
        let cfg: UserConfig = serde_yaml_ng::from_str("font: { size: 9 }").unwrap();
        assert_eq!(cfg.font.family, "VictorMono Nerd Font Propo");
        assert!(cfg.watch);
        assert_eq!(cfg.window_color(true), DEFAULT_WINDOW_DARK);
    }

    #[test]
    fn single_colour_applies_to_both_modes_and_bad_values_error() {
        let d: DirConfig = serde_yaml_ng::from_str("window: { color: '#ff0000' }").unwrap();
        let c = d.window.color.unwrap();
        assert_eq!(c.resolve(true), Some(Rgb(255, 0, 0)));
        assert_eq!(c.resolve(false), Some(Rgb(255, 0, 0)));
        assert!(serde_yaml_ng::from_str::<DirConfig>("window: { color: 'nope' }").is_err());
        assert!(serde_yaml_ng::from_str::<UserConfig>("window: { width: -5 }").is_err());
    }
}

