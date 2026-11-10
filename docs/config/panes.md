# Panes: `panes`

Everything about the terminal panes: the space around the text, how unfocused panes look, and the
header line above each terminal (path, buttons, command timer, bell flash).

| Key | Type | Default | Description |
|---|---|---|---|
| `panes.padding` | [padding](value-types.md#padding) | `0` | Space between a pane's edge and its text, in px. The pane background fills it. |

```yaml
panes:
  padding: 0          # or 8, { x: 8, y: 4 }, { top: 4, left: 12 }
```

## `panes.inactive`

How panes other than the focused one look (only while a window has more than one pane). Every
value fades in over `fade_ms`, so the focused look is always the unmodified one. Values are
clamped to sane ranges, so a typo can't blank a pane.

| Key | Type | Default | Description |
|---|---|---|---|
| `panes.inactive.dim` | number 0–1 | `0.35` | Fade towards the pane background: 0 = none, 1 = fully hidden. |
| `panes.inactive.saturation` | number 0–4 | `1.0` | Colour saturation multiplier: 1 = unchanged, 0 = greyscale, 2 = double. |
| `panes.inactive.lightness` | number 0–4 | `1.0` | Lightness multiplier: 1 = unchanged, 0.8 = darker. |
| `panes.inactive.hue_shift` | number −360–360 | `0` | Hue rotation in degrees. |
| `panes.inactive.fade_ms` | integer | `150` | Transition time in milliseconds; 0 = instant. |

```yaml
panes:
  inactive: { dim: 0.35, saturation: 1.0, lightness: 1.0, hue_shift: 0, fade_ms: 150 }
  # A popular look: half dimmed and half desaturated
  # inactive: { dim: 0.5, saturation: 0.5 }
```

## `panes.header`

The line above each terminal: a text template, the copy / pop-out / close buttons, session badges,
the command timer chip and the bell flash.

| Key | Type | Default | Description |
|---|---|---|---|
| `panes.header.template` | [template](value-types.md#templates) | `{path}` | The text. Placeholders: `{path}` (working directory after `path_replacements`), `{dir}` (its last component), `{title}` (the title the program set, OSC 0/2), `{host}`, `{user}`, `{session}` (detected session, e.g. `ssh me@box`). `{title\|path}` = the title if there is one, else the path. |
| `panes.header.align` | `start` \| `center` \| `end` | `center` | Alignment of the text. Aliases: `left` = `start`, `right` = `end`. |
| `panes.header.path_replacements` | list | `[{ from: $HOME, to: "~" }]` | Rewrite the start of the path, or folder names anywhere in it. See below. |
| `panes.header.trim_path` | bool | `true` | When the text is too wide for the pane, drop whole path segments from the middle (`~/a/b/c/d` becomes `~/a/…/d`). |
| `panes.header.path_ellipsis` | string | `…` | What replaces the removed segments (`...` also works). |
| `panes.header.background` | [themed tint](value-types.md#colours) | unset | Tint laid over the pane background. `#rrggbbaa` for translucency. |
| `panes.header.color` | [themed tint](value-types.md#colours) | unset (normal text colour, dimmed) | Text and icon colour. |
| `panes.header.copy_icon` | [icon name](value-types.md#icon-names) | `copy-path-symbolic` | Icon of the *copy path* button. The built-in default shows two overlapping sheets. Applies live. |
| `panes.header.pop_out_icon` | [icon name](value-types.md#icon-names) | `pop-out-symbolic` | Icon of the *open in a new window* button (also `pop-out-pip-symbolic`). |
| `panes.header.close_icon` | [icon name](value-types.md#icon-names) | `window-close-symbolic` | Icon of the *close pane* button. |

A session (ssh, sudo...) can also tint the header line: see [sessions](session.md).

```yaml
panes:
  header:
    template: "{path}"
    align: center
    path_replacements:
      - { from: $HOME, to: "~" }
    trim_path: true
    path_ellipsis: "…"
    # background: { dark: "#ffffff14", light: "#00000010" }
    # color: { dark: "#c8c8d0", light: "#444450" }
    copy_icon: copy-path-symbolic
    pop_out_icon: pop-out-symbolic
    close_icon: window-close-symbolic
```

### `panes.header.font`

| Key | Type | Default | Description |
|---|---|---|---|
| `panes.header.font.family` | string | unset (the terminal font) | Font family of the header text. Unset keeps Nerd Font icons working with no extra setup. |
| `panes.header.font.size` | number | `9.5` | Points. |
| `panes.header.font.weight` | [font weight](value-types.md#font-weight) | unset (normal) | Weight. |

```yaml
panes:
  header:
    font: { family: Ubuntu Nerd Font, size: 9, weight: normal }
```

Nerd Font: use a proportional `Nerd Font` (e.g. `Ubuntu Nerd Font`) at 9–10 pt, or leave `family`
unset. See [Nerd Fonts](nerd-fonts.md).

### `panes.header.path_replacements[]`

A list of plain-text replacements (not regular expressions). Matching is always by **whole path
components** (`$HOME` doesn't match `/home/gregbacchus2`, and `Downloads` doesn't match
`MyDownloads`). There are two kinds, told apart by `from`:

- **Anchored: `from` starts with `/`** (`/mnt/data/code`). `~` and `$HOME` are your home directory, so
  `$HOME` and `$HOME/code` count too. It matches the **start** of the path, **the first one that
  matches wins**, and the rest of the path follows it.
- **Anywhere:** anything that doesn't start with `/` (`Downloads`, `node_modules`, `src/terminal`). Those components are
  replaced wherever they occur in the part of the path that is left after the anchored replacement,
  every time they occur; where several would match at the same place the first listed wins.

Replaced text is never matched again, so `$HOME/Downloads` with the two replacements below shows as
`house/download`, not a re-replaced mess.

| Key | Type | Default | Description |
|---|---|---|---|
| `panes.header.path_replacements[].from` | string | required | What to replace. If it starts with `/` (or is `~` / `$HOME`, your home directory) it matches the start of the path; otherwise, folder name(s) such as `Downloads` or `src/terminal` match anywhere. `~` and `$HOME` mean your home directory. |
| `panes.header.path_replacements[].to` | string | required | What to show instead. |

```yaml
panes:
  header:
    path_replacements:
      - { from: $HOME,      to: "\uf015" }   # nf-fa-home: /home/me/... starts with the house
      - { from: Downloads,  to: "\uf019" }   # nf-fa-download: wherever a "Downloads" folder is
      - { from: "$HOME/code", to: "\uf1c9" } # (anchored: would have to come before "$HOME" to win)
```

With the first two, `/home/me/Downloads` shows as the house icon, a `/`, then the download icon, and
`/mnt/backup/Downloads/2024` as `/mnt/backup/<download icon>/2024`. Among the **anchored** ones list the
more specific first (`$HOME/code` before `$HOME`). Use a Nerd Font icon as the replacement for a
compact header: `"\uf015"` for home. Quote strings containing `$` and escapes with double quotes.

### `panes.header.command_timer`

A chip in the header that counts up while a command runs. Once a command has run for `after` the
chip appears (`12s`, `2m 05s`, `1h 02m 03s`); when it ends it briefly shows `took 12s` for
`linger`, or `failed (exit 1) after 12s` for a failure.

How it detects commands: if the shell sends **OSC 133** semantic-prompt markers (nushell does by
default; bash, zsh and fish need a shell-integration snippet), the timer is exact, shell builtins
(like `sleep 10sec` in nushell) are timed and the exit status is known. Otherwise a command counts
as running while a foreground process other than the shell owns the pty, which covers external
programs only. Full-screen programs on the alternate screen (vim, less, htop) are not timed.

| Key | Type | Default | Description |
|---|---|---|---|
| `panes.header.command_timer.enabled` | bool | `true` | Turn the chip off or on. |
| `panes.header.command_timer.position` | `left` \| `right` | `right` | `left` = before the text and badges, `right` = next to the buttons. Aliases: `start`, `end`. |
| `panes.header.command_timer.after` | [duration](value-types.md#duration) | `2s` | Show the chip once a command has run this long. |
| `panes.header.command_timer.linger` | [duration](value-types.md#duration) | `4s` | How long `took 12s` stays after a long command ends. `0` = don't show it. |
| `panes.header.command_timer.ignore_alt_screen` | bool | `true` | Don't time full-screen programs (vim, less, htop...). |
| `panes.header.command_timer.ignore` | list of strings | `[ssh, mosh, mosh-client, tmux, screen, zellij]` | Program names that are never timed. Remove `ssh` to time ssh sessions. Applies to external programs (not to builtins). |
| `panes.header.command_timer.show_exit_status` | bool | `true` | Say `failed (exit 1) after 3s` after a failure. Needs OSC 133. |
| `panes.header.command_timer.icon` | string | `""` | Put before the text, e.g. a stopwatch glyph. |
| `panes.header.command_timer.icon_gap` | number (px) | `5` | Extra space after the icon. Raise it (6–10) if a wide Nerd Font glyph touches the time. |
| `panes.header.command_timer.background` | [themed tint](value-types.md#colours) | unset (amber, built in) | Chip background while running. |
| `panes.header.command_timer.color` | [themed tint](value-types.md#colours) | unset (built in) | Chip text while running. |
| `panes.header.command_timer.done_background` | [themed tint](value-types.md#colours) | unset (grey, built in) | Background after it finished. |
| `panes.header.command_timer.done_color` | [themed tint](value-types.md#colours) | unset (built in) | Text after it finished. |
| `panes.header.command_timer.failed_background` | [themed tint](value-types.md#colours) | unset (red, built in) | Background after a failure. |
| `panes.header.command_timer.failed_color` | [themed tint](value-types.md#colours) | unset (built in) | Text after a failure. |

```yaml
panes:
  header:
    command_timer:
      enabled: true
      position: right
      after: 2s
      linger: 4s
      ignore_alt_screen: true
      ignore: [ssh, mosh, mosh-client, tmux, screen, zellij]
      show_exit_status: true
      icon: ""
      icon_gap: 5
      # background: "#ffb02e33"   # amber while running
      # color: "#ffc069"
      # done_background: "#80808038"
      # done_color: "#c8c8d0"
      # failed_background: "#ff525240"
      # failed_color: "#ff9a9a"
```

Nerd Font: `icon: "\uf017 "` (clock) or a Material Design timer glyph such as `"\U000F13AB"`, with
`icon_gap: 6`–`10`. See [Nerd Fonts](nerd-fonts.md).

### `panes.header.bell`

Flash the header when a program rings the terminal bell (BEL, e.g. `printf '\a'`). In nushell:
`print (char bel)`.

| Key | Type | Default | Description |
|---|---|---|---|
| `panes.header.bell.enabled` | bool | `true` | Turn the flash off or on. |
| `panes.header.bell.duration` | [duration](value-types.md#duration) | `500ms` | How long the flash takes to fade out. |
| `panes.header.bell.color` | [themed tint](value-types.md#colours) | unset (soft white in dark mode, soft black in light mode) | Flash colour. |

```yaml
panes:
  header:
    bell:
      enabled: true
      duration: 500ms
      # color: "#ffffff52"     # or { dark: "#ffffff52", light: "#00000029" }
```
