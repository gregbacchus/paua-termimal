# Value types

## Colours

Hex colours are `#rrggbb` or `#rgb` (the `#` is optional). Which form an option takes is shown in
its *Type* column.

| Type | Accepts | Notes |
|---|---|---|
| **Colour** | `"#rrggbb"`, `"#rgb"` | Opaque. |
| **Tint** | `"#rrggbb"`, `"#rrggbbaa"`, `"#rgb"`, `"#rgba"` | The last pair of digits (or digit) is the alpha: `#ffffff14` is white at about 8%. Used for overlays, chips and highlights. |
| **Themed colour** | a Colour, or `{ dark: Colour, light: Colour }` | Different colour for the dark and light colour scheme. A single value applies to both; if only one of `dark` / `light` is given the other mode uses it too. |
| **Themed tint** | a Tint, or `{ dark: Tint, light: Tint }` | As above, with alpha. |

Quote hex colours in YAML (`"#4a2f80"`), otherwise `#` starts a comment.

The VS Code colour keys (see [per-directory config](directory.md)) additionally accept
`rgb(r, g, b)` / `rgba(...)`, bare hex without `#`, and the names `red green blue orange yellow
purple pink teal cyan brown black white gray grey`.

## Duration

A number of seconds (`2`, `1.5`), or a number with a unit: `500ms`, `2s`, `1.5m`, `1h`. `0` is
allowed (the option says what it means). The maximum is 24 hours.

Options named `*_ms` (`fade_ms`, `idle_ms`, `poll_ms`) are plain integers in milliseconds instead.

## Size

A length for the window: `1100` or `1100px` (pixels), or `80%` (of the monitor). Must be positive.

## Font weight

`100`–`1000`, or a name (case-insensitive; `-`, `_` and spaces are ignored):

| Name | Weight | Name | Weight |
|---|---|---|---|
| `thin`, `hairline` | 100 | `semibold`, `demibold` | 600 |
| `extralight`, `ultralight` | 200 | `bold` | 700 |
| `light` | 300 | `extrabold`, `ultrabold` | 800 |
| `semilight` | 350 | `heavy`, `black` | 900 |
| `book` | 380 | `ultraheavy`, `extrablack` | 1000 |
| `normal`, `regular` | 400 | `medium` | 500 |

Variable fonts honour any number; static fonts snap to the nearest installed weight.

## Padding

A number (all four sides, in px), or a map with any of `x`, `y`, `top`, `right`, `bottom`, `left`
(`top`/`bottom` default to `y`, `left`/`right` default to `x`, everything else to 0). Each value
must be 0–200.

```yaml
padding: 8
padding: { x: 8, y: 4 }
padding: { top: 4, left: 12 }
```

## Case mode

`smart` (case-insensitive unless the query has an uppercase letter), `sensitive`, `insensitive`.

## Scrollbar mode

`auto` (fades in while scrolling or when the pointer is at the right edge), `always`, `never`.

## Templates

Pane headers, session labels and status bar widgets use small text templates:

| Syntax | Meaning |
|---|---|
| `{name}` | The value of `name` (empty if it doesn't exist). The names available depend on where the template is used. |
| `{a\|b\|c}` | The first of these that is not empty. |
| `{?name:text}` | `text`, only if `name` is set (not empty, not `0`, not `false`). `text` can contain more placeholders. Status bar only. |
| `{!name:text}` | The opposite: `text` only if `name` is not set. Status bar only. |
| `{{` and `}}` | A literal `{` / `}`. |

An unbalanced `{` is kept as plain text.

## YAML escapes and icons

Icons are usually Nerd Font glyphs. In a double-quoted YAML string write them as escapes:

- `""` for a codepoint up to U+FFFF,
- `"\U000F0109"` (capital `U`, eight hex digits) above U+FFFF, which covers most Material Design
  icons,
- a UTF-16 pair as found in icon cheat sheets and JSON (`"󰄉"`) is also accepted.

Single-quoted and plain YAML strings do not process escapes.

## Icon names

Options such as `copy_icon` take a **theme icon name** (any icon in the current GTK icon theme,
e.g. `edit-copy-symbolic`) or one of ours: `pop-out-symbolic`, `pop-out-pip-symbolic`,
`split-right-symbolic`, `split-down-symbolic`.
