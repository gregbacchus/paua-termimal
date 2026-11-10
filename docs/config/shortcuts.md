# Keyboard shortcuts

Press **F1** by default or open **Help → Keyboard Shortcuts** to see the keys in effect, including
unassigned actions and shortcuts that only work while editing Find or the Command Palette.

| Key | Type | Default | Description |
|---|---|---|---|
| `shortcuts` | map of action ID to string or list of strings | `{}` (use the defaults below) | Change Paua's bindings in every window. Applies live. |

Use one key combination or a YAML list for alternatives. An action you leave out keeps its
defaults. A listed action replaces all of its defaults. Set it to `[]` to clear the binding.

Write combinations like `Ctrl+Shift+C`, `Alt+Page Down`, or `F1`. GTK notation such as
`"<Control><Shift>c"` also works. Use GTK key names for punctuation such as `plus` or `semicolon`.

An explicit binding takes precedence over a default using the same key in the same context. Two
explicit bindings for the same key in overlapping contexts are a config error. Find and the
Command Palette can each use the same key for different actions because they are separate modes.
An application-wide binding can intercept a key that would otherwise go to the shell or a text field.

The default map is empty:

```yaml
shortcuts: {}
```

For example:

```yaml
shortcuts:
  keyboard_shortcuts: F1
  new_tab: [Ctrl+Shift+T, Alt+T]
  close_tab: []
  split_right: Ctrl+Shift+E
  "type:git status": Ctrl+Alt+G
```

## Window and terminal actions

| Action ID | Action | Default |
|---|---|---|
| `new_window` | Open a window in the current directory | Ctrl+Shift+N |
| `new_tab` | Open a tab in the focused pane | Ctrl+Shift+T |
| `close_tab` | Close the selected tab | Ctrl+Shift+W |
| `previous_tab` | Select the previous tab | Ctrl+Shift+Page Up |
| `next_tab` | Select the next tab | Ctrl+Shift+Page Down |
| `split_right` | Split the pane to the right | — |
| `split_down` | Split the pane below | — |
| `pop_out` | Open the tab in a new window (a pane with a single tab moves whole, when its window has another pane) | — |
| `close_pane` | Close the focused pane and all its tabs | — |
| `copy` | Copy the selection | Ctrl+Shift+C, Ctrl+Insert |
| `paste` | Paste from the clipboard | Ctrl+Shift+V, Ctrl+Shift+Insert |
| `paste_primary` | Paste the primary selection | Shift+Insert |
| `copy_path` | Copy the terminal's working directory | — |
| `clear` | Clear the terminal | — |
| `scroll_to_bottom` | Return to the newest terminal output | — |
| `git_sync` | Sync the current repository using the Git widget | — |
| `git_fetch` | Fetch and prune in the current repository | — |
| `git_pull` | Pull with fast-forward only | — |
| `git_push` | Push from the current repository | — |
| `find` | Open Find in Output | Ctrl+Shift+F |
| `palette` | Open the Command Palette | Ctrl+Shift+P |
| `cycle_scheme` | Cycle the colour scheme | — |
| `open_config` | Open the config file in an editor | — |
| `reload_config` | Reload the config | — |
| `keyboard_shortcuts` | Open the keyboard cheat sheet | F1 |
| `about` | Open About Paua | — |
| `quit` | Quit Paua | Ctrl+Q |
| `type:<text>` | Type text into the terminal and press Enter | — |
| `exec:<command>` | Run a command in the terminal's working directory and show its result in a toast | — |

Copy and paste act on the focused text field when one is open. Clipboard managers such as Copyous
can use Ctrl+Shift+Insert to paste into the terminal. Keep it in `paste` if you override that action
and want automatic pasting from Copyous.

Git shortcuts apply when the Git widget has detected a repository. The two command forms use
the same syntax as [status bar actions](status-bar.md#actions). Define
them in your user config. For example, `"type:git status"` does the same thing as a custom palette
command whose `run` text is `git status`.

## Find and Command Palette actions

These bindings apply while the corresponding field has focus.

| Action ID | Context | Action | Default |
|---|---|---|---|
| `close_search` | Both | Close the box and return focus to the terminal | Esc |
| `find_next` | Find | Go to the next match | Enter, keypad Enter, Down |
| `find_previous` | Find | Go to the previous match | Shift+Enter, Shift+keypad Enter, Up |
| `find_toggle_case` | Find | Toggle case sensitivity | — |
| `find_toggle_regex` | Find | Toggle regular expression mode | — |
| `palette_next` | Palette | Select the next result | Down, Tab |
| `palette_previous` | Palette | Select the previous result | Up, Shift+Tab |
| `palette_page_down` | Palette | Move down a page | Page Down |
| `palette_page_up` | Palette | Move up a page | Page Up |
| `palette_run` | Palette | Run the selected result | Enter, keypad Enter |
