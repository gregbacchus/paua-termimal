# Configuration overview

**Start with `paua setup`.** It detects your installed Nerd Fonts, asks which to use for the terminal
and for the window decorations, and writes a minimal `config.yaml` with just those choices. This
section documents everything you can then tune by hand.

Paua reads one YAML file, plus optional per-directory files.

| File | Purpose |
|---|---|
| `~/.config/paua/config.yaml` (honours `$XDG_CONFIG_HOME`) | The user config: everything on these pages |
| `.paua.yaml` in a project directory (or any parent) | The window colour for that directory. See [per-directory config](directory.md) |
| `.vscode/settings.json` | A project colour (Peacock etc.), read as a fallback. See [per-directory config](directory.md) |

**Every option is optional.** A missing file, an empty file, or a missing key all fall back to the
defaults listed on these pages. *Edit → Open Config File* (or the palette command *Open config
file*) opens the file, creating a small starter if it doesn't exist yet. A copy-paste starting
point with every option set is [`examples/config.yaml`](../../examples/config.yaml).

## Sections

| Key | Page |
|---|---|
| `watch` | [General](general.md) |
| `font` | [General](general.md) |
| `window` | [General](general.md) |
| `panes` | [Panes](panes.md) |
| `terminal` | [Terminal](terminal.md) |
| `session` | [Sessions](session.md) |
| `terminal.theme`, `terminal.colors` | [Themes](themes.md) |
| `search` | [Search and palette](search-and-palette.md) |
| `palette` | [Search and palette](search-and-palette.md) |
| `status_bar` | [Status bar](status-bar.md) |

Using icons? See [Nerd Fonts: fonts, icons and good values](nerd-fonts.md).

## Loading, reloading and errors

- With `watch: true` (the default) the user config is reloaded when the file changes, and the
  focused pane's `.paua.yaml` when that changes. *Reload Config* does it on demand. Most options
  apply immediately to every window and pane. Exceptions are called out per option as
  **startup only** (window size and maximised state) or **new terminals only** (`terminal.shell`,
  `terminal.scrollback`).
- If the file can't be parsed, or a value is invalid (a bad colour, a negative size...), the app
  shows a *Config error* toast with the file, line and reason and **keeps the previous settings**.
  At startup it uses the defaults instead.
- Unknown keys are ignored, so older and newer config files keep loading (a typo silently does
  nothing; the example file is validated strictly by the test suite).
- `.vscode/settings.json` is not watched: it is re-read when you enter a directory and when the
  pane gets focus.

## Window colour precedence

The frame (title bar, status bar and borders) takes the first of:

1. the colour of an active **session** (`session.<kind>.window_color`) in the window's focused pane,
2. the directory's colour: `.paua.yaml` `window.color`, then the VS Code project colour
   (`window.vscode_colors`),
3. `window.color`,
4. the built-in default: `#4a2f80` (dark) / `#7a56c2` (light).

Changes of frame colour animate.

## Notation on these pages

- Option paths are written in full (`panes.header.bell.duration`) and **every option is listed in
  a table whose first column is the path in backticks**; the docs test relies on that.
- `[]` in a path means "each entry of this list" (`palette.commands[].title`).
- *Type* names are defined in [value types](value-types.md).
