//! A running-command timer for the pane header.
//!
//! Two signals say a command is running, and the terminal widget picks the best one:
//!
//! - **Shell integration** (OSC 133 markers, e.g. nushell by default): exact start and end times
//!   and the exit status, and it also covers shell builtins such as nushell's `sleep`.
//! - **The foreground process group** on the pane's pty differing from the shell's own: works with
//!   any shell, but only sees external programs.
//!
//! The tracker is a small pure state machine: it is fed the current state every poll and says what
//! the header should show. After a long command ends it can keep showing "took 12s" for a moment.

use std::time::Duration;

/// Settings (see `panes.header.command_timer` in the config).
#[derive(Clone, Debug, PartialEq)]
pub struct TimerConfig {
    pub enabled: bool,
    /// Only show the timer once the command has run this long.
    pub threshold: Duration,
    /// How long "took 12s" stays after a long command ends (zero = don't show it).
    pub linger: Duration,
    /// Full-screen programs (vim, less, htop...) aren't "commands" to time.
    pub ignore_alt_screen: bool,
    /// Program names never timed.
    pub ignore: Vec<String>,
    /// Add the exit status to the "took" message when the shell reports a failure.
    pub show_exit_status: bool,
}

impl Default for TimerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: Duration::from_secs(2),
            linger: Duration::from_secs(4),
            ignore_alt_screen: true,
            ignore: ["ssh", "mosh", "mosh-client", "tmux", "screen", "zellij"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            show_exit_status: true,
        }
    }
}

/// What the header shows.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Shown {
    #[default]
    Hidden,
    /// A long-running command: its elapsed time, e.g. `12s`.
    Running(String),
    /// A long command that just finished, e.g. `took 12s`; `failed` if it reported a non-zero
    /// exit status.
    Done { text: String, failed: bool },
}

/// `7s`, `2m 05s`, `1h 02m 03s`.
pub fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m {:02}s", s / 3600, (s % 3600) / 60, s % 60),
    }
}

/// The program's name from an `argv[0]` (login-shell dashes and directories stripped).
fn base(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name).trim_start_matches('-')
}

/// The command currently running, as the poller sees it.
#[derive(Clone, Copy, Debug)]
pub struct Running<'a> {
    /// Identifies the command: a new value means a new command (the foreground process group's
    /// leader, or a counter bumped by each OSC 133 `C`).
    pub key: i64,
    /// The program in the foreground, if there is one besides the shell (empty for a builtin).
    pub name: &'a str,
    /// When it started, if known exactly (OSC 133); otherwise the first poll that saw it counts.
    pub since_us: Option<i64>,
}

/// The shell said a command ended (OSC 133 `D`).
#[derive(Clone, Copy, Debug)]
pub struct Finished {
    pub at_us: i64,
    pub exit: Option<i32>,
}

#[derive(Debug, Default)]
pub struct Tracker {
    key: Option<i64>,
    /// When the current command started (monotonic microseconds).
    started_us: Option<i64>,
    /// "took ..." text, when it stops showing, and whether it failed.
    done: Option<(String, i64, bool)>,
}

impl Tracker {
    /// Feed the current state; returns what to show. `running` is the command in progress (None
    /// at the prompt), `finished` an end marker seen since the last call, and `now_us` a monotonic
    /// clock.
    pub fn update(
        &mut self,
        now_us: i64,
        running: Option<Running>,
        finished: Option<Finished>,
        alt_screen: bool,
        cfg: &TimerConfig,
    ) -> Shown {
        if !cfg.enabled {
            *self = Self::default();
            return Shown::Hidden;
        }
        let threshold_us = cfg.threshold.as_micros() as i64;
        let linger_us = cfg.linger.as_micros() as i64;

        let active = running.filter(|r| {
            !(cfg.ignore_alt_screen && alt_screen) && !cfg.ignore.iter().any(|i| i == base(r.name))
        });

        if let Some(r) = active {
            if self.key != Some(r.key) {
                // A different command took over: forget the previous one (and its "took").
                self.key = Some(r.key);
                self.started_us = Some(r.since_us.unwrap_or(now_us));
                self.done = None;
            }
            let elapsed = (now_us - self.started_us.unwrap_or(now_us)).max(0);
            return if elapsed >= threshold_us {
                Shown::Running(format_elapsed(Duration::from_micros(elapsed as u64)))
            } else {
                Shown::Hidden
            };
        }

        // Nothing is running (any more).
        self.key = None;
        if let Some(start) = self.started_us.take() {
            // Use the shell's own end time and exit status when it gave them.
            let (end, exit) = match finished {
                Some(f) => (f.at_us.max(start), f.exit),
                None => (now_us, None),
            };
            let elapsed = end - start;
            if elapsed >= threshold_us && linger_us > 0 {
                let failed = exit.is_some_and(|c| c != 0);
                let mut text = format!("took {}", format_elapsed(Duration::from_micros(elapsed as u64)));
                if failed && cfg.show_exit_status {
                    if let Some(c) = exit {
                        text += &format!(" \u{b7} exit {c}");
                    }
                }
                self.done = Some((text, now_us + linger_us, failed));
            }
        }
        match &self.done {
            Some((text, until, failed)) if now_us < *until => Shown::Done { text: text.clone(), failed: *failed },
            _ => {
                self.done = None;
                Shown::Hidden
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000;

    fn run(key: i64, name: &str) -> Option<Running<'_>> {
        Some(Running { key, name, since_us: None })
    }

    fn done(text: &str, failed: bool) -> Shown {
        Shown::Done { text: text.into(), failed }
    }

    #[test]
    fn elapsed_formatting() {
        let f = |s: u64| format_elapsed(Duration::from_secs(s));
        assert_eq!((f(0).as_str(), f(7).as_str(), f(59).as_str()), ("0s", "7s", "59s"));
        assert_eq!((f(60).as_str(), f(125).as_str(), f(3599).as_str()), ("1m 00s", "2m 05s", "59m 59s"));
        assert_eq!((f(3600).as_str(), f(3723).as_str()), ("1h 00m 00s", "1h 02m 03s"));
    }

    #[test]
    fn shows_only_after_the_threshold_and_counts_up() {
        let cfg = TimerConfig::default(); // 2s threshold
        let mut t = Tracker::default();
        assert_eq!(t.update(0, None, None, false, &cfg), Shown::Hidden);
        assert_eq!(t.update(10 * S, run(100, "sleep"), None, false, &cfg), Shown::Hidden); // just started
        assert_eq!(t.update(11 * S, run(100, "sleep"), None, false, &cfg), Shown::Hidden); // 1s
        assert_eq!(t.update(12 * S, run(100, "sleep"), None, false, &cfg), Shown::Running("2s".into()));
        assert_eq!(t.update(75 * S, run(100, "sleep"), None, false, &cfg), Shown::Running("1m 05s".into()));
    }

    #[test]
    fn a_known_start_time_is_used_instead_of_the_first_poll() {
        let cfg = TimerConfig::default();
        let mut t = Tracker::default();
        // OSC 133 said the command began at t=10s; the first poll only happens at 11.9s.
        let r = Some(Running { key: -1, name: "", since_us: Some(10 * S) });
        assert_eq!(t.update(11 * S + 900_000, r, None, false, &cfg), Shown::Hidden); // 1.9s
        assert_eq!(t.update(12 * S, r, None, false, &cfg), Shown::Running("2s".into()));
    }

    #[test]
    fn a_long_command_leaves_a_took_message_that_fades() {
        let cfg = TimerConfig::default(); // linger 4s
        let mut t = Tracker::default();
        t.update(0, run(7, "make"), None, false, &cfg);
        assert_eq!(t.update(9 * S, run(7, "make"), None, false, &cfg), Shown::Running("9s".into()));
        assert_eq!(t.update(10 * S, None, None, false, &cfg), done("took 10s", false));
        assert_eq!(t.update(13 * S, None, None, false, &cfg), done("took 10s", false));
        assert_eq!(t.update(15 * S, None, None, false, &cfg), Shown::Hidden); // lingered 4s
        assert_eq!(t.update(16 * S, None, None, false, &cfg), Shown::Hidden);

        // Short commands leave nothing behind.
        t.update(20 * S, run(8, "ls"), None, false, &cfg);
        assert_eq!(t.update(21 * S, None, None, false, &cfg), Shown::Hidden);

        // linger = 0 turns the message off.
        let quiet = TimerConfig { linger: Duration::ZERO, ..TimerConfig::default() };
        let mut t = Tracker::default();
        t.update(0, run(9, "make"), None, false, &quiet);
        assert_eq!(t.update(30 * S, None, None, false, &quiet), Shown::Hidden);
    }

    #[test]
    fn the_shells_end_time_and_exit_status_are_used() {
        let cfg = TimerConfig::default();
        let mut t = Tracker::default();
        let start = Some(Running { key: -1, name: "", since_us: Some(0) });
        t.update(5 * S, start, None, false, &cfg);
        // The command actually ended at 7s; we only polled at 7.2s.
        let fin = Some(Finished { at_us: 7 * S, exit: Some(0) });
        assert_eq!(t.update(7 * S + 200_000, None, fin, false, &cfg), done("took 7s", false));

        // A failure says so (and is flagged for styling).
        let mut t = Tracker::default();
        t.update(0, start, None, false, &cfg);
        let bad = Some(Finished { at_us: 6 * S, exit: Some(2) });
        assert_eq!(t.update(6 * S, None, bad, false, &cfg), done("took 6s \u{b7} exit 2", true));
        // ...unless exit statuses are turned off (it is still flagged).
        let plain = TimerConfig { show_exit_status: false, ..TimerConfig::default() };
        let mut t = Tracker::default();
        t.update(0, start, None, false, &plain);
        assert_eq!(t.update(6 * S, None, bad, false, &plain), done("took 6s", true));
        // No status reported: just the time.
        let mut t = Tracker::default();
        t.update(0, start, None, false, &cfg);
        let none = Some(Finished { at_us: 6 * S, exit: None });
        assert_eq!(t.update(6 * S, None, none, false, &cfg), done("took 6s", false));
    }

    #[test]
    fn a_new_command_restarts_the_clock_and_clears_the_old_message() {
        let cfg = TimerConfig::default();
        let mut t = Tracker::default();
        t.update(0, run(1, "a"), None, false, &cfg);
        assert_eq!(t.update(5 * S, None, None, false, &cfg), done("took 5s", false));
        // The next command begins while "took 5s" is still showing.
        assert_eq!(t.update(6 * S, run(2, "b"), None, false, &cfg), Shown::Hidden);
        assert_eq!(t.update(9 * S, run(2, "b"), None, false, &cfg), Shown::Running("3s".into()));
    }

    #[test]
    fn full_screen_programs_and_ignored_names_are_not_timed() {
        let cfg = TimerConfig::default();
        let mut t = Tracker::default();
        // vim on the alternate screen.
        t.update(0, run(3, "vim"), None, true, &cfg);
        assert_eq!(t.update(60 * S, run(3, "vim"), None, true, &cfg), Shown::Hidden);
        // Leaving vim: no "took 60s" for it.
        assert_eq!(t.update(61 * S, None, None, false, &cfg), Shown::Hidden);
        // ssh is ignored by default, even with a path or login dash.
        let mut t = Tracker::default();
        t.update(0, run(4, "/usr/bin/ssh"), None, false, &cfg);
        assert_eq!(t.update(60 * S, run(4, "/usr/bin/ssh"), None, false, &cfg), Shown::Hidden);
        // ...but timed once removed from the ignore list.
        let timed = TimerConfig { ignore: vec![], ..TimerConfig::default() };
        let mut t = Tracker::default();
        t.update(0, run(4, "ssh"), None, false, &timed);
        assert_eq!(t.update(60 * S, run(4, "ssh"), None, false, &timed), Shown::Running("1m 00s".into()));
        // The alt-screen rule can be turned off.
        let alt = TimerConfig { ignore_alt_screen: false, ..TimerConfig::default() };
        let mut t = Tracker::default();
        t.update(0, run(5, "vim"), None, true, &alt);
        assert_eq!(t.update(10 * S, run(5, "vim"), None, true, &alt), Shown::Running("10s".into()));
    }

    #[test]
    fn disabled_shows_nothing() {
        let off = TimerConfig { enabled: false, ..TimerConfig::default() };
        let mut t = Tracker::default();
        t.update(0, run(1, "x"), None, false, &off);
        assert_eq!(t.update(100 * S, run(1, "x"), None, false, &off), Shown::Hidden);
    }
}
