//! Find in the terminal's output: extract the text of every row (scrollback + screen), search it,
//! and report matches as (screen row, display column, width) so the renderer can highlight them.
//!
//! The emulator has no search API, so the text comes from formatting a "select all" selection
//! with soft-wrapped rows kept separate (`unwrap = false`), which gives one line per screen row.
//! Matches that span a soft-wrap boundary are therefore not found.

use libghostty_vt::{
    Terminal,
    fmt::Format,
    selection::FormatOptions,
    terminal::{Point, PointCoordinate},
};
use regex::RegexBuilder;
use unicode_width::UnicodeWidthChar;

/// How letter case is treated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaseMode {
    /// Case-insensitive unless the query contains an uppercase letter.
    #[default]
    Smart,
    Sensitive,
    Insensitive,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindOptions {
    pub query: String,
    pub case: CaseMode,
    pub regex: bool,
}

/// One hit. `row` is a screen row (0 = oldest scrollback line); `col` and `cols` are display
/// columns (wide characters count as two).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub row: usize,
    pub col: usize,
    pub cols: usize,
}

/// Where a find stands, for the search box to display.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindStatus {
    /// A find is active on this terminal.
    pub active: bool,
    /// 1-based index of the current match (0 = none).
    pub current: usize,
    pub total: usize,
    /// The query could not be used (bad regex).
    pub error: Option<String>,
}

/// Highlight colours (translucent, laid under the text).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FindStyle {
    pub matched: crate::config::Tint,
    pub current: crate::config::Tint,
}

impl Default for FindStyle {
    fn default() -> Self {
        use super::Rgb;
        Self {
            matched: crate::config::Tint { rgb: Rgb(255, 196, 60), alpha: 0.32 },
            current: crate::config::Tint { rgb: Rgb(255, 140, 0), alpha: 0.65 },
        }
    }
}

/// Most matches kept (keeps drawing and stepping cheap on huge outputs).
pub const MAX_MATCHES: usize = 50_000;

/// Number of rows at the top (oldest) that hold no text at all. The formatter drops those, so
/// without knowing it every match would be reported that many rows too high.
fn leading_blank_rows(term: &Terminal<'_, '_>) -> usize {
    let (Ok(total), Ok(cols)) = (term.total_rows(), term.cols()) else { return 0 };
    for y in 0..total {
        for x in 0..cols {
            let point = Point::Screen(PointCoordinate { x, y: y as u32 });
            let has_text = term
                .grid_ref(point)
                .and_then(|g| g.cell())
                .and_then(|c| c.has_text())
                .unwrap_or(false);
            if has_text {
                return y;
            }
        }
    }
    total
}

/// Every row of scrollback + screen as text, one string per row, index = screen row (0 = oldest
/// scrollback line). Trailing blank rows may be missing; leading ones are restored as empty
/// strings so indices stay aligned with the screen.
pub fn screen_lines(term: &Terminal<'_, '_>) -> Vec<String> {
    let Ok(Some(sel)) = term.select_all() else { return Vec::new() };
    let opts = FormatOptions::new()
        .with_emit_format(Format::Plain)
        .with_unwrap(false)
        .with_trim(false)
        .with_selection(&sel);
    let Ok(Some(bytes)) = term.format_selection_alloc(None, opts) else { return Vec::new() };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = vec![String::new(); leading_blank_rows(term)];
    lines.extend(text.split('\n').map(str::to_owned));
    lines
}

fn width_of(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Search `lines`. Returns at most `max` matches, in order. An empty query matches nothing; an
/// invalid regex is an error (the message is meant for the user).
pub fn find_matches(lines: &[String], opts: &FindOptions, max: usize) -> Result<Vec<Match>, String> {
    if opts.query.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = if opts.regex { opts.query.clone() } else { regex::escape(&opts.query) };
    let insensitive = match opts.case {
        CaseMode::Sensitive => false,
        CaseMode::Insensitive => true,
        CaseMode::Smart => !opts.query.chars().any(char::is_uppercase),
    };
    let re = RegexBuilder::new(&pattern)
        .case_insensitive(insensitive)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| e.to_string().lines().last().unwrap_or("invalid pattern").trim().to_owned())?;

    let mut out = Vec::new();
    for (row, line) in lines.iter().enumerate() {
        for m in re.find_iter(line) {
            if m.start() == m.end() {
                continue; // empty match (e.g. a regex like `a*`)
            }
            out.push(Match {
                row,
                col: width_of(&line[..m.start()]),
                cols: width_of(m.as_str()).max(1),
            });
            if out.len() >= max {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use libghostty_vt::TerminalOptions;

    fn term_with(text: &[u8], cols: u16, rows: u16) -> Terminal<'static, 'static> {
        let mut t = Terminal::new(TerminalOptions { cols, rows, max_scrollback: 100 }).unwrap();
        t.vt_write(text);
        t
    }

    fn opts(q: &str) -> FindOptions {
        FindOptions { query: q.into(), ..Default::default() }
    }

    #[test]
    fn rows_line_up_with_the_screen_including_scrollback() {
        // 3 visible rows, 6 lines written: 3 scroll into history.
        let t = term_with(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix", 20, 3);
        let lines = screen_lines(&t);
        let total = t.total_rows().unwrap();
        assert_eq!(lines.len(), total, "one line per screen row: {lines:?}");
        assert_eq!(lines[0].trim_end(), "one");
        assert_eq!(lines[5].trim_end(), "six");

        let found = find_matches(&lines, &opts("e"), 100).unwrap();
        let rows: Vec<usize> = found.iter().map(|m| m.row).collect();
        assert_eq!(rows, vec![0, 2, 2, 4]); // one, three(x2), five
    }

    /// The formatter drops blank rows at the top of the screen; matches must still be reported on
    /// the screen row where the text really is (this used to highlight the row above).
    #[test]
    fn rows_stay_aligned_when_the_top_of_the_screen_is_blank() {
        let row_of = |t: &Terminal<'static, 'static>, q: &str| {
            find_matches(&screen_lines(t), &opts(q), 10).unwrap()[0].row
        };
        // Three blank rows, then text.
        let t = term_with(b"\r\n\r\n\r\nabc needle\r\n\r\n", 30, 8);
        assert_eq!(row_of(&t, "needle"), 3);

        // Text only at the bottom of a cleared screen.
        let t2 = term_with(b"old\r\n\x1b[2J\x1b[7;1Hbottom needle", 30, 8);
        assert_eq!(row_of(&t2, "needle"), 6);

        // Blank rows at the top of the *scrollback* too: 4 blank lines, then 10 numbered lines on
        // a 5-row screen, so the blanks have scrolled into history.
        let mut text = String::from("\r\n\r\n\r\n\r\n");
        for i in 0..10 {
            text += &format!("line {i}\r\n");
        }
        let t3 = term_with(text.as_bytes(), 30, 5);
        assert_eq!(row_of(&t3, "line 0"), 4);
        assert_eq!(row_of(&t3, "line 9"), 13);
        assert_eq!(t3.total_rows().unwrap(), 15);

        // The match's screen row and the viewport agree: with the viewport at the bottom
        // (offset 10), screen row 13 is viewport row 3, and the cell under the match is the "9".
        let top = t3.scrollbar().unwrap().offset as usize;
        assert_eq!(top, 10);
        let m = find_matches(&screen_lines(&t3), &opts("line 9"), 10).unwrap()[0];
        let cell = t3
            .grid_ref(Point::Viewport(PointCoordinate { x: (m.col + 5) as u16, y: (m.row - top) as u32 }))
            .unwrap();
        let mut buf = ['\0'; 4];
        let n = cell.graphemes(&mut buf).unwrap();
        assert_eq!(&buf[..n], &['9']);

        // Interior blank rows and wrapped rows keep their place too.
        // 20 characters wrap onto rows 0-1, row 2 is blank, "end" is on row 3.
        let t4 = term_with(b"0123456789abcdefghij\r\n\r\nend needle", 10, 6);
        assert_eq!(row_of(&t4, "end"), 3);

        // Nothing at all.
        let t5 = term_with(b"", 10, 3);
        assert!(find_matches(&screen_lines(&t5), &opts("x"), 10).unwrap().is_empty());
    }

    #[test]
    fn columns_account_for_wide_characters() {
        let t = term_with("\u{65e5}\u{672c} needle".as_bytes(), 40, 3); // two wide chars, space, needle
        let lines = screen_lines(&t);
        let m = find_matches(&lines, &opts("needle"), 10).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].row, m[0].col, m[0].cols), (0, 5, 6)); // 2*2 + 1 space
        // A wide match is two columns per character.
        let w = find_matches(&lines, &opts("\u{65e5}\u{672c}"), 10).unwrap();
        assert_eq!((w[0].col, w[0].cols), (0, 4));
    }

    #[test]
    fn case_modes_and_regex() {
        let lines = vec!["Foo foo FOO".to_string()];
        let n = |o: FindOptions| find_matches(&lines, &o, 10).unwrap().len();
        assert_eq!(n(opts("foo")), 3); // smart-case: lowercase query ignores case
        assert_eq!(n(opts("Foo")), 1); // uppercase in the query -> sensitive
        assert_eq!(n(FindOptions { query: "foo".into(), case: CaseMode::Sensitive, regex: false }), 1);
        assert_eq!(n(FindOptions { query: "FOO".into(), case: CaseMode::Insensitive, regex: false }), 3);
        assert_eq!(n(FindOptions { query: "fo+".into(), case: CaseMode::Smart, regex: true }), 3);
        // Literal mode escapes regex syntax; regex mode reports bad patterns.
        assert_eq!(find_matches(&["a.c abc".into()], &opts("a.c"), 10).unwrap().len(), 1);
        assert!(find_matches(&lines, &FindOptions { query: "(".into(), regex: true, ..Default::default() }, 10).is_err());
        assert!(find_matches(&lines, &opts(""), 10).unwrap().is_empty());
    }

    #[test]
    fn match_cap_and_empty_matches() {
        let lines = vec!["aaaa aaaa aaaa".to_string()];
        assert_eq!(find_matches(&lines, &opts("a"), 5).unwrap().len(), 5);
        let r = FindOptions { query: "x*".into(), regex: true, ..Default::default() };
        assert!(find_matches(&lines, &r, 10).unwrap().is_empty()); // only empty matches
    }
}
