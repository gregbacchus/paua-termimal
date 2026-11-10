# Sessions: `session`

Paua works out what a pane's shell is currently running **inside**: an ssh connection, `sudo` /
`su`, a container, a terminal multiplexer, or simply another machine. Each detected session shows
as a badge in the pane header and can also tint the header line and recolour the whole window frame,
so it's hard to type a root command in the wrong window.

Detection combines two signals: the **process tree** on the pane's pty (Linux `/proc`, polled), and
the **host** in the shell's working-directory reports (OSC 7): a host other than this machine counts
as `remote` even when we can't see how you got there.

| Key | Type | Default | Description |
|---|---|---|---|
| `session.detect` | bool | `true` | Detect sessions at all. `false` turns off the `/proc` polling and every badge and tint. |
| `session.poll_ms` | integer (ms) | `1000` | How often to inspect the process tree. |
| `session.badges` | bool | `true` | Show a small badge per detected session in the pane header. |

## Kinds

`session.root`, `session.sudo`, `session.ssh`, `session.container`, `session.remote`,
`session.multiplexer`. When several apply, the first in this order is the pane's most important
session (it picks the header tint and the window colour): root, sudo, ssh, container, remote,
multiplexer.

| Kind | Detected when | Default label |
|---|---|---|
| `root` | the pane's shell runs as root while Paua does not (`sudo -i`, `su`) | `root` |
| `sudo` | `sudo`, `sudoedit`, `doas`, `pkexec`, `su`, `run0` is running | `{tool} {target}` |
| `ssh` | `ssh` is running (the target is `user@host`) | `ssh {target}` |
| `container` | `docker`, `podman`, `nerdctl`, `lxc`, `incus`, `kubectl`, `oc`, `distrobox`, `toolbox`, `nsenter`, `chroot`, `systemd-nspawn`... is running | `{tool} {target}` |
| `remote` | `mosh`, `telnet`, `rlogin`, `rsh` is running, or the shell reports a different host | `remote {target}` |
| `multiplexer` | `tmux`, `screen`, `zellij`, `byobu` is running | `{tool}` |

Each kind has the same options (shown for `ssh`; replace `ssh` by the kind). All are optional:

| Key | Type | Default | Description |
|---|---|---|---|
| `session.ssh.enabled` | bool | `true` | Detect and show this kind at all. (`multiplexer` is commonly set to `false`.) |
| `session.ssh.label` | string | per kind, see above | Badge text. Placeholders: `{tool}` (ssh, sudo, docker...), `{target}` (`user@host`, the container, or the user sudo switches to), `{host}`, `{user}`. Extra spaces are trimmed. |
| `session.ssh.icon` | string | `""` | Put before the label, e.g. a Nerd Font glyph. |
| `session.ssh.background` | [themed tint](value-types.md#colours) | unset (built in) | Badge background. |
| `session.ssh.color` | [themed tint](value-types.md#colours) | unset (built in) | Badge text. |
| `session.ssh.header_background` | [themed tint](value-types.md#colours) | unset (a subtle built-in tint for most kinds) | Tint of the whole header line while this is the pane's top session. `#rrggbbaa` for translucency. |
| `session.ssh.header_color` | [themed tint](value-types.md#colours) | unset | Text and icon colour of the header line then. |
| `session.ssh.window_color` | [themed colour](value-types.md#colours) | unset | Recolour the whole window frame while this session is active in the window's focused pane. It wins over every other window colour. |

The same options exist for each of the other kinds, with that kind's default label:

### `session.root`

| Key | Type | Default |
|---|---|---|
| `session.root.enabled` | bool | `true` |
| `session.root.label` | string | `root` |
| `session.root.icon` | string | `""` |
| `session.root.background` | themed tint | unset |
| `session.root.color` | themed tint | unset |
| `session.root.header_background` | themed tint | unset |
| `session.root.header_color` | themed tint | unset |
| `session.root.window_color` | themed colour | unset |

Meaning of each key: as `session.ssh.<key>` above.

### `session.sudo`

| Key | Type | Default |
|---|---|---|
| `session.sudo.enabled` | bool | `true` |
| `session.sudo.label` | string | `{tool} {target}` |
| `session.sudo.icon` | string | `""` |
| `session.sudo.background` | themed tint | unset |
| `session.sudo.color` | themed tint | unset |
| `session.sudo.header_background` | themed tint | unset |
| `session.sudo.header_color` | themed tint | unset |
| `session.sudo.window_color` | themed colour | unset |

Meaning of each key: as `session.ssh.<key>` above.

### `session.container`

| Key | Type | Default |
|---|---|---|
| `session.container.enabled` | bool | `true` |
| `session.container.label` | string | `{tool} {target}` |
| `session.container.icon` | string | `""` |
| `session.container.background` | themed tint | unset |
| `session.container.color` | themed tint | unset |
| `session.container.header_background` | themed tint | unset |
| `session.container.header_color` | themed tint | unset |
| `session.container.window_color` | themed colour | unset |

Meaning of each key: as `session.ssh.<key>` above.

### `session.remote`

| Key | Type | Default |
|---|---|---|
| `session.remote.enabled` | bool | `true` |
| `session.remote.label` | string | `remote {target}` |
| `session.remote.icon` | string | `""` |
| `session.remote.background` | themed tint | unset |
| `session.remote.color` | themed tint | unset |
| `session.remote.header_background` | themed tint | unset |
| `session.remote.header_color` | themed tint | unset |
| `session.remote.window_color` | themed colour | unset |

Meaning of each key: as `session.ssh.<key>` above.

### `session.multiplexer`

| Key | Type | Default |
|---|---|---|
| `session.multiplexer.enabled` | bool | `true` |
| `session.multiplexer.label` | string | `{tool}` |
| `session.multiplexer.icon` | string | `""` |
| `session.multiplexer.background` | themed tint | unset |
| `session.multiplexer.color` | themed tint | unset |
| `session.multiplexer.header_background` | themed tint | unset |
| `session.multiplexer.header_color` | themed tint | unset |
| `session.multiplexer.window_color` | themed colour | unset |

Meaning of each key: as `session.ssh.<key>` above.

## Defaults and a Nerd Font example

```yaml
# The defaults:
session:
  detect: true
  poll_ms: 1000
  badges: true
  root:        { enabled: true, label: "root" }
  sudo:        { enabled: true, label: "{tool} {target}" }
  ssh:         { enabled: true, label: "ssh {target}" }
  container:   { enabled: true, label: "{tool} {target}" }
  remote:      { enabled: true, label: "remote {target}" }
  multiplexer: { enabled: true, label: "{tool}" }
```

```yaml
# With Nerd Font icons, a red frame for root and a blue one for ssh:
session:
  root:
    icon: "\uf023 "                                   # nf-fa-lock
    window_color: { dark: "#6e1d1d", light: "#c43b3b" }
  sudo:   { icon: "\uf023 " }
  ssh:
    icon: "\uf0c1 "                                   # nf-fa-link
    background: "#4c8bf538"
    color: "#8db8ff"
    header_background: "#4c8bf524"
    header_color: "#cfe0ff"
    window_color: "#1d4f91"
  remote: { icon: "\uf233 " }                         # nf-fa-server
  multiplexer: { enabled: false }
```

Good values: a trailing space inside the icon string (`"\uf023 "`) separates it from the label;
`header_background` and badge `background` work best as translucent tints (`#rrggbb` + `14`–`38`),
and `window_color` as a solid, clearly different colour (a warning, not a preference).
