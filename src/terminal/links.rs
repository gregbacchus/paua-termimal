//! URL detection in the visible terminal rows: plain-text URLs (a regular expression over the text,
//! following soft-wrapped lines) and explicit OSC 8 hyperlinks (looked up per cell in `mouse.rs`).
//! Detection runs only for the cell under the pointer, while the open-link modifier is held.

use regex::Regex;

use super::render::Cell;

/// Rows followed up or down from the pointer's row when a long URL is soft-wrapped.
const MAX_WRAPPED_ROWS: usize = 12;

/// A link under the pointer: where to take the user, and which cells to underline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    /// `(viewport row, first column, number of columns)`, one entry per row the link covers.
    pub spans: Vec<(usize, usize, usize)>,
}

/// Which key must be held to follow links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrlModifier {
    #[default]
    Ctrl,
    Alt,
    CtrlShift,
}

/// Resolved settings handed to the terminal.
#[derive(Debug, Clone)]
pub struct UrlStyle {
    pub enabled: bool,
    pub modifier: UrlModifier,
    pub schemes: Vec<String>,
    pub osc8: bool,
    pub color: Option<crate::config::Tint>,
}

impl Default for UrlStyle {
    fn default() -> Self {
        Self {
            enabled: true,
            modifier: UrlModifier::Ctrl,
            schemes: ["http", "https", "ftp", "ftps", "file", "mailto"].iter().map(|s| (*s).to_owned()).collect(),
            osc8: true,
            color: None,
        }
    }
}

impl UrlStyle {
    /// Is `url` one we are willing to open (its scheme is in the list)?
    pub fn allows(&self, url: &str) -> bool {
        let lower = url.to_ascii_lowercase();
        self.schemes.iter().any(|s| lower.starts_with(&format!("{}:", s.to_ascii_lowercase())))
    }
}

/// The expression that finds URLs of the given schemes.
pub fn url_regex(schemes: &[String]) -> Option<Regex> {
    let alt: Vec<String> = schemes.iter().filter(|s| !s.is_empty()).map(|s| regex::escape(s)).collect();
    if alt.is_empty() {
        return None;
    }
    Regex::new(&format!(r#"(?i)\b(?:{}):[^\s<>"'`\x00-\x1f]+"#, alt.join("|"))).ok()
}

/// Remove punctuation that belongs to the sentence around a URL, not to the URL: trailing
/// `. , ; : ! ? ' "`, and a closing bracket without a matching opening one inside the URL.
pub fn trim_url(s: &str) -> &str {
    let mut end = s.len();
    loop {
        let t = &s[..end];
        let Some(last) = t.chars().next_back() else { break };
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' => t.matches(')').count() > t.matches('(').count(),
            ']' => t.matches(']').count() > t.matches('[').count(),
            '}' => t.matches('}').count() > t.matches('{').count(),
            _ => false,
        };
        if !strip {
            break;
        }
        end -= last.len_utf8();
    }
    &s[..end]
}

/// One cell's worth of text and where it sits.
struct Piece {
    row: usize,
    col: usize,
    cols: usize,
}

/// The URL under viewport cell (`row`, `col`), if any. `wrapped[r]` says row `r` continues on the
/// next row.
pub fn find_link(rows: &[Vec<Cell>], wrapped: &[bool], row: usize, col: usize, re: &Regex) -> Option<Link> {
    if row >= rows.len() {
        return None;
    }
    let is_wrapped = |r: usize| wrapped.get(r).copied().unwrap_or(false);
    let mut first = row;
    while first > 0 && is_wrapped(first - 1) && row - first < MAX_WRAPPED_ROWS {
        first -= 1;
    }
    let mut last = row;
    while last + 1 < rows.len() && is_wrapped(last) && last - row < MAX_WRAPPED_ROWS {
        last += 1;
    }

    // The logical line as text, with a piece (cell) for every character.
    let mut text = String::new();
    let mut pieces: Vec<Piece> = Vec::new();
    let mut char_piece: Vec<usize> = Vec::new();
    for (r, cells) in rows.iter().enumerate().take(last + 1).skip(first) {
        for (c, cell) in cells.iter().enumerate() {
            if cell.skip {
                continue;
            }
            let t = if cell.text.is_empty() { " " } else { cell.text.as_str() };
            pieces.push(Piece { row: r, col: c, cols: if cell.wide { 2 } else { 1 } });
            for ch in t.chars() {
                text.push(ch);
                char_piece.push(pieces.len() - 1);
            }
        }
    }
    let target = pieces.iter().position(|p| p.row == row && col >= p.col && col < p.col + p.cols)?;

    for m in re.find_iter(&text) {
        let url = trim_url(m.as_str());
        // A bare `scheme:` with nothing after it isn't a URL; `://` is required except for mailto.
        let lower = url.to_ascii_lowercase();
        let after = match (lower.find("://"), lower.strip_prefix("mailto:")) {
            (Some(i), _) => &lower[i + 3..],
            (None, Some(rest)) => rest,
            _ => continue,
        };
        if after.is_empty() {
            continue;
        }
        let start_char = text[..m.start()].chars().count();
        let end_char = start_char + url.chars().count();
        let covered = &char_piece[start_char..end_char];
        if !covered.contains(&target) {
            continue;
        }
        // Underline spans: the extent of the covered pieces on each row.
        let mut spans: Vec<(usize, usize, usize)> = Vec::new();
        for &pi in covered {
            let p = &pieces[pi];
            match spans.last_mut() {
                Some((r, c, n)) if *r == p.row => *n = (p.col + p.cols) - *c,
                _ => spans.push((p.row, p.col, p.cols)),
            }
        }
        return Some(Link { url: url.to_owned(), spans });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Rgb;

    fn row(s: &str, width: usize) -> Vec<Cell> {
        let mut cells: Vec<Cell> = s
            .chars()
            .map(|c| Cell {
                text: c.to_string(),
                fg: Rgb(255, 255, 255),
                bg: None,
                bold: false,
                italic: false,
                underline: false,
                strike: false,
                wide: false,
                skip: false,
            })
            .collect();
        while cells.len() < width {
            cells.push(Cell { text: String::new(), ..cells.first().cloned().unwrap_or_else(|| row("x", 1).remove(0)) });
        }
        cells
    }

    fn re() -> Regex {
        url_regex(&UrlStyle::default().schemes).unwrap()
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_url() {
        assert_eq!(trim_url("https://a.com/x."), "https://a.com/x");
        assert_eq!(trim_url("https://a.com/x),"), "https://a.com/x");
        assert_eq!(trim_url("https://en.wikipedia.org/wiki/Rust_(language)"), "https://en.wikipedia.org/wiki/Rust_(language)");
        assert_eq!(trim_url("https://a.com/?q=1&b=2!"), "https://a.com/?q=1&b=2");
        assert_eq!(trim_url("https://a.com/a]"), "https://a.com/a");
        assert_eq!(trim_url("https://a.com/[x]"), "https://a.com/[x]");
    }

    #[test]
    fn finds_the_url_under_the_pointer_only() {
        let rows = vec![row("see https://example.com/a?b=1, and http://x.org ok", 60)];
        let w = vec![false];
        let l = find_link(&rows, &w, 0, 10, &re()).unwrap();
        assert_eq!(l.url, "https://example.com/a?b=1");
        assert_eq!(l.spans, vec![(0, 4, 25)]);
        assert_eq!(find_link(&rows, &w, 0, 2, &re()), None); // on "see"
        assert_eq!(find_link(&rows, &w, 0, 29, &re()), None); // on ", and"
        let l2 = find_link(&rows, &w, 0, 40, &re()).unwrap();
        assert_eq!(l2.url, "http://x.org");
        assert_eq!(find_link(&rows, &w, 5, 0, &re()), None); // out of range
    }

    #[test]
    fn urls_wrapped_over_several_rows_are_one_link() {
        let rows = vec![row("go to https://example.com/very/lo", 33), row("ng/path/here and more", 33)];
        let w = vec![true, false];
        for (r, c) in [(0, 10), (1, 3)] {
            let l = find_link(&rows, &w, r, c, &re()).unwrap();
            assert_eq!(l.url, "https://example.com/very/long/path/here");
            assert_eq!(l.spans, vec![(0, 6, 27), (1, 0, 12)]);
        }
        // Without the wrap flag they are separate.
        let l = find_link(&rows, &[false, false], 0, 10, &re()).unwrap();
        assert_eq!(l.url, "https://example.com/very/lo");
    }

    #[test]
    fn schemes_are_configurable_and_incomplete_urls_ignored() {
        let rows = vec![row("mailto:me@x.org https:// ftp://h/f ssh://h", 60)];
        let w = vec![false];
        assert_eq!(find_link(&rows, &w, 0, 3, &re()).unwrap().url, "mailto:me@x.org");
        assert_eq!(find_link(&rows, &w, 0, 20, &re()), None); // `https://` alone
        assert_eq!(find_link(&rows, &w, 0, 29, &re()).unwrap().url, "ftp://h/f");
        assert_eq!(find_link(&rows, &w, 0, 38, &re()), None); // ssh isn't enabled by default
        let ssh = url_regex(&["ssh".to_owned()]).unwrap();
        assert_eq!(find_link(&rows, &w, 0, 38, &ssh).unwrap().url, "ssh://h");
        assert!(url_regex(&[]).is_none());
    }

    #[test]
    fn only_allowed_schemes_are_opened() {
        let s = UrlStyle::default();
        assert!(s.allows("https://x") && s.allows("HTTP://x") && s.allows("mailto:a@b.c") && s.allows("file:///tmp"));
        assert!(!s.allows("javascript:alert(1)") && !s.allows("ssh://h") && !s.allows("x-evil://a"));
    }
}
