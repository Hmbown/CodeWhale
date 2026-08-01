//! End-to-end harness composing [`PtySession`] + [`Frame`].
//!
//! Tests build a [`Harness`] via [`Harness::builder`], drive the TUI with
//! [`Harness::send`] / [`Harness::paste`], poll the parsed terminal state
//! with [`Harness::wait_for`], and assert on [`Harness::frame`] /
//! filesystem state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

use super::{Frame, PtySession};

/// Scale a wait budget for shared CI runners.
///
/// PTY scenarios boot a real binary and wait on real terminal output, and the
/// budgets in the scenarios are tuned for a developer laptop running one test
/// at a time. CI runs the whole workspace suite on a shared runner, where the
/// same output can legitimately arrive several times later. Every budget this
/// scales is a deadline on a poll that returns as soon as the condition holds,
/// so a larger budget never slows a passing run — it only changes how long a
/// genuinely stuck scenario waits before failing. Local runs keep the tight
/// value so a real hang still surfaces quickly while developing.
pub fn ci_scaled(base: Duration) -> Duration {
    if std::env::var_os("CI").is_some() {
        base * 4
    } else {
        base
    }
}

pub struct Harness {
    pty: PtySession,
    frame: Frame,
    last_pump: Instant,
    cursor_query_tail: Vec<u8>,
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

    /// Point `$HOME` (and config/cache defaults) at a fresh dir so the spawned
    /// binary cannot read or mutate the developer's real user config.
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
            let codewhale_config = home.join(".codewhale").join("config.toml");
            let deepseek_config = home.join(".deepseek").join("config.toml");
            builder = builder
                .env("HOME", home.to_string_lossy())
                .env("XDG_CONFIG_HOME", home.join(".config").to_string_lossy())
                .env("XDG_DATA_HOME", home.join(".local/share").to_string_lossy())
                .env("XDG_CACHE_HOME", home.join(".cache").to_string_lossy())
                .env("USERPROFILE", home.to_string_lossy())
                .env("CODEWHALE_CONFIG_PATH", codewhale_config.to_string_lossy())
                .env("DEEPSEEK_CONFIG_PATH", deepseek_config.to_string_lossy());
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
            cursor_query_tail: Vec::new(),
        })
    }
}

impl Harness {
    pub fn builder(program: impl Into<PathBuf>) -> HarnessBuilder {
        HarnessBuilder::new(program)
    }

    pub fn pid(&self) -> Option<u32> {
        self.pty.pid()
    }

    pub fn send(&mut self, bytes: impl AsRef<[u8]>) -> Result<()> {
        self.pty.write_bytes(bytes.as_ref())
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.pty.resize(rows, cols)?;
        self.frame.resize(rows, cols);
        Ok(())
    }

    pub fn paste(&mut self, text: &str) -> Result<()> {
        self.pty.write_bytes(&super::paste::bracketed(text))
    }

    pub fn paste_unbracketed(&mut self, text: &str) -> Result<()> {
        self.pty.write_bytes(&super::paste::unbracketed(text))
    }

    /// Pull whatever the child has written since last call into the frame
    /// parser. Returns `true` if any new bytes arrived.
    pub fn pump(&mut self) -> bool {
        let bytes = self.pty.drain();
        let any = !bytes.is_empty();
        if any {
            let cursor_queries =
                consume_cursor_position_queries(&mut self.cursor_query_tail, &bytes);
            self.frame.feed(&bytes);
            if cursor_queries > 0 {
                let (row, col) = self.frame.cursor();
                let response = format!("\x1b[{};{}R", row.saturating_add(1), col.saturating_add(1));
                for _ in 0..cursor_queries {
                    if self.pty.write_bytes(response.as_bytes()).is_err() {
                        break;
                    }
                }
            }
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
        let budget = ci_scaled(timeout);
        let deadline = Instant::now() + budget;
        loop {
            self.pump();
            if predicate(&self.frame) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "wait_for timed out after {:?}.\n{}",
                    budget,
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
        // Only the ceiling scales: `quiet_for` is the definition of "settled",
        // not a budget, and stretching it would change what the test asserts.
        let budget = ci_scaled(max);
        let max_deadline = Instant::now() + budget;
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
                    budget,
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
        panic!("env {key} not set; is the binary declared in this crate?")
    }

    /// Best-effort cooperative shutdown.
    pub fn shutdown(self) -> Option<i32> {
        self.pty.shutdown(Duration::from_secs(2))
    }

    /// Wait for the child process to exit without sending it a signal.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> Option<i32> {
        self.pty.wait_until(Instant::now() + ci_scaled(timeout))
    }

    pub fn debug_dump(&mut self) -> String {
        self.pump();
        self.frame.debug_dump()
    }

    /// Every byte the child has written, from spawn to now. Survives `pump`,
    /// so terminal-mode assertions stay valid after the frame parser has
    /// consumed the stream.
    pub fn transcript(&self) -> Vec<u8> {
        self.pty.transcript()
    }

    /// Replay the transcript into a [`TerminalModeLedger`].
    pub fn terminal_modes(&self) -> super::TerminalModeLedger {
        super::TerminalModeLedger::from_transcript(&self.transcript())
    }

    /// Frame dump plus terminal-mode ledger. Every bounded wait in the matrix
    /// fails with this rather than a bare `assertion failed`, so a CI timeout
    /// carries the screen *and* the control-stream state that produced it.
    pub fn diagnostics(&mut self) -> String {
        let modes = self.terminal_modes().debug_dump();
        format!("{}{modes}", self.debug_dump())
    }
}

const CURSOR_POSITION_QUERIES: [&[u8]; 2] = [b"\x1b[6n", b"\x1b[?6n"];

/// Consume terminal cursor-position queries from a chunked PTY output stream.
///
/// Crossterm asks the terminal for its cursor after Ratatui clears the screen.
/// A real terminal answers that DSR request; the QA PTY must do the same or the
/// child waits for crossterm's timeout before it can paint its first frame.
fn consume_cursor_position_queries(tail: &mut Vec<u8>, bytes: &[u8]) -> usize {
    let mut stream = std::mem::take(tail);
    stream.extend_from_slice(bytes);

    let mut count = 0;
    let mut index = 0;
    while index < stream.len() {
        if let Some(query) = CURSOR_POSITION_QUERIES
            .iter()
            .find(|query| stream[index..].starts_with(query))
        {
            count += 1;
            index += query.len();
        } else {
            index += 1;
        }
    }

    let max_tail = CURSOR_POSITION_QUERIES
        .iter()
        .map(|query| query.len().saturating_sub(1))
        .max()
        .unwrap_or(0)
        .min(stream.len());
    let keep = (1..=max_tail)
        .rev()
        .find(|&len| {
            CURSOR_POSITION_QUERIES
                .iter()
                .any(|query| len < query.len() && query.starts_with(&stream[stream.len() - len..]))
        })
        .unwrap_or(0);
    tail.extend_from_slice(&stream[stream.len() - keep..]);
    count
}

/// Construct a sealed-`HOME` workspace under a `tempfile::TempDir` so the
/// scenario can never read or mutate the developer's real config / skills.
pub fn make_sealed_workspace() -> Result<SealedWorkspace> {
    let tmp = tempfile::TempDir::new().context("tempdir")?;
    let workspace = tmp.path().join("workspace");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&workspace).context("mkdir workspace")?;
    std::fs::create_dir_all(home.join(".codewhale")).context("mkdir home/.codewhale")?;
    std::fs::create_dir_all(home.join(".deepseek")).context("mkdir home/.deepseek")?;
    let silent_notifications = "[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n";
    std::fs::write(
        home.join(".codewhale").join("config.toml"),
        silent_notifications,
    )
    .context("write silent CodeWhale PTY config")?;
    std::fs::write(
        home.join(".deepseek").join("config.toml"),
        silent_notifications,
    )
    .context("write silent legacy PTY config")?;
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

#[cfg(test)]
mod tests {
    use super::consume_cursor_position_queries;

    #[test]
    fn cursor_position_queries_survive_chunk_boundaries() {
        let mut tail = Vec::new();
        assert_eq!(
            consume_cursor_position_queries(&mut tail, b"before\x1b["),
            0
        );
        assert_eq!(consume_cursor_position_queries(&mut tail, b"6nafter"), 1);
        assert!(tail.is_empty());
    }

    #[test]
    fn cursor_position_queries_accept_standard_and_dec_forms() {
        let mut tail = Vec::new();
        assert_eq!(
            consume_cursor_position_queries(&mut tail, b"\x1b[6n\x1b[?6n"),
            2
        );
        assert!(tail.is_empty());
    }
}
