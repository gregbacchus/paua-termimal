# AGENTS.md: Paua

Paua (te reo Māori for abalone shell: iridescent, colourful) is a **native Rust GTK4 + libadwaita
terminal emulator for GNOME/Ubuntu**. VS Code-like custom-coloured window chrome (title bar, status
bar, frame), tiled panes with draggable dividers, tabs inside each pane, a real terminal (libghostty-vt: truecolor,
ligatures, smooth scrolling, selection, mouse reporting), and a lot of configuration. No web view;
GTK CSS is only used for styling. It is a young project: expect to refactor.

## ⚠️ Rule: keep the configuration documentation up to date

**Whenever you add, rename, remove or change the meaning, type or default of ANY configuration
option, you MUST update the documentation in the same change.** The docs are exhaustive and are
treated as part of the feature:

1. **`docs/config/*.md`**: every option has a table row whose first column is the full dotted path in
   backticks (`` `panes.header.bell.duration` ``; `[]` = each list entry), with its type, default and
   description, in the page for that section (index: `docs/README.md`). Keep the *Default* YAML block
   of the section and the Nerd Font guidance (`docs/config/nerd-fonts.md`) correct too. Say whether the
   option applies live, **startup only**, or **new terminals only**.
2. **`examples/config.yaml`**: the worked example (VictorMono Nerd Font Propo in the terminal,
   Ubuntu Nerd Font in the chrome). Add the new option with a sensible value and a comment. A test
   loads it with the real types, rejecting unknown keys, so a typo or a removed option fails.
3. **`README.md`** if it is user-visible behaviour.
4. **`all_option_paths()`** in `src/config.rs` (tests) if you add a **new config struct**: register it
   there with its path prefix. The docs-coverage tests (`docs_cover_every_config_option`,
   `docs_do_not_describe_options_that_do_not_exist`) then fail until the docs match the code exactly,
   in both directions.
5. Run `cargo test`. A failing docs test means the docs are out of date: fix the docs, never the test.

Also: every config struct has `#[cfg_attr(test, serde(deny_unknown_fields))]` (release builds
ignore unknown keys on purpose so old and new files keep loading; test builds reject them so the
example and docs can't hide typos). Keep that attribute on new structs.

## ⚠️ Rule: keep THIRD-PARTY-NOTICES.md up to date

`THIRD-PARTY-NOTICES.md` reproduces the licences of everything compiled into the binary (the Rust
crates and libghostty-vt with its bundled simdutf, Highway, uucode and Zig runtime), as their licences
require. It is **generated, never edited by hand**:

```bash
PATH=$HOME/.local/zig:$PATH cargo build                    # fetches Ghostty's source and Zig packages
PATH=$HOME/.local/zig:$PATH python3 scripts/update-third-party-notices.py          # regenerate
PATH=$HOME/.local/zig:$PATH python3 scripts/update-third-party-notices.py --check  # is it current?
```

**Whenever you add, remove or update a dependency (`Cargo.toml` / `Cargo.lock`), or update
libghostty-vt, regenerate it and commit the result in the same change.** Run `--check` before every
release tag (the release tarball includes the file). If the script fails because a component's licence
can't be found, fix the script (`scripts/update-third-party-notices.py`) rather than leaving the file
stale. Do not add a dependency whose licence is not permissive (MIT / Apache-2.0 / BSD / ISC /
Unlicense / Zlib / BSL) without asking: Paua is MIT and ships a single binary.

## Build, run, test

```bash
PATH=$HOME/.local/zig:$PATH cargo run          # libghostty-vt needs Zig 0.15.2 on PATH to build
PATH=$HOME/.local/zig:$PATH cargo test         # ~90 unit tests; no display needed for most
cargo run -- setup | install | uninstall | --help   # CLI: first-run wizard; desktop entry + icons
```

System packages: `build-essential pkg-config libgtk-4-dev libadwaita-1-dev` (Ubuntu 24.04+).
`.cargo/config.toml` builds the Zig library as `ReleaseFast` even for debug builds: Zig's Debug mode
re-verifies page structures on every hyperlink operation, which made nushell's `ls` output get slower
every run. Do not remove it.

Releases: bump `version` in `Cargo.toml` (this updates `Cargo.lock`: commit both), run the notices `--check` (above), tag `vX.Y.Z`, push the tag; `.github/workflows/release.yml`
builds x86_64 and aarch64 tarballs, publishes the GitHub release and updates the Homebrew tap
(`packaging/homebrew/paua.rb.in`). CI is `.github/workflows/ci.yml`.

## Layout

| Path | What lives there |
|---|---|
| `src/main.rs` | Entry point, CLI (`install`, `uninstall`), accelerators, CSS and icon loading. `APP_ID = io.github.gregbacchus.Paua` |
| `src/app.rs` | `App`: windows, panes, splitting / closing / pop-out, focus, frame colours, palette items, menus, status-bar host (`run_action`), live config reload |
| `src/config.rs` | **All config types**, parsing (colours, durations, sizes, weights), VS Code colours, `.paua.yaml`, plus the config + docs tests |
| `src/terminal/` | The terminal widget `TermView` (libghostty-vt + portable-pty): `mod.rs` state / input / feed, `render.rs` (GSK snapshot, Pango runs, cairo nodes), `mouse.rs`, `scrollbar.rs`, `search.rs`, `input.rs`, `pty.rs`, `boxdraw.rs`, `osc133.rs` (semantic-prompt markers) |
| `src/header.rs` | The pane header: template, path replacement and trimming, badges, timer chip, bell flash |
| `src/statusbar.rs` | Status bar widgets, priority layout, background jobs; `src/git.rs` git status / branches / ops; `src/exec.rs` command runner with timeouts; `src/template.rs` templates |
| `src/fonts.rs` | `paua fonts` and the setup offer: downloads Nerd Fonts from the official release (pinned `NERD_FONTS_VERSION`), verifies SHA-256, installs with licence files. Nothing is bundled; never download without the user's consent |
| `src/themes.rs` | Terminal colour themes: built-ins, theme files (Ghostty + YAML), `terminal.colors`, and the `paua themes` CLI (`install` downloads the community collection) |
| `src/session.rs` | ssh / sudo / container / multiplexer / remote detection from `/proc` |
| `src/cmdtimer.rs` | Command-running timer state machine |
| `src/searchbox.rs`, `src/palette.rs` | Title-bar find box / command palette and fuzzy matching |
| `src/setup.rs` | `paua setup`: first-run wizard (finds Nerd Fonts, asks, writes a *minimal* config, offers `install`). Keep the config it writes minimal and valid |
| `src/icons.rs` | Icons built into the binary (split / pop-out symbolic icons, app icon), written to `~/.cache/paua/icons` at start-up so an installed binary finds them. New custom icons go in `assets/icons` **and** in `ACTION_ICONS` here |
| `src/install.rs` | `paua install` (desktop entry, icons built into the binary; offers `setup` when there is no config) |
| `src/frame.rs`, `src/style.css` | Animated frame colour; the stylesheet |
| `assets/icons/` | App icon (PNGs 16 to 512) and symbolic action icons |
| `docs/` | Configuration documentation (see the rule above) |
| `LICENSE`, `THIRD-PARTY-NOTICES.md`, `scripts/update-third-party-notices.py` | Paua's MIT licence; the generated licences of bundled third-party code (see the rule above) and its generator |
| `examples/` | `config.yaml` (full example), `paua.yaml` (per-directory example) |
| `packaging/homebrew/`, `.github/workflows/` | Homebrew formula template, CI and release |

## Conventions and gotchas

- **Rust edition 2024**: `gen` is a reserved word; use another name.
- **GTK is single-threaded.** Never block the main thread. Anything that can be slow (running a
  command, `git`, `/proc` scans, file checks) goes through `exec::spawn` / `exec::run` with a
  **hard timeout** (children are killed as a process group) and results are applied on the main
  thread. One in-flight job per widget; drop results from before a config reload (`epoch`).
- **Per-directory `.paua.yaml` may only set colours.** Never let a file inside a repository run
  commands (e.g. add status bar widgets) without an explicit trust mechanism.
- **Config is live-reloaded** (`App::apply_config`): when adding an option, make it apply on reload
  unless it is inherently startup-only, and document which.
- **Text selection / Nerd Font icons:** YAML `\u` escapes are one UTF-16 unit; glyphs above U+FFFF
  need `\U000F0109`. `normalize_surrogate_escapes` also accepts the `\U000F0109` pair form.
- **Rendering:** runs containing `()[]{}/\` use cached Cairo textures to avoid GTK glyph clipping.
  The texture fallback uses `gsk::CairoNode::new(bounds)` + `draw_context()` to keep positions
  correct under HiDPI scaling. See the [GTK glyph clipping workaround](README.md#gtk-glyph-clipping-workaround)
  for the upstream fix and the checks to run before removing this code.
- **Command timing** uses OSC 133 markers when the shell sends them (nushell does; shell builtins are
  then timed), else the pty's foreground process group.
- **libghostty-vt** quirks: the formatter drops leading and trailing blank rows (see
  `leading_blank_rows` in `search.rs`); `scrollbar().offset` semantics are noted at their use.
- **Testing the real app headlessly:** the established pattern is a *temporary* `PAUA_SELFTEST`
  block in `App::build` that types into a pane and prints state, run with a scratch
  `XDG_CONFIG_HOME`. Remove it afterwards: no debug hooks are committed. `render_sample` (ignored
  test) renders text to PNG; it reads `PAUA_*` env vars (`PAUA_PNG_DIR`, `PAUA_SCALE`, ...).
- Repository: the git profile is chosen from the **SSH** remote URL; an HTTPS remote gets no email.
- Style: prefer small modules with unit tests for pure logic (parsing, layout fitting, git output);
  commit messages end with the attribution line required by the session instructions.
