//! Running external commands for status bar widgets: always off the UI thread and always with a
//! hard timeout. A command that overruns is killed together with everything it started.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gtk::glib;

/// Keep at most this much of a command's output.
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Clone, Default)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    /// The program could not be started (not installed, bad directory...).
    Spawn(String),
    TimedOut,
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Spawn(e) => write!(f, "{e}"),
            ExecError::TimedOut => write!(f, "timed out"),
        }
    }
}

fn reader(mut r: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if buf.len() < MAX_OUTPUT {
                buf.extend_from_slice(&chunk[..n.min(MAX_OUTPUT - buf.len())]);
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    rx
}

/// Run `cmd` (blocking), giving up after `timeout`. Call from a worker thread.
pub fn run(mut cmd: Command, timeout: Duration) -> Result<Output, ExecError> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = cmd.spawn().map_err(|e| ExecError::Spawn(e.to_string()))?;
    let out_rx = reader(child.stdout.take().expect("piped"));
    let err_rx = reader(child.stderr.take().expect("piped"));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= deadline => {
                // SAFETY: plain syscall; the group id is the child's pid (process_group(0)).
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.kill();
                let _ = child.wait();
                return Err(ExecError::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(4)),
            Err(e) => return Err(ExecError::Spawn(e.to_string())),
        }
    };
    // The readers finish when the pipes close; don't wait forever for a stray background process.
    let grace = Duration::from_millis(300);
    Ok(Output {
        stdout: out_rx.recv_timeout(grace).unwrap_or_default(),
        stderr: err_rx.recv_timeout(grace).unwrap_or_default(),
        success: status.success(),
    })
}

/// `sh -c <script>` in `cwd`.
pub fn run_shell(script: &str, cwd: &Path, timeout: Duration) -> Result<Output, ExecError> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    if cwd.is_dir() {
        cmd.current_dir(cwd);
    }
    run(cmd, timeout)
}

/// Run `job` on a worker thread and hand its result to `done` on the main thread (`None` if the
/// job panicked).
pub fn spawn<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(Option<T>) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(job());
    });
    glib::spawn_future_local(async move {
        done(rx.recv().await.ok());
    });
}

/// The first non-empty line of command output, colours removed and shortened.
pub fn first_line(s: &str) -> String {
    let clean = crate::template::strip_ansi(s);
    let line = clean.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_output_and_status() {
        let o = run_shell("echo hi; echo oops >&2; exit 3", Path::new("/"), Duration::from_secs(5)).unwrap();
        assert_eq!(o.stdout.trim(), "hi");
        assert_eq!(o.stderr.trim(), "oops");
        assert!(!o.success);
    }

    #[test]
    fn a_slow_command_is_killed_with_its_children() {
        let t = Instant::now();
        let r = run_shell("sleep 30 & sleep 30; echo never", Path::new("/"), Duration::from_millis(200));
        assert_eq!(r.unwrap_err(), ExecError::TimedOut);
        assert!(t.elapsed() < Duration::from_secs(3), "took {:?}", t.elapsed());
    }

    #[test]
    fn a_missing_program_is_a_spawn_error() {
        let r = run(Command::new("definitely-not-a-program-xyz"), Duration::from_secs(1));
        assert!(matches!(r, Err(ExecError::Spawn(_))));
    }

    #[test]
    fn first_line_cleans_up() {
        assert_eq!(first_line("\n \u{1b}[32mv20.1\u{1b}[0m  \nnext"), "v20.1");
        assert_eq!(first_line(""), "");
    }
}
