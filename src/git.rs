//! Git for the status bar: everything shells out to `git` (hide the widget if it isn't there),
//! through `exec` so that every call has a timeout.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::exec::{self, ExecError};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub branch: String,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub deleted: u32,
    pub renamed: u32,
    pub untracked: u32,
    pub conflicted: u32,
    pub stashed: u32,
}

impl Status {
    pub fn dirty(&self) -> bool {
        self.staged + self.modified + self.deleted + self.renamed + self.untracked + self.conflicted > 0
    }
}

/// Parse `git status --porcelain=v2 --branch --show-stash`.
pub fn parse_status(out: &str) -> Status {
    let mut s = Status::default();
    let mut oid = String::new();
    for line in out.lines() {
        if let Some(h) = line.strip_prefix("# branch.head ") {
            if h == "(detached)" {
                s.detached = true;
            } else {
                s.branch = h.to_owned();
            }
        } else if let Some(o) = line.strip_prefix("# branch.oid ") {
            oid = o.to_owned();
        } else if let Some(u) = line.strip_prefix("# branch.upstream ") {
            s.upstream = Some(u.to_owned());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    s.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    s.behind = n.parse().unwrap_or(0);
                }
            }
        } else if let Some(n) = line.strip_prefix("# stash ") {
            s.stashed = n.trim().parse().unwrap_or(0);
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            let xy: Vec<char> = line.chars().skip(2).take(2).collect();
            let (x, y) = (xy.first().copied().unwrap_or('.'), xy.get(1).copied().unwrap_or('.'));
            if x != '.' {
                s.staged += 1;
            }
            if matches!(y, 'M' | 'T' | 'A') {
                s.modified += 1;
            }
            if y == 'D' {
                s.deleted += 1;
            }
            if line.starts_with("2 ") {
                s.renamed += 1;
            }
        } else if line.starts_with("u ") {
            s.conflicted += 1;
        } else if line.starts_with("? ") {
            s.untracked += 1;
        }
    }
    if s.detached {
        s.branch = oid.chars().take(7).collect();
    }
    s
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Repo(Status),
    NotRepo,
    NoGit,
}

fn git(cwd: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.arg("--no-optional-locks").args(args).env("LC_ALL", "C").env("GIT_TERMINAL_PROMPT", "0");
    if cwd.is_dir() {
        c.current_dir(cwd);
    }
    c
}

/// The state of the repository containing `cwd`. Blocking.
pub fn status(cwd: &Path, timeout: Duration) -> Fetched {
    match exec::run(git(cwd, &["status", "--porcelain=v2", "--branch", "--show-stash"]), timeout) {
        Ok(o) if o.success => Fetched::Repo(parse_status(&o.stdout)),
        Ok(_) | Err(ExecError::TimedOut) => Fetched::NotRepo,
        Err(ExecError::Spawn(_)) => Fetched::NoGit,
    }
}

/// Run a git command that changes things; `Ok(first line of output)` or `Err(first line of error)`.
pub fn run(cwd: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    match exec::run(git(cwd, args), timeout) {
        Ok(o) if o.success => Ok(exec::first_line(&o.stdout)),
        Ok(o) => {
            let e = exec::first_line(&o.stderr);
            Err(if e.is_empty() { exec::first_line(&o.stdout) } else { e })
        }
        Err(e) => Err(e.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// `main`, or `origin/feature` for a remote branch.
    pub name: String,
    pub remote: bool,
    pub current: bool,
}

impl Branch {
    /// What to give `git switch`: remote branches are switched to by their bare name.
    pub fn switch_name(&self) -> &str {
        if self.remote { self.name.split_once('/').map_or(&self.name, |(_, b)| b) } else { &self.name }
    }
}

pub fn parse_branches(out: &str) -> Vec<Branch> {
    let mut v: Vec<Branch> = Vec::new();
    for line in out.lines() {
        let mut p = line.splitn(3, '|');
        let (Some(head), Some(short), Some(full)) = (p.next(), p.next(), p.next()) else { continue };
        if short.ends_with("/HEAD") || short == "origin" {
            continue;
        }
        let remote = full.starts_with("refs/remotes/");
        let b = Branch { name: short.to_owned(), remote, current: head == "*" };
        // A remote branch that also exists locally adds nothing.
        if remote && v.iter().any(|x| !x.remote && x.name == b.switch_name()) {
            continue;
        }
        v.push(b);
    }
    // Local branches first (most recently used first within each group).
    v.sort_by_key(|b| b.remote);
    v
}

pub fn branches(cwd: &Path, timeout: Duration) -> Result<Vec<Branch>, String> {
    let args = [
        "for-each-ref",
        "--sort=-committerdate",
        "--format=%(HEAD)|%(refname:short)|%(refname)",
        "refs/heads",
        "refs/remotes",
    ];
    match exec::run(git(cwd, &args), timeout) {
        Ok(o) if o.success => Ok(parse_branches(&o.stdout)),
        Ok(o) => Err(exec::first_line(&o.stderr)),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_busy_status() {
        let out = "\
# branch.oid 0123456789abcdef
# branch.head feature/x
# branch.upstream origin/feature/x
# branch.ab +2 -1
# stash 3
1 M. N... 100644 100644 100644 a b src/a.rs
1 .M N... 100644 100644 100644 a b src/b.rs
1 MM N... 100644 100644 100644 a b src/c.rs
1 .D N... 100644 100644 000000 a b src/d.rs
2 R. N... 100644 100644 100644 a b R100 new.rs\told.rs
u UU N... 100644 100644 100644 100644 a b c conflict.rs
? untracked.txt
? other.txt
";
        let s = parse_status(out);
        assert_eq!(s.branch, "feature/x");
        assert_eq!(s.upstream.as_deref(), Some("origin/feature/x"));
        assert_eq!((s.ahead, s.behind, s.stashed), (2, 1, 3));
        assert_eq!(s.staged, 3); // M., MM, R.
        assert_eq!(s.modified, 2); // .M, MM
        assert_eq!((s.deleted, s.renamed, s.conflicted, s.untracked), (1, 1, 1, 2));
        assert!(s.dirty());
    }

    #[test]
    fn clean_detached_and_no_upstream() {
        let s = parse_status("# branch.oid abcdef0123456\n# branch.head (detached)\n");
        assert!(s.detached && s.branch == "abcdef0" && !s.dirty() && s.upstream.is_none());
        let s = parse_status("# branch.oid x\n# branch.head main\n");
        assert_eq!((s.branch.as_str(), s.ahead, s.behind), ("main", 0, 0));
    }

    #[test]
    fn branch_list() {
        let out = "\
*|main|refs/heads/main
 |dev|refs/heads/dev
 |origin/main|refs/remotes/origin/main
 |origin/feature|refs/remotes/origin/feature
 |origin/HEAD|refs/remotes/origin/HEAD
 |origin|refs/remotes/origin/HEAD
";
        let b = parse_branches(out);
        let names: Vec<&str> = b.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main", "dev", "origin/feature"]);
        assert!(b[0].current);
        assert_eq!(b[2].switch_name(), "feature");
    }

    #[test]
    fn a_real_repo_end_to_end() {
        let dir = std::env::temp_dir().join(format!("paua-git-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t = Duration::from_secs(10);
        let ok = |args: &[&str]| run(&dir, args, t).unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        ok(&["init", "-q", "-b", "main"]);
        ok(&["config", "user.email", "t@example.com"]);
        ok(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        ok(&["add", "."]);
        ok(&["commit", "-q", "-m", "one"]);
        std::fs::write(dir.join("b.txt"), "y").unwrap();
        let Fetched::Repo(s) = status(&dir, t) else { panic!("not a repo") };
        assert_eq!((s.branch.as_str(), s.untracked, s.staged), ("main", 1, 0));
        ok(&["switch", "-q", "-c", "topic"]);
        let b = branches(&dir, t).unwrap();
        assert!(b.iter().any(|b| b.name == "topic" && b.current));
        assert!(b.iter().any(|b| b.name == "main" && !b.current));
        let plain = std::env::temp_dir().join(format!("paua-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(status(&plain, t), Fetched::NotRepo);
        let _ = std::fs::remove_dir_all(&plain);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
