//! Command palette: the items, fuzzy matching and small helpers. (The widget lives in
//! `searchbox.rs`; the app decides which items exist.)

use std::path::{Path, PathBuf};
use std::rc::Rc;

/// One thing the palette can do.
#[derive(Clone)]
pub struct PaletteItem {
    /// Group label shown as a small tag: "Command", "Pane", "Directory", "SSH"...
    pub section: &'static str,
    pub title: String,
    /// Secondary text: a shortcut, a path, a hint. Also searched, with lower weight.
    pub subtitle: String,
    pub run: Rc<dyn Fn()>,
}

impl PaletteItem {
    pub fn new(section: &'static str, title: impl Into<String>, subtitle: impl Into<String>, run: impl Fn() + 'static) -> Self {
        Self { section, title: title.into(), subtitle: subtitle.into(), run: Rc::new(run) }
    }
}

/// Fuzzy-match `query` against `text` as an in-order, case-insensitive subsequence. Returns a
/// score (higher = better) and the matched character indices (into `text.chars()`), or `None`.
///
/// Consecutive characters, matches at the start of a word, and short gaps score higher, so
/// `spr` finds "**Sp**lit **r**ight" ahead of "S**p**lit st**r**eam".
pub fn fuzzy(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    let t: Vec<char> = text.chars().collect();
    let tl: Vec<char> = t.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let (n, m) = (q.len(), t.len());
    if n > m {
        return None;
    }

    let word_start = |j: usize| j == 0 || !t[j - 1].is_alphanumeric() || (t[j - 1].is_lowercase() && t[j].is_uppercase());
    const NONE: i32 = i32::MIN / 2;
    // best[i][j]: best score with q[i] matched at text position j.
    let mut best = vec![vec![NONE; m]; n];
    let mut from = vec![vec![usize::MAX; m]; n];
    for j in 0..m {
        if tl[j] == q[0] {
            best[0][j] = 10 + if word_start(j) { 8 } else { 0 } + if j == 0 { 6 } else { 0 } - (j as i32).min(8);
        }
    }
    for i in 1..n {
        for j in i..m {
            if tl[j] != q[i] {
                continue;
            }
            let base = 10 + if word_start(j) { 8 } else { 0 };
            for k in (i - 1)..j {
                if best[i - 1][k] == NONE {
                    continue;
                }
                let gap = (j - k - 1) as i32;
                let link = if gap == 0 { 6 } else { -gap.min(6) };
                let score = best[i - 1][k] + base + link;
                if score > best[i][j] {
                    best[i][j] = score;
                    from[i][j] = k;
                }
            }
        }
    }
    let (mut j, score) = (0..m).map(|j| (j, best[n - 1][j])).max_by_key(|(_, s)| *s)?;
    if score == NONE {
        return None;
    }
    let mut idx = vec![0; n];
    for i in (0..n).rev() {
        idx[i] = j;
        if i > 0 {
            j = from[i][j];
        }
    }
    Some((score - (m as i32 - n as i32).min(20), idx)) // slightly prefer shorter texts
}

/// One displayed result: an index into the items and the title characters to emphasise.
pub struct Ranked {
    pub item: usize,
    pub title_matches: Vec<usize>,
}

/// Filter and order `items` for `query`. With an empty query everything is returned in order.
pub fn rank(items: &[PaletteItem], query: &str, limit: usize) -> Vec<Ranked> {
    if query.trim().is_empty() {
        return (0..items.len().min(limit))
            .map(|item| Ranked { item, title_matches: Vec::new() })
            .collect();
    }
    let mut scored: Vec<(i32, usize, Vec<usize>)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            if let Some((s, idx)) = fuzzy(query, &it.title) {
                Some((s + 20, i, idx)) // title matches beat subtitle-only matches
            } else {
                fuzzy(query, &it.subtitle).map(|(s, _)| (s, i, Vec::new()))
            }
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, item, title_matches)| Ranked { item, title_matches })
        .collect()
}

/// Quote a path for typing into a shell: left alone if it only has safe characters, otherwise in
/// single quotes (or double quotes if it contains a single quote).
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_-./~+@%:,=".contains(c)) {
        return s.to_owned();
    }
    if !s.contains('\'') {
        return format!("'{s}'");
    }
    let escaped: String = s
        .chars()
        .flat_map(|c| match c {
            '"' | '\\' | '$' | '`' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!("\"{escaped}\"")
}

/// Host aliases from an OpenSSH client config: the names on `Host` lines, without wildcard
/// patterns or negations.
pub fn ssh_hosts(config: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for line in config.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("Host ")
            .or_else(|| line.strip_prefix("Host\t"))
            .or_else(|| line.strip_prefix("host "))
        else {
            continue;
        };
        for name in rest.split_whitespace() {
            let name = name.trim_matches('"');
            if !name.is_empty() && !name.contains(['*', '?', '!']) && !hosts.iter().any(|h| h == name) {
                hosts.push(name.to_owned());
            }
        }
    }
    hosts
}

/// `~/.ssh/config` host aliases (empty if there is none).
pub fn load_ssh_hosts(home: &Path) -> Vec<String> {
    std::fs::read_to_string(home.join(".ssh").join("config"))
        .map(|c| ssh_hosts(&c))
        .unwrap_or_default()
}

/// Remember `dir` as the most recently used, newest first, without duplicates.
pub fn push_recent(list: &mut Vec<PathBuf>, dir: PathBuf, cap: usize) {
    list.retain(|d| *d != dir);
    list.insert(0, dir);
    list.truncate(cap);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, sub: &str) -> PaletteItem {
        PaletteItem::new("Command", title, sub, || {})
    }

    #[test]
    fn fuzzy_matches_subsequences_in_order() {
        let (_, idx) = fuzzy("spr", "Split right").unwrap();
        assert_eq!(idx, vec![0, 1, 6]); // S, p, r(ight)
        assert!(fuzzy("xyz", "Split right").is_none());
        assert!(fuzzy("rs", "Split right").is_none()); // order matters
        assert_eq!(fuzzy("", "anything").unwrap().1, Vec::<usize>::new());
        assert!(fuzzy("a  b", "a b").is_some()); // spaces in the query are ignored
        assert!(fuzzy("longer than text", "hi").is_none());
    }

    #[test]
    fn better_matches_score_higher() {
        let s = |q: &str, t: &str| fuzzy(q, t).unwrap().0;
        assert!(s("split", "Split right") > s("split", "Spreadsheet link tool")); // consecutive
        assert!(s("pr", "Copy path reload") < s("pr", "Print")); // start of word / text
        assert!(s("cp", "Copy path") > s("cp", "Collapse panes everywhere")); // word starts + short
        // Word-start bonus: "sr" prefers Split Right over a mid-word hit.
        assert!(s("sr", "Split Right") > s("sr", "Disrupt"));
    }

    #[test]
    fn ranking_prefers_titles_and_keeps_order_for_empty_queries() {
        let items = vec![
            item("Reload config", "Ctrl+R"),
            item("Split right", "creates a pane"),
            item("Copy path", "to clipboard"),
        ];
        let all = rank(&items, "", 10);
        assert_eq!(all.iter().map(|r| r.item).collect::<Vec<_>>(), vec![0, 1, 2]);

        let r = rank(&items, "path", 10);
        assert_eq!(r[0].item, 2);
        assert_eq!(r[0].title_matches, vec![5, 6, 7, 8]);

        // Subtitle-only matches are found, after title matches.
        let r = rank(&items, "clip", 10);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].item, 2);
        assert!(r[0].title_matches.is_empty());

        assert_eq!(rank(&items, "e", 2).len(), 2); // limit
        assert!(rank(&items, "zzzz", 10).is_empty());
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(shell_quote("/home/greg/code"), "/home/greg/code");
        assert_eq!(shell_quote("~/code/x-y_z.rs"), "~/code/x-y_z.rs");
        assert_eq!(shell_quote("/mnt/my dir"), "'/mnt/my dir'");
        assert_eq!(shell_quote("/tmp/it's"), "\"/tmp/it's\"");
        assert_eq!(shell_quote("/tmp/it's $HOME"), "\"/tmp/it's \\$HOME\"");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn ssh_config_hosts() {
        let cfg = "\
# comment
Host box
    HostName 10.0.0.1
Host web-* !skip
Host prod staging \"quoted\"
host lower
  Host indented
Match host x
Host box
";
        assert_eq!(ssh_hosts(cfg), vec!["box", "prod", "staging", "quoted", "lower", "indented"]);
        assert!(ssh_hosts("").is_empty());
    }

    #[test]
    fn recent_directories_are_newest_first_and_unique() {
        let mut l = Vec::new();
        for d in ["/a", "/b", "/a", "/c"] {
            push_recent(&mut l, PathBuf::from(d), 3);
        }
        assert_eq!(l, vec![PathBuf::from("/c"), PathBuf::from("/a"), PathBuf::from("/b")]);
        push_recent(&mut l, PathBuf::from("/d"), 3);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0], PathBuf::from("/d"));
    }
}
