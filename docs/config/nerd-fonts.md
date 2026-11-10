# Nerd Fonts: choosing fonts, icons and good values

Paua works with any font, but icons in pane headers, the command timer, session badges, git
status and status bar widgets are most useful with a [Nerd Font](https://www.nerdfonts.com/)
(a normal font patched with thousands of icon glyphs). The example config uses
**VictorMono Nerd Font Propo** in the terminal and **Ubuntu Nerd Font** in the chrome.

`paua setup` does the font part for you, including **downloading** them: if VictorMono, Ubuntu and the
Symbols-only icon font aren't installed it offers to fetch them from the official Nerd Fonts release
(checked against its SHA-256 sums, installed with their licence files in
`~/.local/share/fonts/NerdFonts/`), and `paua fonts list` / `paua fonts install <name>...` /
`paua fonts uninstall <name>...` manage them yourself (JetBrainsMono, FiraCode, Hack and more).
The Symbols-only font supplies the icons for any other font, so even a plain font shows them.
It also lists the Nerd Fonts it finds (`fc-list`), suggests
VictorMono Nerd Font Propo for the terminal and Ubuntu Nerd Font for the decorations when they are
installed (otherwise the first suitable family), and writes your choices to the config. Run it again
after installing fonts. The rest of this page explains the choices.

## Which variant

Each Nerd Font ships in three flavours:

| Family name ends in | Glyph widths | Use it for |
|---|---|---|
| `Nerd Font Propo` | Icons keep their natural width | **The terminal.** Text lines up on the character grid, icons are drawn at full size. A very wide icon can touch the next character: leave a space after it. |
| `Nerd Font Mono` | Every icon squeezed into one cell | The terminal, if you want icons strictly one cell wide (they come out smaller). |
| `Nerd Font` (no suffix) | Icons may be wider than one cell | Proportional UI text: the pane header, status bar. In the terminal it can misalign prompts. |

Install the fonts for your user (`~/.local/share/fonts`) and check the exact family name with
`fc-list | grep -i "victor"`. Use that name (without the style) in `font.family`,
`panes.header.font.family` and `status_bar.font.family`. Use a **version 3** Nerd Font: v2
placed the Material Design icons (`nf-md-*`) at different codepoints, so glyph numbers on the
current cheat sheet don't match old fonts.

## Good values

| Option | Good value with a Nerd Font | Why |
|---|---|---|
| `font.family` | `VictorMono Nerd Font Propo` (any `... Propo` / `... Mono`) | Terminal text and icons from one font; the Propo / Mono variants keep the grid. |
| `font.size` | `10`–`12` | Points. Icons are scaled with the text. |
| `font.features` | unset, or `"-calt"` | Leave ligatures on unless a font's ligatures bother you. |
| `panes.header.font.family` | `Ubuntu Nerd Font` (or leave unset to use the terminal font) | A proportional UI font looks better for a path; leaving it unset keeps icons working with no extra setup. |
| `panes.header.font.size` | `9`–`10` | Smaller than the terminal text so the header recedes. |
| `status_bar.font.family` | the same Nerd Font as the header | Otherwise icons come from a fallback font and may be missing. |
| `status_bar.font.size` | `9`–`10` | Unset gives 0.9 × the UI font. |
| `status_bar.font.weight` | `light`–`normal` | Needs that weight installed; static fonts snap to the nearest one. |
| `panes.header.command_timer.icon_gap` | `6`–`10` (default 5) | Wide glyphs overflow their advance and overlap the time; raise this until they clear. |
| icons with a trailing space | `"\uf07c "` | A space after an icon is another way to get room (the status bar's `git.icons.*` and `widgets[].icon` use it as written). |
| `status_bar.git.icon` | `"\ue0a0"` or `"\uf418"` | A branch glyph before the branch name. Combine glyphs freely: `"\U000F02A2 \uf418"`. |

## Writing the glyphs

Write icons as escapes in **double-quoted** strings (see [value types](value-types.md#yaml-escapes-and-icons)):
`"\uf015"` up to U+FFFF, `"\U000F0109"` above it. Pasting the glyph itself also works, but escapes
survive editors and fonts that can't display them.

Find codepoints in the [cheat sheet](https://www.nerdfonts.com/cheat-sheet): the search box takes
names like `home`, `git branch` or `clock`, and each result shows its hex code.

Codepoints that are handy here (Nerd Fonts v3):

| Glyph | Name | Write as | Good for |
|---|---|---|---|
| nf-fa-home | home | `"\uf015"` | `panes.header.path_replacements` for `$HOME` |
| nf-fa-folder_open | open folder | `"\uf07c"` | `dirs` widget, header |
| nf-fa-download | download | `"\uf019"` | replacing `~/Downloads` |
| nf-fa-file_code_o | code file | `"\uf1c9"` | replacing `~/code` |
| nf-fa-clock_o | clock | `"\uf017"` | `clock` widget, command timer |
| nf-fa-link | link | `"\uf0c1"` | `session.ssh.icon` |
| nf-fa-lock | lock | `"\uf023"` | `session.root.icon`, `session.sudo.icon` |
| nf-fa-server | server | `"\uf233"` | `session.remote.icon` |
| nf-fa-terminal | terminal | `"\uf120"` | `session.multiplexer.icon` |
| nf-dev-git_branch | branch | `"\ue725"` | `status_bar.git.icon` |
| nf-pl-branch | branch (powerline) | `"\ue0a0"` | `status_bar.git.icon` |
| nf-oct-git_branch | branch | `"\uf418"` | `status_bar.git.icon` |
| nf-fa-refresh | refresh | `"\uf021"` | `status_bar.git.sync_icon` |
| nf-fa-pencil | pencil | `"\uf040"` | `git.icons.modified` |
| nf-fa-trash | trash | `"\uf1f8"` | `git.icons.deleted` |
| nf-fa-question | question | `"\uf128"` | `git.icons.untracked` |
| nf-fa-archive | archive | `"\uf187"` | `git.icons.stashed` |
| nf-fa-eraser | eraser | `"\uf12d"` | a "clear terminal" button |
| nf-dev-nodejs_small | Node.js | `"\ue718"` | a `node` widget |
| nf-dev-rust | Rust | `"\ue7a8"` | a `rust` widget |
| nf-dev-docker | Docker | `"\ue7b0"` | a Docker widget |
| nf-seti-typescript | TypeScript | `"\ue628"` | a TypeScript widget |

If you see empty boxes (or nothing) instead of an icon, the font used by that part of the UI
doesn't contain it: set `status_bar.font.family` / `panes.header.font.family` to your Nerd Font, or
check it is installed (`fc-list | grep -i nerd`). Emoji (for example `"🚨"`) come from the
system's colour emoji font and are not affected by these settings.
