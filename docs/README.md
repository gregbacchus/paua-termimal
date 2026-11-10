# Paua documentation

**New here? Run `paua setup`**: it finds your Nerd Fonts, asks which to use for the terminal and the
window decorations, and writes a minimal config (see the [README](../README.md#get-started)).
Everything below is optional tuning.

| Page | What it covers |
|---|---|
| [Configuration overview](config/README.md) | Where the config lives, loading, live reload, errors, precedence |
| [Value types](config/value-types.md) | Colours, durations, sizes, font weights, padding, templates, YAML escapes |
| [General: `watch`, `font`, `window`](config/general.md) | Reloading, the terminal font, window size and colour |
| [Panes: `panes`](config/panes.md) | Padding, inactive-pane look, tab row and indicators, headers, command timer, bell flash |
| [Terminal: `terminal`](config/terminal.md) | Scrollback, shell, scrollbar, scroll-to-bottom button, clickable URLs |
| [Sessions: `session`](config/session.md) | Detecting ssh / sudo / containers / other hosts, badges and tints |
| [Themes: `terminal.theme`, `terminal.colors`](config/themes.md) | Terminal colour themes: built-in, ~900 installable, per light / dark, your own |
| [Search and palette: `search`, `palette`](config/search-and-palette.md) | Find in output, command palette, custom commands |
| [Keyboard shortcuts: `shortcuts`](config/shortcuts.md) | Default keys, action IDs and custom bindings |
| [Status bar: `status_bar`](config/status-bar.md) | Widgets, priorities, `exec`, git, templates, actions |
| [Nerd Fonts](config/nerd-fonts.md) | Choosing a font variant, good values, icon codepoints |
| [Per-directory config](config/directory.md) | `.paua.yaml` and VS Code project colours |

A complete, working example is [`examples/config.yaml`](../examples/config.yaml); it is checked by
the test suite, so every key in it is real.

These pages are **exhaustive and must stay in sync with the code**: a test
(`docs_cover_every_config_option` in `src/config.rs`) fails when a config option is missing from
the docs, or documented but gone. See [`AGENTS.md`](../AGENTS.md).
