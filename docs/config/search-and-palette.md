# Search and palette: `search`, `palette`

The box in the middle of the title bar does two things: **find in output** (Ctrl+Shift+F, or click
the box and type `/`) searches the selected tab's scrollback and screen, and the **command
palette** (Ctrl+Shift+P, or click the box; type `>`) runs commands. A chip at the left of the box
shows the mode.

## `search`

| Key | Type | Default | Description |
|---|---|---|---|
| `search.case` | `smart` \| `sensitive` \| `insensitive` | `smart` | `smart` is case-insensitive unless the query contains an uppercase letter. The box's *Aa* toggle forces `sensitive`. |
| `search.regex` | bool | `false` | Start with the regular-expression toggle (`.*`) on. |
| `search.match_color` | [themed tint](value-types.md#colours) | unset (`#ffc43c52`, amber at 32%) | Highlight colour of every match. |
| `search.current_color` | [themed tint](value-types.md#colours) | unset (`#ff8c00a6`, orange at 65%) | Highlight colour of the current match. |

```yaml
search:
  case: smart
  regex: false
  # match_color: "#ffc43c52"
  # current_color: "#ff8c00a6"
```

## `palette`

Built-in entries include *Find in output, Split right, Split down, New tab, Close tab, Previous tab,
Next tab, Open pane in new window, Close pane, Clear terminal, Copy working directory, New window,
Cycle colour scheme, Reload config,* and *Open config file*. You can also jump to tabs across panes
and windows, `cd` to recent directories, `ssh` to your SSH hosts, and run your own commands.

| Key | Type | Default | Description |
|---|---|---|---|
| `palette.max_results` | integer | `40` | Most results shown at once. |
| `palette.directories` | bool | `true` | Offer `cd <dir>` for recently visited directories. |
| `palette.recent_dirs` | integer | `25` | How many recent directories to remember (this run only). |
| `palette.ssh_hosts` | bool | `true` | Offer `ssh <host>` for the `Host` aliases in `~/.ssh/config` (wildcards and negations are skipped). |
| `palette.commands` | list | `[]` | Your own entries, see below. |

### `palette.commands[]`

Each entry types `run` into the selected tab in the focused pane, followed by Enter.

| Key | Type | Default | Description |
|---|---|---|---|
| `palette.commands[].title` | string | required | The name shown (and searched). |
| `palette.commands[].run` | string | required | The text typed into the pane. |
| `palette.commands[].subtitle` | string | the command itself | Dimmed text on the right (also searched). |

```yaml
palette:
  max_results: 40
  directories: true
  recent_dirs: 25
  ssh_hosts: true
  commands:
    - { title: "Run tests", run: "cargo test" }
    - { title: "Git status", run: "git status -sb", subtitle: "in this repo" }
```
