//! Working out what a pane's shell session is "inside": ssh, sudo/su, a container, a terminal
//! multiplexer, or another machine.
//!
//! Two signals are combined by the app:
//!
//! 1. **The process tree** (this module, Linux `/proc`): the pane's foreground process group (the
//!    program currently running on the pty) and its ancestors up to the shell, plus its
//!    descendants. `ssh`, `sudo`, `docker exec`... show up here as ordinary processes.
//! 2. **The host in the shell's directory reports** (OSC 7): if it differs from this machine, the
//!    shell is on another host, even if we can't see how we got there. That part lives in the app.

use std::fs;
use std::sync::OnceLock;

/// What kind of session. The order is the priority used when several apply (e.g. to pick a
/// window colour): the most "dangerous" first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Root,
    Sudo,
    Ssh,
    Container,
    Remote,
    Multiplexer,
}

impl Kind {
    pub const ALL: [Kind; 6] =
        [Kind::Root, Kind::Sudo, Kind::Ssh, Kind::Container, Kind::Remote, Kind::Multiplexer];

    /// CSS class put on a pane's header while this is the pane's most important session.
    pub fn header_class(self) -> &'static str {
        match self {
            Kind::Root => "session-root",
            Kind::Sudo => "session-sudo",
            Kind::Ssh => "session-ssh",
            Kind::Container => "session-container",
            Kind::Remote => "session-remote",
            Kind::Multiplexer => "session-multiplexer",
        }
    }

    /// Name used for CSS classes and config keys.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Root => "root",
            Kind::Sudo => "sudo",
            Kind::Ssh => "ssh",
            Kind::Container => "container",
            Kind::Remote => "remote",
            Kind::Multiplexer => "multiplexer",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    /// The program, e.g. `ssh`, `sudo`, `docker`.
    pub tool: String,
    /// Where/who to, for display: `user@host`, a container name, the target user (empty for root).
    pub target: String,
    pub host: String,
    pub user: String,
}

impl Item {
    fn new(kind: Kind, tool: &str) -> Self {
        Self { kind, tool: tool.to_owned(), target: String::new(), host: String::new(), user: String::new() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionInfo {
    pub items: Vec<Item>,
}

impl SessionInfo {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn has(&self, kind: Kind) -> bool {
        self.items.iter().any(|i| i.kind == kind)
    }
}

// ---------------------------------------------------------------------------------------------
// Classifying a command line
// ---------------------------------------------------------------------------------------------

fn base(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s).trim_start_matches('-')
}

/// The first argument that isn't an option, skipping the values of options that take one.
/// `value_flags` lists those options (both `-x` and `--long` spellings). Stops at `--`.
fn first_positional<'a>(args: &'a [String], value_flags: &[&str]) -> Option<&'a str> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            return it.next().map(String::as_str);
        }
        if a.starts_with('-') {
            if value_flags.contains(&a.as_str()) {
                it.next(); // its value
            }
            continue;
        }
        return Some(a);
    }
    None
}

/// The value of `-x value`, `-xvalue` or `--long=value` / `--long value`.
fn option_value(args: &[String], short: &str, long: &str) -> Option<String> {
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if a == "--" {
            break;
        }
        if a == short || a == long {
            return it.peek().map(|v| (*v).clone());
        }
        if let Some(v) = a.strip_prefix(&format!("{long}=")) {
            return Some(v.to_owned());
        }
        if !short.is_empty() && !a.starts_with("--") {
            if let Some(v) = a.strip_prefix(short) {
                if !v.is_empty() {
                    return Some(v.to_owned());
                }
            }
        }
    }
    None
}

fn ssh_item(tool: &str, args: &[String]) -> Item {
    // ssh options that consume the next argument.
    const WITH_VALUE: &[&str] = &[
        "-b", "-c", "-D", "-E", "-e", "-F", "-I", "-i", "-J", "-L", "-l", "-m", "-O", "-o", "-p", "-Q",
        "-R", "-S", "-W", "-w",
    ];
    let mut item = Item::new(Kind::Ssh, tool);
    let Some(dest) = first_positional(&args[1..], WITH_VALUE) else { return item };
    // ssh://user@host:port  |  user@host  |  host
    let dest = dest.strip_prefix("ssh://").unwrap_or(dest);
    let (user, host) = match dest.rsplit_once('@') {
        Some((u, h)) => (u.to_owned(), h),
        None => (option_value(&args[1..], "-l", "").unwrap_or_default(), dest),
    };
    let host = host.split('/').next().unwrap_or(host);
    let host = if host.starts_with('[') {
        host.trim_start_matches('[').split(']').next().unwrap_or(host) // [::1]:22
    } else {
        host.rsplit_once(':').map_or(host, |(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { host })
    };
    item.host = host.to_owned();
    item.target = if user.is_empty() { host.to_owned() } else { format!("{user}@{host}") };
    item.user = user;
    item
}

fn sudo_item(tool: &str, args: &[String]) -> Item {
    let mut item = Item::new(Kind::Sudo, tool);
    let user = option_value(&args[1..], "-u", "--user")
        .or_else(|| (tool == "su").then(|| first_positional(&args[1..], &["-c", "-s", "-g", "-G"]).map(str::to_owned)).flatten())
        .unwrap_or_default();
    if user != "root" {
        item.target = user.clone();
        item.user = user;
    }
    item
}

fn container_item(tool: &str, args: &[String]) -> Option<Item> {
    const DOCKER_VALUE: &[&str] = &[
        "-e", "--env", "-u", "--user", "-w", "--workdir", "-v", "--volume", "-p", "--publish", "--name",
        "-l", "--label", "--network", "--entrypoint", "-h", "--hostname", "--mount", "--env-file", "--cap-add",
        "--cap-drop", "--detach-keys", "--platform", "--pull", "--memory", "-m", "--cpus",
    ];
    let pos = args.iter().position(|a| matches!(a.as_str(), "exec" | "run" | "attach"))?;
    let mut item = Item::new(Kind::Container, tool);
    item.target = first_positional(&args[pos + 1..], DOCKER_VALUE).unwrap_or_default().to_owned();
    Some(item)
}

fn kubectl_item(tool: &str, args: &[String]) -> Option<Item> {
    const VALUE: &[&str] = &[
        "-n", "--namespace", "-c", "--container", "--context", "--cluster", "-f", "--filename", "-s", "--server",
        "--kubeconfig", "--user", "-l", "--selector", "--image", "--target",
    ];
    let pos = args.iter().position(|a| matches!(a.as_str(), "exec" | "debug" | "attach"))?;
    let mut item = Item::new(Kind::Container, tool);
    item.target = first_positional(&args[pos + 1..], VALUE).unwrap_or_default().to_owned();
    Some(item)
}

/// Classify one process by its command line (`argv[0]` first).
pub fn classify(args: &[String]) -> Option<Item> {
    let tool = base(args.first()?);
    match tool {
        "ssh" | "autossh" => Some(ssh_item(tool, args)),
        "mosh" | "mosh-client" => {
            let mut item = Item::new(Kind::Remote, tool);
            // `mosh-client IP PORT` or `mosh [user@]host`
            let host = first_positional(&args[1..], &["--ssh", "--predict", "-p", "--port"]).unwrap_or_default();
            item.target = host.to_owned();
            item.host = host.rsplit('@').next().unwrap_or(host).to_owned();
            Some(item)
        }
        "telnet" | "rlogin" | "rsh" | "ftp" | "sftp" | "lftp" | "netcat" | "nc" | "ncat" | "socat" => {
            // Only the interactive remote-login ones are a "session"; file transfer tools aren't.
            if matches!(tool, "telnet" | "rlogin" | "rsh") {
                let mut item = Item::new(Kind::Remote, tool);
                let host = first_positional(&args[1..], &["-l", "-p"]).unwrap_or_default();
                item.target = host.to_owned();
                item.host = host.to_owned();
                Some(item)
            } else {
                None
            }
        }
        "sudo" | "sudoedit" | "doas" | "pkexec" | "su" | "run0" => Some(sudo_item(tool, args)),
        "docker" | "podman" | "nerdctl" | "lxc" | "incus" => container_item(tool, args),
        "kubectl" | "oc" => kubectl_item(tool, args),
        "distrobox" | "distrobox-enter" | "toolbox" => {
            let mut item = Item::new(Kind::Container, tool);
            let rest: &[String] = if tool == "distrobox-enter" { &args[1..] } else {
                let pos = args.iter().position(|a| a == "enter")?;
                &args[pos + 1..]
            };
            item.target = option_value(rest, "-n", "--name")
                .or_else(|| first_positional(rest, &["-n", "--name", "-i", "--image"]).map(str::to_owned))
                .unwrap_or_default();
            Some(item)
        }
        "nsenter" | "chroot" | "systemd-nspawn" | "machinectl" | "unshare" | "bwrap" | "firejail" => {
            Some(Item::new(Kind::Container, tool))
        }
        t if t.starts_with("tmux") || t.starts_with("screen") || t == "zellij" || t == "byobu" => {
            Some(Item::new(Kind::Multiplexer, t.split(':').next().unwrap_or(t)))
        }
        _ => None,
    }
}

/// Fill `{tool}`, `{target}`, `{host}`, `{user}` in a badge label and tidy the spacing.
pub fn format_label(template: &str, item: &Item) -> String {
    let s = template
        .replace("{tool}", &item.tool)
        .replace("{target}", &item.target)
        .replace("{host}", &item.host)
        .replace("{user}", &item.user);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether two host names are the same machine (case-insensitive, ignoring the domain part).
pub fn same_host(a: &str, b: &str) -> bool {
    let short = |h: &str| h.split('.').next().unwrap_or(h).to_ascii_lowercase();
    a.is_empty() || b.is_empty() || a == "localhost" || b == "localhost" || short(a) == short(b)
}

// ---------------------------------------------------------------------------------------------
// Reading the process tree
// ---------------------------------------------------------------------------------------------

fn read_args(pid: i32) -> Option<Vec<String>> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    (!args.is_empty()).then_some(args)
}

/// The program name of a process (`argv[0]`, as it was started), if it can be read.
pub fn process_name(pid: i32) -> Option<String> {
    read_args(pid).and_then(|a| a.into_iter().next())
}

/// The working directory of a process (Linux `/proc/<pid>/cwd`), if it can be read and still exists.
pub fn cwd_of(pid: i32) -> Option<std::path::PathBuf> {
    let p = fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    // A directory that was removed reads as "/path (deleted)".
    (!p.to_string_lossy().ends_with(" (deleted)")).then_some(p)
}

fn parent_of(pid: i32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // "pid (comm) S ppid ..." where comm may itself contain spaces and parentheses.
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().nth(1)?.parse().ok()
}

fn children_of(pid: i32) -> Vec<i32> {
    fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
        .map(|s| s.split_whitespace().filter_map(|c| c.parse().ok()).collect())
        .unwrap_or_default()
}

fn uid_of(pid: i32) -> Option<u32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn own_uid() -> u32 {
    uid_of(std::process::id() as i32).unwrap_or(u32::MAX)
}

/// Whether this process itself runs inside a container (the baseline for every pane).
fn inside_container() -> Option<&'static Item> {
    static C: OnceLock<Option<Item>> = OnceLock::new();
    C.get_or_init(|| {
        let marker = ["/run/.containerenv", "/.dockerenv"].iter().any(|p| std::path::Path::new(p).exists());
        marker.then(|| {
            let mut item = Item::new(Kind::Container, "container");
            item.target = std::env::var("CONTAINER_ID")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_default();
            item
        })
    })
    .as_ref()
}

/// Inspect a shell and the program running in the foreground on its pty.
///
/// `fg_leader` is the foreground process group's leader (what `tcgetpgrp` reports); processes from
/// there up to `shell_pid`, and a few levels of its descendants, are classified.
pub fn detect(shell_pid: i32, fg_leader: Option<i32>) -> SessionInfo {
    let mut pids: Vec<i32> = Vec::new();

    if let Some(leader) = fg_leader.filter(|p| *p != shell_pid) {
        // The leader and its ancestors up to (not including) the shell.
        let mut p = leader;
        for _ in 0..16 {
            if p == shell_pid || p <= 1 {
                break;
            }
            pids.push(p);
            match parent_of(p) {
                Some(pp) => p = pp,
                None => break,
            }
        }
        // And what it started: `sudo ssh host`, `docker exec ... ssh`...
        let mut frontier = vec![leader];
        for _ in 0..3 {
            let next: Vec<i32> = frontier.iter().flat_map(|p| children_of(*p)).collect();
            pids.extend(&next);
            frontier = next;
            if frontier.is_empty() {
                break;
            }
        }
    }

    let mut items: Vec<Item> = Vec::new();
    let mut push = |item: Item| {
        if !items.contains(&item) {
            items.push(item);
        }
    };

    // A root shell (`sudo -s` replaced it, or the pane was started as root) when we aren't root.
    if own_uid() != 0 && uid_of(shell_pid) == Some(0) {
        push(Item::new(Kind::Root, "root"));
    }
    if let Some(c) = inside_container() {
        push(c.clone());
    }
    for pid in pids {
        if let Some(item) = read_args(pid).and_then(|a| classify(&a)) {
            push(item);
        }
    }
    items.sort_by_key(|i| i.kind);
    SessionInfo { items }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_process_working_directory_is_read_from_proc() {
        let here = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert_eq!(cwd_of(std::process::id() as i32).and_then(|p| p.canonicalize().ok()), Some(here));
        assert_eq!(cwd_of(i32::MAX), None);
    }

    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    fn one(s: &str) -> Item {
        classify(&args(s)).unwrap_or_else(|| panic!("not classified: {s}"))
    }

    #[test]
    fn ssh_destinations() {
        let i = one("ssh user@box.example.com");
        assert_eq!((i.kind, i.host.as_str(), i.user.as_str(), i.target.as_str()), (Kind::Ssh, "box.example.com", "user", "user@box.example.com"));
        assert_eq!(one("/usr/bin/ssh -p 2222 -i ~/.ssh/key -o StrictHostKeyChecking=no greg@10.0.0.5").target, "greg@10.0.0.5");
        assert_eq!(one("ssh -l bob server").target, "bob@server");
        assert_eq!(one("ssh -J jump host2 uptime").host, "host2");
        assert_eq!(one("ssh ssh://amy@h:2200").target, "amy@h");
        assert_eq!(one("ssh -tt prod").target, "prod");
        assert_eq!(one("ssh -6 [::1]:22").host, "::1");
        assert_eq!(one("ssh").target, ""); // no destination yet
        assert!(classify(&args("ssh-keygen -t ed25519")).is_none());
        assert!(classify(&args("ssh-agent")).is_none());
    }

    #[test]
    fn sudo_and_friends() {
        let i = one("sudo -i");
        assert_eq!((i.kind, i.target.as_str()), (Kind::Sudo, "")); // root: nothing to add
        assert_eq!(one("sudo -u postgres psql").target, "postgres");
        assert_eq!(one("sudo --user=www-data bash").target, "www-data");
        assert_eq!(one("doas -u bob sh").target, "bob");
        assert_eq!(one("su - alice").target, "alice");
        assert_eq!(one("su -").target, "");
        assert_eq!(one("pkexec bash").kind, Kind::Sudo);
        assert_eq!(one("-sudo").tool, "sudo"); // login-shell style argv[0]
    }

    #[test]
    fn containers_and_multiplexers() {
        assert_eq!(one("docker exec -it -e FOO=1 web bash").target, "web");
        assert_eq!(one("docker run --rm -it -v /a:/b alpine sh").target, "alpine");
        assert_eq!(one("podman exec -it db psql").target, "db");
        assert!(classify(&args("docker ps")).is_none());
        assert_eq!(one("kubectl exec -n prod -it mypod -- sh").target, "mypod");
        assert_eq!(one("distrobox enter dev").target, "dev");
        assert_eq!(one("toolbox enter fedora").target, "fedora");
        assert_eq!(one("tmux attach -t main").kind, Kind::Multiplexer);
        assert_eq!(one("screen -r").kind, Kind::Multiplexer);
        assert_eq!(one("mosh greg@far").host, "far");
        assert_eq!(one("telnet 10.1.1.1").kind, Kind::Remote);
        assert!(classify(&args("vim file.txt")).is_none());
        assert!(classify(&[]).is_none());
    }

    #[test]
    fn labels_and_host_comparison() {
        let ssh = one("ssh greg@box.example.com");
        assert_eq!(format_label("ssh {target}", &ssh), "ssh greg@box.example.com");
        assert_eq!(format_label("{host}", &ssh), "box.example.com");
        let sudo = one("sudo -i");
        assert_eq!(format_label("{tool} {target}", &sudo), "sudo"); // empty target leaves no gap
        assert_eq!(format_label("\u{f817} {tool}  {user} ", &sudo), "\u{f817} sudo");
        assert!(same_host("Box.local", "box"));
        assert!(same_host("localhost", "anything"));
        assert!(!same_host("far.example.com", "box"));
    }

    #[test]
    fn priority_order_puts_root_and_sudo_first() {
        let mut kinds = vec![Kind::Multiplexer, Kind::Ssh, Kind::Root, Kind::Container, Kind::Sudo, Kind::Remote];
        kinds.sort();
        assert_eq!(kinds, Kind::ALL.to_vec());
    }

    /// Real processes: a fake `ssh` (python started with `argv[0]` = "ssh", which, unlike
    /// multi-call coreutils, doesn't care what it is called) is found through /proc.
    #[test]
    fn finds_a_foreground_ssh_in_the_real_process_tree() {
        use std::os::unix::process::CommandExt;
        let Ok(mut child) = std::process::Command::new("python3")
            .arg0("ssh")
            .args(["-c", "import time; time.sleep(10)", "me@box"])
            .spawn()
        else {
            eprintln!("python3 not available; skipping");
            return;
        };
        let me = std::process::id() as i32;
        // The command line is briefly empty while the kernel finishes exec: retry.
        let mut info = SessionInfo::default();
        for _ in 0..50 {
            info = detect(me, Some(child.id() as i32));
            if info.has(Kind::Ssh) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        let ssh = info.items.iter().find(|i| i.kind == Kind::Ssh).expect("ssh not detected");
        assert_eq!((ssh.tool.as_str(), ssh.target.as_str()), ("ssh", "me@box"));

        // With nothing in the foreground there is nothing to report (we are not root).
        if own_uid() != 0 && inside_container().is_none() {
            assert!(detect(me, Some(me)).is_empty());
        }
    }

    #[test]
    fn stat_parsing_survives_odd_process_names() {
        // Our own parent exists and parses.
        assert!(parent_of(std::process::id() as i32).is_some());
    }
}
