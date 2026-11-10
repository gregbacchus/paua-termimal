# Terminal: `terminal`

| Key | Type | Default | Description |
|---|---|---|---|
| `terminal.scrollback` | integer | `10000` | Lines of scrollback per terminal. **New terminals only** (panes created after the change). |
| `terminal.shell` | string (path) | unset (`$SHELL`) | The program to run in a new pane, e.g. `/usr/local/bin/nu`. **New terminals only.** |

Colours: the terminal's colour theme (`terminal.theme`) and its adjustments (`terminal.colors`) are on
the [themes page](themes.md).

```yaml
terminal:
  scrollback: 10000
  # shell: /usr/bin/nu
```

## `terminal.scrollbar`

A thin overlay scrollbar on the right edge of each pane. It only exists while there is scrollback.
It can be written as just a mode (`scrollbar: always`) or as a map with any of the keys below.

| Key | Type | Default | Description |
|---|---|---|---|
| `terminal.scrollbar.mode` | `auto` \| `always` \| `never` | `auto` | `auto` fades in while scrolling or when the pointer is at the right edge and fades out when idle. |
| `terminal.scrollbar.width` | number (px) | `4` | Thumb width at rest (1–40). |
| `terminal.scrollbar.hover_width` | number (px) | `8` | Thumb width when the pointer is near (1–40). |
| `terminal.scrollbar.margin` | number (px) | `3` | Gap between the thumb and the pane's right edge (0–40). |
| `terminal.scrollbar.min_thumb` | number (px) | `28` | Smallest thumb height (4–400). |
| `terminal.scrollbar.color` | [themed tint](value-types.md#colours) | unset (the pane's text colour) | Thumb colour. |
| `terminal.scrollbar.opacity` | number 0–1 | `0.32` | Thumb opacity at rest. |
| `terminal.scrollbar.hover_opacity` | number 0–1 | `0.55` | Thumb opacity when hovered. |
| `terminal.scrollbar.track_opacity` | number 0–1 | `0.08` | Opacity of the track shown while hovered. |
| `terminal.scrollbar.idle_ms` | integer (ms) | `1200` | How long it stays visible after scrolling (`auto` mode; max 60000). |
| `terminal.scrollbar.fade_ms` | integer (ms) | `200` | Fade time (max 5000). |

```yaml
terminal:
  scrollbar:
    mode: auto
    width: 4
    hover_width: 8
    margin: 3
    min_thumb: 28
    # color: { dark: "#88c0ffcc", light: "#2050a0" }
    opacity: 0.32
    hover_opacity: 0.55
    track_opacity: 0.08
    idle_ms: 1200
    fade_ms: 200
```

## `terminal.scroll_to_bottom_button`

A small round button at the bottom right that appears while you are scrolled up and jumps back to
the latest output. Write `scroll_to_bottom_button: false` to disable it, or a map to style it.

| Key | Type | Default | Description |
|---|---|---|---|
| `terminal.scroll_to_bottom_button.enabled` | bool | `true` | Show the button. |
| `terminal.scroll_to_bottom_button.size` | number (px) | `28` | Diameter (12–80). |
| `terminal.scroll_to_bottom_button.margin_right` | number (px) | `20` | Gap from the pane's right edge (0–200). |
| `terminal.scroll_to_bottom_button.margin_bottom` | number (px) | `14` | Gap from the pane's bottom edge (0–200). |
| `terminal.scroll_to_bottom_button.color` | [themed tint](value-types.md#colours) | unset (the pane's text colour) | Circle and chevron colour. |
| `terminal.scroll_to_bottom_button.background_opacity` | number 0–1 | `0.14` | Circle fill opacity. |
| `terminal.scroll_to_bottom_button.hover_background_opacity` | number 0–1 | `0.28` | Circle fill opacity when hovered. |
| `terminal.scroll_to_bottom_button.icon_opacity` | number 0–1 | `0.7` | Chevron opacity. |
| `terminal.scroll_to_bottom_button.hover_icon_opacity` | number 0–1 | `1.0` | Chevron opacity when hovered. |

```yaml
terminal:
  scroll_to_bottom_button:
    enabled: true
    size: 28
    margin_right: 20
    margin_bottom: 14
    # color: "#ffffff"
    background_opacity: 0.14
    hover_background_opacity: 0.28
    icon_opacity: 0.7
    hover_icon_opacity: 1.0
```

## `terminal.urls`

Following links in the output: hold the **modifier** (Ctrl by default) and the link under the pointer
is underlined and the pointer becomes a hand; **modifier + click** opens it in your default
application (a browser for `https://`, your mail client for `mailto:`, the file manager or default
app for `file://`). Two kinds of link are recognised:

- **Plain URLs** in the text, including ones that wrap over several lines. Trailing punctuation
  that belongs to the sentence (`.`, `,`, a closing bracket without an opening one...) is left out.
- **Explicit hyperlinks** (OSC 8) that programs emit, such as the file names in nushell's `ls`.

Only links whose scheme is in `schemes` are underlined or opened. While a program has mouse
tracking enabled (vim, htop...) the click goes to the program, so use `ctrl_shift` (Shift forces a
local click) or hold **Ctrl+Shift** with the default `ctrl`.

| Key | Type | Default | Description |
|---|---|---|---|
| `terminal.urls.enabled` | bool | `true` | Turn link detection and opening off or on. |
| `terminal.urls.modifier` | `ctrl` \| `alt` \| `ctrl_shift` | `ctrl` | The key held to follow links. |
| `terminal.urls.schemes` | list of strings | `[http, https, ftp, ftps, file, mailto]` | URL schemes that are detected and opened. Add `ssh` or `git` to follow those too; a program can't make Paua open a scheme that isn't listed. |
| `terminal.urls.osc8` | bool | `true` | Also follow explicit OSC 8 hyperlinks. |
| `terminal.urls.color` | [themed tint](value-types.md#colours) | unset (the text colour) | Colour of the underline while hovering. |

```yaml
terminal:
  urls:
    enabled: true
    modifier: ctrl          # ctrl | alt | ctrl_shift
    schemes: [http, https, ftp, ftps, file, mailto]
    osc8: true
    # color: { dark: "#8db8ff", light: "#1d4f91" }
```
