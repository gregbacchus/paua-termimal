# General: `watch`, `font`, `window`

## `watch`

| Key | Type | Default | Description |
|---|---|---|---|
| `watch` | bool | `true` | Reload `config.yaml` (and the focused pane's `.paua.yaml`) when the file changes on disk. If either is a symbolic link (a dotfiles repository, stow...), the link, any links it passes through and the real file are all watched, so editing the real file reloads too. With `false`, use *Reload Config* (File menu / palette) to re-read. The change itself is picked up live. |

```yaml
watch: true
```

## `font`

The terminal font. Fonts for the pane header and status bar are set separately
(`panes.header.font`, `status_bar.font`). See [Nerd Fonts](nerd-fonts.md) for which variant to use.

| Key | Type | Default | Description |
|---|---|---|---|
| `font.family` | string | `VictorMono Nerd Font Propo` | Font family name, as `fc-list` shows it. Fonts with ligatures render them. |
| `font.size` | number | `11` | Size in points. |
| `font.weight` | [font weight](value-types.md#font-weight) | unset (400) | Weight of regular text. |
| `font.bold_weight` | [font weight](value-types.md#font-weight) | unset (700, or 200 heavier than `weight` if that is higher) | Weight of bold text. |
| `font.features` | string | unset (font defaults) | OpenType features, comma separated (HarfBuzz / CSS `font-feature-settings` syntax): `"-calt"` or `"calt=0"` switches contextual ligatures off, `"ss01"` enables a stylistic set, `"-liga, -calt"` turns off several. |

Default and a typical Nerd Font setup:

```yaml
font:
  family: VictorMono Nerd Font Propo
  size: 11
  # weight: normal
  # bold_weight: bold
  # features: "-calt"
```

Good values with a Nerd Font: a `... Propo` or `... Mono` family, size 10–12; keep `features` unset
unless you want ligatures off.

## `window`

The window frame is the title bar, the status bar and the borders between panes; it is drawn in one
colour. [Precedence of colours](README.md#window-colour-precedence).

| Key | Type | Default | Description |
|---|---|---|---|
| `window.maximized` | bool | `false` | Start maximised. **Startup only.** |
| `window.width` | [size](value-types.md#size) | `1100` | Initial width. **Startup only.** Percentages are of the monitor. |
| `window.height` | [size](value-types.md#size) | `700` | Initial height. **Startup only.** |
| `window.color` | [themed colour](value-types.md#colours) | unset (`#4a2f80` dark / `#7a56c2` light) | The default frame colour. |
| `window.inactive_dim` | number 0–1 | `0.12` | How much the frame darkens while the window is not focused: 0 = not at all, 1 = black. |
| `window.vscode_colors` | bool | `true` | Also take a directory's colour from the nearest `.vscode/settings.json` (Peacock etc.). A `.paua.yaml` colour wins over it. Re-read when you enter a directory and on focus; the file isn't watched. |
| `window.vscode_color_keys` | list of strings | `[peacock.color, titleBar.activeBackground, statusBar.background]` | Settings to try, in order. Each key is looked up at the top level of the file (`peacock.color`) and inside `workbench.colorCustomizations` (`titleBar.activeBackground`). |

```yaml
window:
  maximized: false
  width: 1100        # 1100, 1100px or 80%
  height: 700
  # color: { dark: "#4a2f80", light: "#7a56c2" }
  inactive_dim: 0.12
  vscode_colors: true
  vscode_color_keys: [peacock.color, titleBar.activeBackground, statusBar.background]
```

Good values: `width: 60%` / `height: 80%` for a centred, resizable layout on large monitors; a
mid-saturation `color` (the default purple is `#4a2f80`) keeps white chrome text readable.
