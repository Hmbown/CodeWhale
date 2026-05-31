//! End-to-end harness composing [`PtySession`] + [`Frame`].
//!
//! Tests build a [`Harness`] via [`Harness::builder`], drive the TUI with
//! [`Harness::send`] / [`Harness::paste`] / [`Harness::resize`], poll the
//! parsed terminal state with [`Harness::wait_for`], and assert on
//! [`Harness::frame`] / filesystem state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

use super::{Frame, PtySession};

pub struct Harness {
    pty: PtySession,
    frame: Frame,
    last_pump: Instant,
}

pub struct HarnessBuilder {
    program: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    env: HashMap<String, String>,
    rows: u16,
    cols: u16,
    clear_env: bool,
    seal_home: Option<PathBuf>,
}

impl HarnessBuilder {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            rows: 40,
            cols: 120,
            clear_env: false,
            seal_home: None,
        }
    }

    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, p: impl Into<PathBuf>) -> Self {
        self.cwd = Some(p.into());
        self
    }

    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.insert(k.into(), v.into());
        self
    }

    pub fn size(mut self, rows: u16, cols: u16) -> Self {
        self.rows = rows;
        self.cols = cols;
        self
    }

    pub fn clear_env(mut self) -> Self {
        self.clear_env = true;
        self
    }

    /// Point `$HOME` (and `XDG_*` defaults) at a fresh dir so the spawned
    /// binary cannot read or mutate the developer's real `~/.deepseek/`.
    pub fn seal_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.seal_home = Some(home.into());
        self
    }

    pub fn spawn(self) -> Result<Harness> {
        let mut builder = PtySession::builder(&self.program)
            .args(self.args.iter().cloned())
            .size(self.rows, self.cols);
        if self.clear_env {
            builder = builder.clear_env(true);
        }
        if let Some(cwd) = self.cwd.as_deref() {
            builder = builder.cwd(cwd);
        }
        if let Some(home) = self.seal_home.as_deref() {
            std::fs::create_dir_all(home).context("create sealed HOME")?;
            builder = builder
                .env("HOME", home.to_string_lossy())
                .env("XDG_CONFIG_HOME", home.join(".config").to_string_lossy())
                .env("XDG_DATA_HOME", home.join(".local/share").to_string_lossy())
                .env("XDG_CACHE_HOME", home.join(".cache").to_string_lossy())
                .env("USERPROFILE", home.to_string_lossy());
        }
        for (k, v) in &self.env {
            builder = builder.env(k, v);
        }

        let pty = builder.spawn().context("spawn PtySession")?;
        let frame = Frame::new(self.rows, self.cols);
        Ok(Harness {
            pty,
            frame,
            last_pump: Instant::now(),
        })
    }
}

impl Harness {
    pub fn builder(program: impl Into<PathBuf>) -> HarnessBuilder {
        HarnessBuilder::new(program)
    }

    pub fn send(&mut self, bytes: impl AsRef<[u8]>) -> Result<()> {
        self.pty.write_bytes(bytes.as_ref())
    }

    pub fn paste(&mut self, text: &str) -> Result<()> {
        self.pty.write_bytes(&super::paste::bracketed(text))
    }

    pub fn paste_unbracketed(&mut self, text: &str) -> Result<()> {
        self.pty.write_bytes(&super::paste::unbracketed(text))
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.pty.resize(rows, cols)?;
        self.frame.resize(rows, cols);
        Ok(())
    }

    /// Pull whatever the child has written since last call into the frame
    /// parser. Returns `true` if any new bytes arrived.
    pub fn pump(&mut self) -> bool {
        let bytes = self.pty.drain();
        let any = !bytes.is_empty();
        if any {
            self.frame.feed(&bytes);
            self.last_pump = Instant::now();
        }
        any
    }

    /// Pump output and return the parsed frame. Convenience for asserts.
    pub fn frame(&mut self) -> &Frame {
        self.pump();
        &self.frame
    }

    /// Block (briefly sleeping) until `predicate(frame)` is true or `timeout`
    /// elapses. Pumps the PTY on each tick.
    pub fn wait_for<F>(&mut self, mut predicate: F, timeout: Duration) -> Result<()>
    where
        F: FnMut(&Frame) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            if predicate(&self.frame) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "wait_for timed out after {:?}.\n{}",
                    timeout,
                    self.frame.debug_dump()
                ));
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    /// Wait for the literal substring to appear anywhere on the screen.
    pub fn wait_for_text(&mut self, needle: &str, timeout: Duration) -> Result<()> {
        let owned = needle.to_string();
        self.wait_for(move |f| f.contains(&owned), timeout)
    }

    /// Wait for stable output: no new bytes for `quiet_for` consecutive
    /// pump ticks, bounded by `max`. Useful for "let the UI settle".
    pub fn wait_for_idle(&mut self, quiet_for: Duration, max: Duration) -> Result<()> {
        let max_deadline = Instant::now() + max;
        let mut quiet_since = Instant::now();
        loop {
            if self.pump() {
                quiet_since = Instant::now();
            }
            if quiet_since.elapsed() >= quiet_for {
                return Ok(());
            }
            if Instant::now() >= max_deadline {
                return Err(anyhow!(
                    "wait_for_idle: never settled within {:?}\n{}",
                    max,
                    self.frame.debug_dump()
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Resolve a binary by Cargo bin-name (uses `CARGO_BIN_EXE_<name>`).
    /// Tests should call this rather than hard-coding paths.
    pub fn cargo_bin(name: &str) -> PathBuf {
        // Newer Cargo exposes CARGO_BIN_EXE_* at runtime; older supported
        // Cargo versions expose it to the integration test at compile time.
        let key = format!("CARGO_BIN_EXE_{name}");
        if let Some(path) = std::env::var_os(&key) {
            return PathBuf::from(path);
        }
        if name == "codewhale-tui"
            && let Some(path) = option_env!("CARGO_BIN_EXE_codewhale-tui")
        {
            return PathBuf::from(path);
        }
        // Legacy fallback for callers still referencing the old bin name.
        if name == "codewhale-tui"
            && let Some(path) = option_env!("CARGO_BIN_EXE_deepseek-tui")
        {
            return PathBuf::from(path);
        }
        panic!("env {key} not set; is the binary declared in this crate?")
    }

    /// Best-effort cooperative shutdown.
    pub fn shutdown(self) -> Option<i32> {
        self.pty.shutdown(Duration::from_secs(2))
    }

    pub fn debug_dump(&mut self) -> String {
        self.pump();
        self.frame.debug_dump()
    }
}

/// Construct a sealed-`HOME` workspace under a `tempfile::TempDir` so the
/// scenario can never read or mutate the developer's real config / skills.
pub fn make_sealed_workspace() -> Result<SealedWorkspace> {
    let tmp = tempfile::TempDir::new().context("tempdir")?;
    let workspace = tmp.path().join("workspace");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&workspace).context("mkdir workspace")?;
    std::fs::create_dir_all(home.join(".deepseek")).context("mkdir home/.deepseek")?;
    Ok(SealedWorkspace {
        _tmp: tmp,
        workspace,
        home,
    })
}

pub struct SealedWorkspace {
    _tmp: tempfile::TempDir,
    pub workspace: PathBuf,
    pub home: PathBuf,
}

impl SealedWorkspace {
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
    pub fn home(&self) -> &Path {
        &self.home
    }
    pub fn user_skills_dir(&self) -> PathBuf {
        self.home.join(".deepseek").join("skills")
    }
}
