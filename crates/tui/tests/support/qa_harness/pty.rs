//! Pseudo-terminal session wrapping `portable-pty`.
//!
//! Spawns a binary in a real PTY, pumps the child's stdout into an in-memory
//! buffer on a background thread, and exposes write/wait/kill primitives
//! the test harness composes.
//!
//! The reader thread is necessary because `portable-pty`'s reader is blocking
//! and the test thread must remain free to send input + poll for screen
//! changes.

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct PtySession {
    /// Held (not read) so the PTY master stays open for the child's lifetime.
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    buffer: Arc<Mutex<Vec<u8>>>,
    reader_handle: Option<JoinHandle<()>>,
}

pub struct PtySessionBuilder<'a> {
    program: &'a Path,
    args: Vec<String>,
    cwd: Option<&'a Path>,
    env: Vec<(String, String)>,
    rows: u16,
    cols: u16,
    clear_env: bool,
}

impl<'a> PtySessionBuilder<'a> {
    pub fn new(program: &'a Path) -> Self {
        Self {
            program,
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            rows: 40,
            cols: 120,
            clear_env: false,
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, p: &'a Path) -> Self {
        self.cwd = Some(p);
        self
    }

    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    /// Wipe the inherited environment before applying explicit `env(..)`
    /// overrides. Use for sealed scenarios that must not see the developer's
    /// real `~/.deepseek/`, `$HOME`, or API keys.
    pub fn clear_env(mut self, yes: bool) -> Self {
        self.clear_env = yes;
        self
    }

    pub fn size(mut self, rows: u16, cols: u16) -> Self {
        self.rows = rows;
        self.cols = cols;
        self
    }

    pub fn spawn(self) -> Result<PtySession> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: self.rows,
                cols: self.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;

        let mut cmd = CommandBuilder::new(self.program);
        for a in &self.args {
            cmd.arg(a);
        }
        if let Some(cwd) = self.cwd {
            cmd.cwd(cwd);
        }
        if self.clear_env {
            cmd.env_clear();
            if let Some(path) = std::env::var_os("PATH") {
                cmd.env("PATH", path);
            }
        }
        // TERM must be set to something xterm-ish so crossterm enables the
        // capabilities the TUI assumes (256 color, bracketed paste, …).
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        for (k, v) in &self.env {
            cmd.env(k, v);
        }

        let child = pair.slave.spawn_command(cmd).context("spawn child")?;
        // Drop the slave end so EOF propagates correctly when the child exits.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("clone reader")?;
        let writer = pair.master.take_writer().context("take writer")?;

        let buffer: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let buf_thread = Arc::clone(&buffer);
        let reader_handle = thread::Builder::new()
            .name("qa-pty-reader".into())
            .spawn(move || {
                let mut chunk = [0u8; 8192];
                loop {
                    match reader.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Ok(mut b) = buf_thread.lock() {
                                b.extend_from_slice(&chunk[..n]);
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .context("reader thread")?;

        Ok(PtySession {
            master: pair.master,
            child,
            writer,
            buffer,
            reader_handle: Some(reader_handle),
        })
    }
}

impl PtySession {
    pub fn builder(program: &Path) -> PtySessionBuilder<'_> {
        PtySessionBuilder::new(program)
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes).context("pty write")?;
        self.writer.flush().context("pty flush")?;
        Ok(())
    }

    /// Drain any bytes the reader thread has pushed into the buffer. Returns
    /// the bytes read this call. Non-blocking — returns immediately even if
    /// the buffer is empty.
    pub fn drain(&mut self) -> Vec<u8> {
        let mut b = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *b)
    }

    /// Block until the child exits or the deadline passes. Returns the exit
    /// status if reaped, or `None` on timeout.
    pub fn wait_until(&mut self, deadline: Instant) -> Option<i32> {
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status.exit_code() as i32),
                Ok(None) => {}
                Err(_) => return None,
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Send SIGTERM-equivalent and wait briefly. Returns the exit status if
    /// the child reaped within `grace`, or `None` otherwise.
    pub fn shutdown(mut self, grace: Duration) -> Option<i32> {
        self.kill_and_join_reader(grace)
    }

    fn kill_and_join_reader(&mut self, grace: Duration) -> Option<i32> {
        let _ = self.child.kill();
        let exit = self.wait_until(Instant::now() + grace);
        if exit.is_some()
            && let Some(handle) = self.reader_handle.take()
        {
            // Don't block on the reader thread forever — it exits on EOF.
            let _ = handle.join();
        }
        exit
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.kill_and_join_reader(Duration::from_secs(2));
    }
}
