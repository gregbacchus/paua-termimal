# Terminal colour themes: `terminal.theme`, `terminal.colors`

A theme sets the terminal's **text and background colours, the cursor, the selection and the 16
ANSI colours** (the palette programs use for `ls --color`, prompts, syntax highlighting...). The
pane card and the padding around the text take the theme's background, so the whole pane matches.
The window frame (title bar, status bar, borders) keeps its own colour: see [`window`](general.md#window).

You can pick one theme for both colour schemes, or a different one for **light** and **dark**; the
terminal switches when the colour scheme does (View → Cycle Colour Scheme, or the system setting).

| Key | Type | Default | Description |
|---|---|---|---|
| `terminal.theme` | theme name, or `{ dark, light }` | `paua` | The theme. A name applies to both colour schemes. A map sets the **dark** and/or the **light** theme; a mode you leave out uses `paua`. |
| `terminal.colors` | map | none | Colours that adjust the theme (below). |

```yaml
terminal:
  theme: nord                        # one theme for both
```

```yaml
terminal:
  theme:                             # a different theme per colour scheme
    dark: Catppuccin Mocha
    light: Catppuccin Latte
```

```yaml
terminal:
  theme: { dark: tokyo-night }       # light mode keeps the default (paua) theme
```

## Where theme names come from

Names are matched ignoring case (`catppuccin mocha` finds `Catppuccin Mocha`). The first place that
has the name wins:

1. **Built in**: `paua` (the default, the app's own look with the standard palette), `catppuccin`,
   `dracula`, `gruvbox`, `nord`, `one`, `solarized`, `tokyo-night`. Families with both variants pick
   the right one for the colour scheme (`solarized` is `solarized-dark` in dark mode and
   `solarized-light` in light mode; `catppuccin` is mocha / latte); you can also name a variant
   (`gruvbox-light`, `one-dark`, `catppuccin-latte`...).
2. **Your own files** in `~/.config/paua/themes/`.
3. **The installed collection** in `~/.local/share/paua/themes/`: about **900 community themes**
   (the [iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes) collection, also
   used by Ghostty), installed with `paua themes install` (see below).
4. **Ghostty's themes**: `~/.config/ghostty/themes/` and the `ghostty/themes` folder of any system
   data directory (for example `/usr/share/ghostty/themes`), so an existing Ghostty installation's
   themes work as they are.

An unknown name shows a *Config error* toast listing what is available and keeps the default theme.
Theme names are file names, never paths.

## The theme collection: `paua themes`

```bash
paua themes install            # download ~900 themes (about 4 MB; needs git and network)
paua themes list               # built-in and installed themes
paua themes list nord          # only names containing "nord"
paua themes preview "Dracula+"            # colour swatches in your terminal
paua themes preview "Solarized Light" --light
```

`paua themes install` runs `git clone` of the collection's repository (sparse, only the Ghostty
format folder) into a temporary folder, copies the files to `~/.local/share/paua/themes/` with its
licence (MIT) and removes the temporary folder. Run it again to update. After that, use any of the
names `paua themes list` shows:

```yaml
terminal:
  theme: { dark: "Tokyo Night", light: "Tokyo Night Day" }
```

## Your own theme files

Put a file in `~/.config/paua/themes/`. Two formats are understood:

- **Ghostty format** (any file name without a `.yaml` / `.yml` extension): `key = value` lines.
  Understood keys: `background`, `foreground`, `cursor-color`, `selection-background` and
  `palette = N=#rrggbb`. `#` starts a comment; unknown keys are ignored. Thousands of themes are
  published in this format, so you can drop them in as they are.
- **YAML** (`name.yaml`): the same keys as `terminal.colors` below.

```
# ~/.config/paua/themes/Midnight
background = #0b0e14
foreground = #bfbdb6
cursor-color = #e6b450
selection-background = #273747
palette = 0=#01060e
palette = 1=#ea6c73
palette = 2=#91b362
```

```yaml
# ~/.config/paua/themes/midnight.yaml
background: "#0b0e14"
foreground: "#bfbdb6"
cursor: "#e6b450"
palette: { 0: "#01060e", 1: "#ea6c73", 2: "#91b362" }
```

Entries a theme file doesn't set keep the default theme's value (and the standard palette).

## `terminal.colors`

Adjust the chosen theme, or set colours without a theme. Every colour is a
[themed colour](value-types.md#colours): one value, or `{ dark, light }`. Colours set here win over
the theme.

| Key | Type | Default | Description |
|---|---|---|---|
| `terminal.colors.foreground` | [themed colour](value-types.md#colours) | the theme's | Default text colour. |
| `terminal.colors.background` | [themed colour](value-types.md#colours) | the theme's | Default background (also the pane card and padding). |
| `terminal.colors.cursor` | [themed colour](value-types.md#colours) | the theme's (the text colour) | Cursor colour. |
| `terminal.colors.selection_background` | [themed colour](value-types.md#colours) | the theme's (a blend of text into background) | Selected text background. |
| `terminal.colors.palette` | map of index to [themed colour](value-types.md#colours) | the theme's | Palette entries by number: 0 to 7 are black, red, green, yellow, blue, magenta, cyan, white; 8 to 15 their bright versions; 16 to 255 the extended 256-colour palette. |

```yaml
terminal:
  theme: nord
  colors:
    background: "#1b1f27"                       # a darker background than Nord's
    cursor: { dark: "#ffcc00", light: "#aa6600" }
    palette: { 1: "#ff6b6b", 9: "#ff8787" }     # a brighter red
```

## Good values and tips

- **Pair themes of the same family** for dark and light (`Catppuccin Mocha` / `Catppuccin Latte`,
  `Tokyo Night` / `Tokyo Night Day`, `Solarized Dark` / `Solarized Light`): a family name such as
  `solarized` does this for you.
- **Nerd Font icons and themes are independent**: themes only colour text, so any theme works with
  the fonts in [Nerd Fonts](nerd-fonts.md).
- **Contrast**: the *inactive pane* dimming (`panes.inactive`) fades towards the theme's
  background, and bright-black (palette 8) is what many prompts use for dim text: avoid themes
  where it is nearly the background colour.
- **Colour 256-palette entries 16 to 255** are not part of themes; they use the standard
  xterm colours unless you set them in `terminal.colors.palette`.
- `paua themes preview <name>` shows how a theme looks before you commit to it.
