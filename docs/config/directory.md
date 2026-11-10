# Per-directory config

A directory can ask for its own window (frame) colour, so each project is recognisable at a glance.
The colour is looked up from the pane's working directory **up through its parents** and takes
effect while that pane is focused (it is only re-resolved when the directory changes or the pane is
focused).

## `.paua.yaml`

The nearest `.paua.yaml` above the working directory is used. Only the window colour can be set so
far. It is watched for changes (when `watch` is on), applies to the focused pane's window, and takes
precedence over the VS Code colour below.

| Key | Type | Default | Description |
|---|---|---|---|
| `window.color` | [themed colour](value-types.md#colours) | unset | Frame colour for this directory. |

```yaml
# .paua.yaml in a project directory
window:
  color: "#8a2b2b"                                  # or:
  # color: { dark: "#8a2b2b", light: "#c8445a" }
```

`.paua.yaml` files are deliberately limited to colours: a cloned repository must not be able to
change anything else (in particular it cannot add status bar widgets, which run commands).

## VS Code project colours

If the directory has no `.paua.yaml` colour, the nearest `.vscode/settings.json` (JSON with
comments and trailing commas is fine) is checked, so projects already coloured with the
[Peacock](https://marketplace.visualstudio.com/items?itemName=johnpapa.vscode-peacock) extension
just work. Which settings are tried, and whether this happens at all, is set in
[`window.vscode_colors` and `window.vscode_color_keys`](general.md#window). Each key is looked up
at the top level of the file (`"peacock.color": "#832561"`) and inside
`workbench.colorCustomizations` (`"titleBar.activeBackground"`); the first usable colour wins. The
colour may be `#rrggbb[aa]`, `#rgb`, `rgb(r, g, b)` or a basic name (see
[value types](value-types.md#colours)); alpha is ignored. The file is **not watched**: it is
re-read when you enter the directory and when the pane gets focus.
