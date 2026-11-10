//! PTY + shell spawning. The reader runs on its own thread and ships byte chunks to the GTK main
//! thread over a bounded channel (bounded so a flood like `cat bigfile` applies backpressure).

use std::cell::RefCell;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::rc::Rc;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

pub type SharedWriter = Rc<RefCell<Box<dyn Write + Send>>>;

pub struct Pty {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: SharedWriter,
    pub child: Box<dyn Child + Send + Sync>,
    pub output: async_channel::Receiver<Vec<u8>>,
}

pub fn spawn(
    cols: u16,
    rows: u16,
    shell: Option<&str>,
    cwd: Option<PathBuf>,
    env: &[(String, String)],
) -> anyhow_like::Result<Pty> {
    let pair = native_pty_system().openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let shell = shell
        .map(str::to_owned)
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/sh".to_owned());

    let mut cmd = CommandBuilder::new(shell);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "paua");
    for (k, v) in env {
        cmd.env(k, v); // the user's `terminal.env` wins over the defaults above
    }
    if let Some(cwd) = cwd.or_else(|| std::env::current_dir().ok()) {
        cmd.cwd(cwd);
    }

    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    let (tx, rx) = async_channel::bounded::<Vec<u8>>(32);
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send_blocking(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
        // Dropping `tx` closes the channel, which the main thread treats as "child exited".
    });

    Ok(Pty {
        master: pair.master,
        writer: Rc::new(RefCell::new(writer)),
        child,
        output: rx,
    })
}

/// Tiny local error alias so we don't pull in `anyhow` for the spike.
pub mod anyhow_like {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `terminal.env` reaches the shell and wins over Paua's own variables.
    #[test]
    fn extra_environment_reaches_the_child_and_overrides_defaults() {
        let env = vec![("VTE_VERSION".to_owned(), "7600".to_owned()), ("TERM_PROGRAM".to_owned(), "custom".to_owned())];
        let pty = spawn(80, 24, Some("/usr/bin/env"), None, &env).expect("spawn env");
        let mut out = String::new();
        while let Ok(chunk) = pty.output.recv_blocking() {
            out.push_str(&String::from_utf8_lossy(&chunk));
        }
        assert!(out.contains("VTE_VERSION=7600"), "{out}");
        assert!(out.contains("TERM_PROGRAM=custom") && !out.contains("TERM_PROGRAM=paua"), "{out}");
        assert!(out.contains("COLORTERM=truecolor"));
    }
}
