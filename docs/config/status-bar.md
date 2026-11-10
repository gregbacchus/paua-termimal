# Status bar: `status_bar`

The bar at the bottom of each window is a row of **widgets**, left and right. Each widget has a
`priority`: when the window is too narrow the **lowest priority drops out first** (of equals, the
one configured last). Widgets describe the **focused pane**: its directory, git repository, host,
session and last exit status.

Without a `status_bar.widgets` list you get git on the left and a recent-directories menu and a
clock on the right. A `widgets` list **replaces** that default set.

| Key | Type | Default | Description |
|---|---|---|---|
| `status_bar.enabled` | bool | `true` | Show the bar at all. |
| `status_bar.timeout` | [duration](value-types.md#duration) | `2s` | Default kill time for `exec` widgets (a widget can override it with `timeout`). |
| `status_bar.widgets` | list | git, dirs, clock (below) | The widgets. |

**Nothing in the bar can hang the interface.** Commands, `git` and file checks run on worker
threads, at most one job per widget at a time, in the pane's directory, and are **killed together
with everything they started** after the timeout. A failed, slow or empty widget simply hides.
`exec` results are cached per directory.

## `status_bar.font`

The bar's text. Anything unset keeps the system UI font (size: 0.9 × the UI font). Also applies to
the drop-down lists (branch picker, menus). Use a Nerd Font here if your widgets use icons; see
[Nerd Fonts](nerd-fonts.md).

| Key | Type | Default | Description |
|---|---|---|---|
| `status_bar.font.family` | string | unset (system UI font) | Font family. |
| `status_bar.font.size` | number | unset | Points. |
| `status_bar.font.weight` | [font weight](value-types.md#font-weight) | unset | Weight. |
| `status_bar.font.italic` | bool | `false` | Italic text. |

```yaml
status_bar:
  font: { family: Ubuntu Nerd Font, size: 9, weight: light, italic: false }
```

## `status_bar.widgets[]`

What a widget is depends on which field you give it, checked in this order:

| Kind | Chosen when | Shows |
|---|---|---|
| `clock` | `builtin: clock` | The time, `format` (strftime, default `%H:%M`). Updates every second; the tooltip has the full date. |
| `git` | `builtin: git` | Branch, state and a sync button; clicking the branch opens the branch picker. See [git](#status_bargit). |
| `dirs` | `builtin: dirs` | A menu of recent directories; picking one types `cd <dir>` into the pane. The label is `text` (default `{dir}`). |
| menu | `menu` is a non-empty list | A button that opens a list of actions. |
| button | `action` is set | A button that runs an action. |
| `exec` | `exec` is set | The first line a shell command prints. |
| text | none of the above | Plain text from the `text` template. |

| Key | Type | Default | Description |
|---|---|---|---|
| `status_bar.widgets[].id` | string | `w0`, `w1`... (position) | Names the widget; its CSS class is `sb-<id>`. Keep ids unique. |
| `status_bar.widgets[].side` | `left` \| `right` | `left` | Which end of the bar. Widgets are laid out in list order within a side. |
| `status_bar.widgets[].priority` | integer | `50` | Higher stays when space runs out. |
| `status_bar.widgets[].builtin` | `clock` \| `git` \| `dirs` | unset | Use a built-in widget. An unknown name is reported and shown as plain text. |
| `status_bar.widgets[].text` | [template](#templates) | per kind: `{output}` (exec), `{dir}` (dirs), the `id` (button, menu), empty (text) | The text. |
| `status_bar.widgets[].exec` | string | unset | Shell command (`sh -c`) run in the pane's directory; its **first non-empty output line** (colours removed, 200 characters at most) is `{output}`. A non-zero exit, a timeout or empty output counts as "no output". |
| `status_bar.widgets[].icon` | string | `""` | Put before the text, e.g. a Nerd Font glyph (`""`). A widget with neither icon nor text is hidden. |
| `status_bar.widgets[].tooltip` | [template](#templates) | unset | Tooltip. |
| `status_bar.widgets[].format` | string | `%H:%M` (clock) | Clock only: a `strftime` format, e.g. `%H:%M:%S`, `%a %d %b`. |
| `status_bar.widgets[].action` | string | unset | Makes the widget a button. See [actions](#actions). |
| `status_bar.widgets[].menu` | list | `[]` | Makes it a menu: see below. |
| `status_bar.widgets[].when` | map | unset | Show only in some directories: see below. |
| `status_bar.widgets[].refresh` | `cwd` \| `prompt` | `cwd` | When an `exec` / `when` widget re-runs besides its first time: `cwd` = when the pane's directory changes, `prompt` = also after every command (needs shell integration, OSC 133; nushell has it). |
| `status_bar.widgets[].interval` | [duration](value-types.md#duration) | unset | Also re-run this often while the window is focused. |
| `status_bar.widgets[].cache` | [duration](value-types.md#duration) | `30s` | Reuse an `exec` result for this long in the same directory. |
| `status_bar.widgets[].timeout` | [duration](value-types.md#duration) | `status_bar.timeout` | Kill the command after this long. |
| `status_bar.widgets[].hide_empty` | bool | `true` | Hide an `exec` widget while it has no output. |
| `status_bar.widgets[].color` | [themed tint](value-types.md#colours) | unset | Text colour. |
| `status_bar.widgets[].background` | [themed tint](value-types.md#colours) | unset | Background. |

### `status_bar.widgets[].when`

Looked for in the pane's directory **and all its parents**.

| Key | Type | Default | Description |
|---|---|---|---|
| `status_bar.widgets[].when.files` | list of strings | `[]` | Show only if **any** of these exists (files or directories). Empty = no requirement. |
| `status_bar.widgets[].when.not_files` | list of strings | `[]` | Hide if **any** of these exists. |

### `status_bar.widgets[].menu[]`

| Key | Type | Default | Description |
|---|---|---|---|
| `status_bar.widgets[].menu[].label` | string | required | The entry's text. |
| `status_bar.widgets[].menu[].action` | string | required | What it does: see [actions](#actions). |

### Templates

`text` and `tooltip` use the [template syntax](value-types.md#templates) with these names:

| Name | Value |
|---|---|
| `{output}` | The first line the widget's `exec` printed. |
| `{path}` | The pane's directory as shown in headers (`panes.header.path_replacements` applied). |
| `{dir}` | The last component of the directory. |
| `{host}` | The host the shell reports (OSC 7). |
| `{user}` | Your user name. |
| `{title}` | The title set by the program in the pane. |
| `{session}` | The detected session text, e.g. `ssh me@box`. |
| `{exit}` | Exit status of the last command (empty if the shell doesn't report it; needs OSC 133). `0` counts as not set in `{?exit:...}`. |

```yaml
- { id: exit, side: right, text: "{?exit:exit {exit}}", color: "#ff6b6b" }   # only after a failure
```

### Actions

Used by `action` and `menu[].action`:

| Action | Does |
|---|---|
| `split_right`, `split_down` | Split the focused pane. |
| `find`, `palette` | Open the title-bar box in that mode. |
| `clear` | Clear the focused pane's screen and scrollback. |
| `close_pane`, `pop_out`, `copy_path` | Close / open in a new window / copy the working directory of the focused pane. |
| `new_window` | Open a window in the focused pane's directory. |
| `cycle_scheme` | Cycle system / light / dark. |
| `reload_config`, `open_config` | Re-read / open the config file. |
| `type:<text>` | Type `<text>` into the focused pane, then Enter. |
| `exec:<command>` | Run a shell command in the background in the pane's directory (killed after 60 s); the first line of the result is shown as a toast. |

An unknown action shows a toast saying so.

### Defaults

```yaml
status_bar:
  enabled: true
  timeout: 2s
  widgets:
    - { id: git,   builtin: git,   side: left,  priority: 80 }
    - { id: dirs,  builtin: dirs,  side: right, priority: 30, icon: "⌂" }
    - { id: clock, builtin: clock, side: right, priority: 20 }
```

### Examples, with Nerd Font icons

```yaml
status_bar:
  widgets:
    - { id: git,  builtin: git,  side: left, priority: 80 }
    - id: node                                         # only in Node projects
      side: left
      priority: 51
      icon: " "                                  # nf-dev-nodejs_small
      exec: "node --version"
      when: { files: [package.json], not_files: [bun.lockb] }
      cache: 1m
    - id: rust
      side: left
      priority: 50
      icon: " "                                  # nf-dev-rust
      exec: "rustc --version | cut -d' ' -f2"
      when: { files: [Cargo.toml] }
    - { id: exit, side: right, priority: 60, text: "{?exit:exit {exit}}", color: "#ff6b6b" }
    - { id: clear, side: right, priority: 25, icon: " ", tooltip: "Clear terminal", action: clear }
    - id: layout
      side: right
      priority: 24
      text: "Layout"
      menu: [{ label: Split right, action: split_right }, { label: Split down, action: split_down }]
    - { id: dirs,  builtin: dirs,  side: right, priority: 30, icon: " " }   # nf-fa-folder_open
    - { id: clock, builtin: clock, side: right, priority: 10, icon: " ", format: "%H:%M:%S" }
```

Good values: priorities in steps of 10 (git 80, errors 60, language versions 50, menus 30, clock 10)
leave room to slot widgets in; a trailing space in `icon` separates it from the text; give slow
commands `cache: 1m` or more and a short `timeout`.

## `status_bar.git`

The built-in git widget shells out to `git` (with `--no-optional-locks`, so it never blocks your own
git commands). It is **hidden** if git isn't installed or the directory isn't a repository. It
refreshes when the pane's directory changes, after each command (shells with OSC 133), and every
`interval`, and shows the branch, a `*` when there are changes, icons with counts, and a sync
button.

**Interaction**
- Click the **branch** to open a searchable branch list: type to filter, **Enter** switches to the
  first match, or creates a branch with the name you typed (`git switch -c`); remote branches are
  switched to by their bare name. The bottom of the list has **Fetch**, **Pull** (fast-forward
  only) and **Push** buttons.
- The **sync button** next to the branch (shown when the branch has an upstream) pulls
  (`--ff-only`) if you are behind, pushes if you are ahead, and fetches if you are neither. It
  shows `↓2 ↑1`-style counts; with both, the `diverged` icon.
- Failures (a dirty tree blocking a switch, a diverged branch) show git's message in a toast; nothing
  destructive is offered. Fetch, pull and push give up after `network_timeout`.

| Key | Type | Default | Description |
|---|---|---|---|
| `status_bar.git.icon` | string | `⎇` | Before the branch name. With a Nerd Font: `""`, `""` or `""`. |
| `status_bar.git.format` | [template](#templates) | `{branch}{?dirty:*} {status}` | The text. Names: `{branch}` (a short hash when detached), `{upstream}`, `{ahead}`, `{behind}`, `{sync}` (e.g. `↓2 ↑1`), `{diverged}` (its icon when ahead *and* behind), `{staged}`, `{modified}`, `{deleted}`, `{renamed}`, `{untracked}`, `{conflicted}`, `{stashed}` (counts; empty when 0), `{status}` (all the state icons with counts, below) and `{dirty}` (`*` when there are changes). |
| `status_bar.git.counts` | bool | `true` | Counts next to the icons in `{status}` and the sync button (`✚2` vs `✚`). |
| `status_bar.git.interval` | [duration](value-types.md#duration) | `5s` | Refresh this often while the window is focused. |
| `status_bar.git.timeout` | [duration](value-types.md#duration) | `3s` | Kill `git status` after this long (the widget then hides). |
| `status_bar.git.network_timeout` | [duration](value-types.md#duration) | `60s` | Give up on fetch / pull / push after this long. |
| `status_bar.git.sync_button` | bool | `true` | Show the sync button. |
| `status_bar.git.sync_icon` | string | `view-refresh-symbolic` | The sync button's icon: a theme icon name (`view-refresh-symbolic`, `emblem-synchronizing-symbolic`...) or text such as a glyph (`""`). Empty = none. |

### `status_bar.git.icons`

The icons used in `{status}` and the sync button. Used exactly as written, so a trailing space
(`"✚ "`) puts a gap before the count.

| Key | Type | Default | Description |
|---|---|---|---|
| `status_bar.git.icons.ahead` | string | `↑` | Commits to push. |
| `status_bar.git.icons.behind` | string | `↓` | Commits to pull. |
| `status_bar.git.icons.diverged` | string | `⇅` | Shown (then with both counts) when ahead and behind. |
| `status_bar.git.icons.staged` | string | `●` | Changes added to the index. |
| `status_bar.git.icons.modified` | string | `✚` | Modified in the working tree. |
| `status_bar.git.icons.deleted` | string | `✖` | Deleted. |
| `status_bar.git.icons.renamed` | string | `»` | Renamed. |
| `status_bar.git.icons.untracked` | string | `?` | Untracked files. |
| `status_bar.git.icons.conflicted` | string | `!` | Merge conflicts. |
| `status_bar.git.icons.stashed` | string | `≡` | Stash entries. |

```yaml
# The defaults:
status_bar:
  git:
    icon: "⎇"
    format: "{branch}{?dirty:*} {status}"
    counts: true
    interval: 5s
    timeout: 3s
    network_timeout: 60s
    sync_button: true
    sync_icon: view-refresh-symbolic
    icons:
      ahead: "↑"
      behind: "↓"
      diverged: "⇅"
      staged: "●"
      modified: "✚"
      deleted: "✖"
      renamed: "»"
      untracked: "?"
      conflicted: "!"
      stashed: "≡"
```

```yaml
# With Nerd Font icons (needs status_bar.font.family set to a Nerd Font):
status_bar:
  git:
    icon: " "                 # nf-pl-branch
    format: "{branch}{?dirty:*} {status}"
    sync_icon: ""             # nf-fa-refresh
    icons:
      staged: " "             # nf-fa-plus
      modified: " "           # nf-fa-pencil
      deleted: " "            # nf-fa-trash
      untracked: " "          # nf-fa-question
      stashed: " "           # nf-fa-archive
      conflicted: "🚨"
```

Good values: leave `counts: true` so `✚2` tells you how many; use `{sync}` in `format` if you turn
the sync button off (`sync_button: false`) and still want ahead/behind in the text.
