//! Tiny text templates for status bar widgets.
//!
//! - `{name}` the value of `name` (empty if unknown)
//! - `{a|b|c}` the first of these that is not empty
//! - `{?name:text}` `text` only if `name` is set (not empty, not `0`, not `false`); `text` may
//!   contain more placeholders, e.g. `{?exit:failed with {exit}}`
//! - `{!name:text}` the opposite
//! - `{{` and `}}` write a literal brace

pub fn render(t: &str, get: &dyn Fn(&str) -> Option<String>) -> String {
    let b = t.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' if b.get(i + 1) == Some(&b'{') => {
                out.push('{');
                i += 2;
            }
            b'}' if b.get(i + 1) == Some(&b'}') => {
                out.push('}');
                i += 2;
            }
            b'{' => {
                let (mut depth, mut end) = (0, None);
                for (j, &c) in b.iter().enumerate().skip(i) {
                    match c {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(j);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                match end {
                    Some(e) => {
                        out.push_str(&expand(&t[i + 1..e], get));
                        i = e + 1;
                    }
                    None => {
                        out.push_str(&t[i..]); // unbalanced: keep as text
                        break;
                    }
                }
            }
            _ => {
                let next = t[i..].find(['{', '}']).map_or(t.len(), |p| i + p.max(1));
                out.push_str(&t[i..next]);
                i = next;
            }
        }
    }
    out
}

fn truthy(v: &str) -> bool {
    !(v.is_empty() || v == "0" || v == "false")
}

fn expand(inner: &str, get: &dyn Fn(&str) -> Option<String>) -> String {
    if let Some(rest) = inner.strip_prefix('?').or_else(|| inner.strip_prefix('!')) {
        let negate = inner.starts_with('!');
        let (key, body) = rest.split_once(':').unwrap_or((rest, ""));
        let set = truthy(&get(key.trim()).unwrap_or_default());
        return if set != negate { render(body, get) } else { String::new() };
    }
    if inner.contains('|') {
        return inner
            .split('|')
            .filter_map(|k| get(k.trim()))
            .find(|v| !v.is_empty())
            .unwrap_or_default();
    }
    get(inner.trim()).unwrap_or_default()
}

/// Remove terminal escape sequences (colours etc.) from command output.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => {
                for c in it.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = it.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(k: &str) -> Option<String> {
        match k {
            "a" => Some("A".into()),
            "empty" => Some(String::new()),
            "zero" => Some("0".into()),
            "n" => Some("3".into()),
            _ => None,
        }
    }

    #[test]
    fn plain_values_fallbacks_and_conditionals() {
        let r = |t| render(t, &vars);
        assert_eq!(r("x {a} y"), "x A y");
        assert_eq!(r("{missing}!"), "!");
        assert_eq!(r("{empty|missing|a}"), "A");
        assert_eq!(r("{?n:up {n}}"), "up 3");
        assert_eq!(r("{?zero:hidden}{?empty:hidden}{?missing:hidden}"), "");
        assert_eq!(r("{!zero:shown}"), "shown");
        assert_eq!(r("{?a:[{?n:{n}}]}"), "[3]");
        assert_eq!(r("{{literal}} {a}"), "{literal} A");
        assert_eq!(r("héllo {a} ünï"), "héllo A ünï");
        assert_eq!(r("open {a"), "open {a");
    }

    #[test]
    fn ansi_is_stripped() {
        assert_eq!(strip_ansi("\u{1b}[1;32mok\u{1b}[0m done"), "ok done");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}text"), "text");
        assert_eq!(strip_ansi("plain"), "plain");
    }
}
