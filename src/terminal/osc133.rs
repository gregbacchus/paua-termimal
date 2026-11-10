//! Spotting OSC 133 "semantic prompt" markers in the shell's output.
//!
//! Shells with integration (nushell by default, bash/zsh/fish with a snippet) wrap each command:
//!
//! - `OSC 133;A` a prompt starts, `OSC 133;B` the prompt ended (input starts),
//! - `OSC 133;C` the command starts running, `OSC 133;D;<exit code>` it finished.
//!
//! That is the exact signal for timing commands, including shell builtins (which don't show up
//! as a separate process). The scanner is fed the raw bytes as they arrive and copes with a
//! sequence being split across reads.

/// A marker found in the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    PromptStart,
    InputStart,
    CommandStart,
    /// `D`, with the exit status if the shell reported one.
    CommandEnd(Option<i32>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Esc,
    /// Inside `ESC ]`, collecting the payload.
    Osc,
    /// Saw ESC inside the payload (maybe the `ESC \` terminator).
    OscEsc,
}

#[derive(Debug, Default)]
pub struct Scanner {
    state: State,
    buf: Vec<u8>,
}

/// Longest payload we bother to keep (real 133 markers are a handful of bytes).
const MAX_PAYLOAD: usize = 96;

impl Scanner {
    /// Feed more output; returns the markers completed by it, in order.
    pub fn scan(&mut self, data: &[u8]) -> Vec<Marker> {
        let mut found = Vec::new();
        for &b in data {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    }
                }
                State::Esc => match b {
                    b']' => {
                        self.buf.clear();
                        self.state = State::Osc;
                    }
                    0x1b => {} // stay
                    _ => self.state = State::Ground,
                },
                State::Osc => match b {
                    0x07 => self.finish(&mut found),
                    0x1b => self.state = State::OscEsc,
                    _ => {
                        self.buf.push(b);
                        // Other OSCs (title, cwd...) can be long: drop out as soon as it is clear
                        // this isn't 133, instead of buffering it.
                        let prefix = b"133;";
                        let n = self.buf.len().min(prefix.len());
                        if self.buf[..n] != prefix[..n] || self.buf.len() > MAX_PAYLOAD {
                            self.buf.clear();
                            self.state = State::Ground;
                        }
                    }
                },
                State::OscEsc => {
                    if b == b'\\' {
                        self.finish(&mut found);
                    } else {
                        self.buf.clear();
                        self.state = if b == 0x1b { State::Esc } else { State::Ground };
                    }
                }
            }
        }
        found
    }

    fn finish(&mut self, found: &mut Vec<Marker>) {
        self.state = State::Ground;
        let payload = std::mem::take(&mut self.buf);
        let Some(rest) = payload.strip_prefix(b"133;") else { return };
        let mut parts = rest.split(|b| *b == b';');
        match parts.next() {
            Some(b"A") => found.push(Marker::PromptStart),
            Some(b"B") => found.push(Marker::InputStart),
            Some(b"C") => found.push(Marker::CommandStart),
            Some(b"D") => {
                let code = parts
                    .next()
                    .and_then(|p| std::str::from_utf8(p).ok())
                    .and_then(|s| s.trim().parse::<i32>().ok());
                found.push(Marker::CommandEnd(code));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_markers_with_either_terminator() {
        let mut s = Scanner::default();
        let m = s.scan(b"\x1b]133;A\x1b\\prompt text\x1b]133;B\x07\x1b]133;C\x1b\\output\x1b]133;D;0\x07");
        assert_eq!(
            m,
            vec![Marker::PromptStart, Marker::InputStart, Marker::CommandStart, Marker::CommandEnd(Some(0))]
        );
    }

    #[test]
    fn exit_codes_and_extra_parameters() {
        let mut s = Scanner::default();
        assert_eq!(s.scan(b"\x1b]133;D;130\x1b\\"), vec![Marker::CommandEnd(Some(130))]);
        assert_eq!(s.scan(b"\x1b]133;D\x07"), vec![Marker::CommandEnd(None)]);
        assert_eq!(s.scan(b"\x1b]133;D;2;aid=42\x07"), vec![Marker::CommandEnd(Some(2))]);
        assert_eq!(s.scan(b"\x1b]133;A;k=i\x07"), vec![Marker::PromptStart]);
        assert_eq!(s.scan(b"\x1b]133;D;abc\x07"), vec![Marker::CommandEnd(None)]);
    }

    #[test]
    fn a_marker_split_across_reads_is_still_found() {
        let mut s = Scanner::default();
        let whole = b"xx\x1b]133;D;7\x1b\\yy";
        for cut in 1..whole.len() {
            let mut s = Scanner::default();
            let mut got = s.scan(&whole[..cut]);
            got.extend(s.scan(&whole[cut..]));
            assert_eq!(got, vec![Marker::CommandEnd(Some(7))], "cut at {cut}");
        }
        // One byte at a time, too.
        let mut got = Vec::new();
        for b in whole {
            got.extend(s.scan(std::slice::from_ref(b)));
        }
        assert_eq!(got, vec![Marker::CommandEnd(Some(7))]);
    }

    #[test]
    fn other_sequences_are_ignored_and_dont_derail_the_scanner() {
        let mut s = Scanner::default();
        // A long title (OSC 2), a cwd report (OSC 7), colour escapes, then a real marker.
        let long_title = format!("\x1b]2;{}\x07", "t".repeat(500));
        let data = format!("{long_title}\x1b]7;file://host/tmp\x1b\\\x1b[38;2;1;2;3mhi\x1b[0m\x1b]133;C\x07");
        assert_eq!(s.scan(data.as_bytes()), vec![Marker::CommandStart]);
        // An OSC 1337 / 1330 that merely starts with "13" isn't mistaken for 133.
        assert!(s.scan(b"\x1b]1337;SetMark\x07\x1b]1330;C\x07").is_empty());
        // Unknown 133 kinds are skipped; garbage after ESC is harmless.
        assert!(s.scan(b"\x1b]133;Z\x07\x1b[A\x1bx").is_empty());
        assert_eq!(s.scan(b"\x1b]133;C\x07"), vec![Marker::CommandStart]);
    }
}
