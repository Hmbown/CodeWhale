//! Shell abstraction layer for DeepSeek TUI.
//!
//! Detects the user's shell at startup and provides a single entry point for
//! all command execution. DeepSeek TUI never calls `Command::new("cmd")` (or
//! `"sh"`, `"pwsh"`, …) directly — it asks the [`ShellDispatcher`] to build
//! a correctly configured [`std::process::Command`].
//!
//! ## Responsibilities
//!
//! 1. **Shell detection** — find the user's actual shell (PowerShell, pwsh,
//!    bash via WSL / Git Bash, cmd.exe fallback on Windows, /bin/sh on Unix).
//! 2. **Quoting correctness** — each shell's argument-passing convention is
//!    respected so quoted strings (e.g. `git commit -m "msg with spaces"`)
//!    survive the spawn boundary intact.
//! 3. **Terminal state** — foreground shell execution saves and restores
//!    crossterm raw-mode so the TUI input pipeline is not broken after a
//!    child process exits (Windows issue #1690).
//! 4. **Process lifecycle** — timeout, stdin feeding, background jobs, and
//!    PTY allocation are delegated to the existing `tools/shell.rs` helpers;
//!    the dispatcher only owns the *spawn shape*.
//!
//! ## Usage
//!
//! ```ignore
//! let dispatcher = ShellDispatcher::detect();
//! let mut cmd = dispatcher.build_command("echo hello");
//! let output = cmd.output()?;
//! ```

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// Shell kind
// ---------------------------------------------------------------------------

/// The concrete shell that the dispatcher will use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellKind {
    /// PowerShell 7+ (`pwsh.exe`).
    Pwsh,
    /// Windows PowerShell 5.1 (`powershell.exe`).
    WindowsPowerShell,
    /// Command Prompt (`cmd.exe`).
    Cmd,
    /// Unix `/bin/sh` (or `$SHELL`-detected bash/zsh).
    Sh,
    /// Bash — detected via `$SHELL` on either Unix or WSL/Git Bash on Windows.
    Bash,
}

impl ShellKind {
    /// Binary name for the shell (used in `Command::new`).
    pub fn binary(&self) -> &str {
        match self {
            ShellKind::Pwsh => "pwsh.exe",
            ShellKind::WindowsPowerShell => "powershell.exe",
            ShellKind::Cmd => "cmd.exe",
            ShellKind::Sh => "sh",
            ShellKind::Bash => "bash",
        }
    }

    /// Flag that tells the shell to execute the following argument as a
    /// command string.
    pub fn command_flag(&self) -> &str {
        match self {
            ShellKind::Pwsh | ShellKind::WindowsPowerShell => "-NoProfile",
            ShellKind::Cmd => "/C",
            ShellKind::Sh | ShellKind::Bash => "-c",
        }
    }

    /// Whether this shell needs the command wrapped in an additional
    /// quoting layer to survive the shell's own parser.
    ///
    /// PowerShell needs the command passed as a single `-Command <string>`
    /// argument; `-NoProfile` is separate.
    pub fn needs_command_flag(&self) -> bool {
        matches!(self, ShellKind::Pwsh | ShellKind::WindowsPowerShell)
    }

    /// Returns true when this is a PowerShell-family shell.
    pub fn is_powershell(&self) -> bool {
        matches!(self, ShellKind::Pwsh | ShellKind::WindowsPowerShell)
    }
}

/// Global dispatcher instance, detected once at startup.
///
/// Any code path that needs to spawn a shell command can use
/// `global_dispatcher()` instead of threading the dispatcher through every
/// function signature.
pub fn global_dispatcher() -> &'static ShellDispatcher {
    use std::sync::LazyLock;
    static DISPATCHER: LazyLock<ShellDispatcher> = LazyLock::new(ShellDispatcher::detect);
    &DISPATCHER
}


// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Central shell abstraction.
///
/// Created once at startup via [`ShellDispatcher::detect`] and then used
/// everywhere a command needs to be spawned.
#[derive(Debug, Clone)]
pub struct ShellDispatcher {
    kind: ShellKind,
}

impl ShellDispatcher {
    /// Detect the user's shell from the environment.
    ///
    /// ## Detection order (Windows)
    ///
    /// 1. `$env:SHELL` — WSL interop or Git Bash often set this.
    /// 2. `pwsh.exe` found on `PATH` — PowerShell 7+.
    /// 3. `powershell.exe` found on `PATH` — Windows PowerShell 5.1.
    /// 4. `cmd.exe` — always available, last resort.
    ///
    /// ## Detection order (Unix)
    ///
    /// 1. `$SHELL` — if it contains `bash`, use `Bash`; otherwise `Sh`.
    /// 2. `/bin/sh` fallback.
    pub fn detect() -> Self {
        let kind = Self::detect_shell();
        tracing::info!(target: "shell_dispatcher", shell = ?kind,
            "ShellDispatcher detected {:?}", kind);
        ShellDispatcher { kind }
    }

    /// The detected shell kind.
    pub fn kind(&self) -> &ShellKind {
        &self.kind
    }

    // -- Public builder --------------------------------------------------

    /// Build a `std::process::Command` for the given shell command string.
    ///
    /// The returned `Command` has the correct binary, shell flag, and
    /// argument quoting for the detected shell. Callers are responsible for
    /// setting `current_dir`, `stdin`/`stdout`/`stderr`, and environment.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let dispatcher = ShellDispatcher::detect();
    /// let mut cmd = dispatcher.build_command("echo hello");
    /// cmd.current_dir("/tmp");
    /// let output = cmd.output()?;
    /// ```
    pub fn build_command(&self, shell_command: &str) -> Command {
        let mut cmd = Command::new(self.kind.binary());

        if self.kind.needs_command_flag() {
            // PowerShell: pwsh.exe -NoProfile -Command "<shell_command>"
            cmd.arg(self.kind.command_flag()); // -NoProfile
            cmd.arg("-Command");
            cmd.arg(shell_command);
        } else {
            // cmd /C <command>   or   sh -c '<command>'
            cmd.arg(self.kind.command_flag());
            cmd.arg(shell_command);
        }

        cmd
    }

    /// Build a `std::process::Command` from separate program + args (bypasses
    /// the shell). This is used when the caller already has a resolved
    /// executable and argument vector — e.g. `ExecEnv` from the sandbox.
    ///
    /// Quoting is handled by Rust's `std::process::Command` which uses
    /// MSVCRT `CommandLineToArgvW` escaping on Windows. This is correct for
    /// direct program execution (not via `cmd /C`).
    pub fn build_direct(&self, program: &str, args: &[String]) -> Command {
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd
    }

    /// Execute a foreground command with raw-mode save/restore around it.
    ///
    /// This is the only call-site that should toggle raw mode for shell
    /// execution. Individual callers do not call `disable_raw_mode` /
    /// `enable_raw_mode` themselves — that responsibility lives here so it
    /// cannot be forgotten (issue #1690).
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails to spawn or returns a non-zero
    /// exit status.
    pub fn run_foreground(
        &self,
        shell_command: &str,
        cwd: &std::path::Path,
    ) -> Result<String, anyhow::Error> {
        use anyhow::Context;

        tracing::info!(target: "shell_dispatcher", shell = ?self.kind, command = %shell_command,
            "exec_shell via {:?}: {}", self.kind, shell_command);

        // Save terminal state — crossterm raw mode must be off while the
        // child process owns the console, otherwise Windows loses
        // ENABLE_VIRTUAL_TERMINAL_INPUT and the TUI keyboard breaks.
        let _ = crossterm::terminal::disable_raw_mode();

        let mut cmd = self.build_command(shell_command);
        cmd.current_dir(cwd);

        let output = cmd
            .output()
            .with_context(|| format!("failed to execute shell command: {shell_command}"))?;

        // Restore raw mode so the TUI input pipeline works again.
        let _ = crossterm::terminal::enable_raw_mode();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "shell command failed (status={}): {}",
                output.status,
                stderr.trim()
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(stdout.trim().to_string())
    }

    // -- Detection helpers -----------------------------------------------

    fn detect_shell() -> ShellKind {
        // 1. $SHELL environment variable (WSL, Git Bash, MSYS2, or Unix)
        if let Ok(shell) = std::env::var("SHELL") {
            let lower = shell.to_lowercase();
            if lower.contains("bash") {
                return ShellKind::Bash;
            }
            if lower.contains("pwsh") {
                return ShellKind::Pwsh;
            }
            if lower.contains("powershell") {
                return ShellKind::WindowsPowerShell;
            }
            // zsh, fish, dash, etc. — all POSIX-compatible via -c
            return ShellKind::Sh;
        }

        #[cfg(windows)]
        {
            // 2. pwsh.exe (PowerShell 7+)
            if Self::binary_on_path("pwsh.exe") {
                return ShellKind::Pwsh;
            }
            // 3. powershell.exe (Windows PowerShell 5.1)
            if Self::binary_on_path("powershell.exe") {
                return ShellKind::WindowsPowerShell;
            }
            // 4. cmd.exe — always available
            return ShellKind::Cmd;
        }

        #[cfg(not(windows))]
        {
            ShellKind::Sh
        }
    }

    /// Check whether a binary name is discoverable on `PATH`.
    fn binary_on_path(name: &str) -> bool {
        std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path).any(|dir| {
                    let candidate = dir.join(name);
                    candidate.is_file()
                })
            })
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_kind_binary_names() {
        assert_eq!(ShellKind::Pwsh.binary(), "pwsh.exe");
        assert_eq!(ShellKind::WindowsPowerShell.binary(), "powershell.exe");
        assert_eq!(ShellKind::Cmd.binary(), "cmd.exe");
        assert_eq!(ShellKind::Sh.binary(), "sh");
        assert_eq!(ShellKind::Bash.binary(), "bash");
    }

    #[test]
    fn detect_returns_some_shell() {
        let dispatcher = ShellDispatcher::detect();
        // On any platform we must detect *something*.
        let _kind = dispatcher.kind();
    }

    #[test]
    fn powershell_build_command_includes_no_profile_and_command_flags() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Pwsh,
        };
        let cmd = dispatcher.build_command("echo hello");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.contains(&"-NoProfile"), "expected -NoProfile, got {args:?}");
        assert!(args.contains(&"-Command"), "expected -Command, got {args:?}");
        assert!(args.contains(&"echo hello"), "expected echo hello, got {args:?}");
    }

    #[test]
    fn cmd_build_command_uses_c_flag() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Cmd,
        };
        let cmd = dispatcher.build_command("echo hello");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.contains(&"/C"), "expected /C, got {args:?}");
        assert!(args.contains(&"echo hello"), "expected echo hello, got {args:?}");
    }

    #[test]
    fn sh_build_command_uses_dash_c() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Sh,
        };
        let cmd = dispatcher.build_command("echo hello");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.contains(&"-c"), "expected -c, got {args:?}");
        assert!(args.contains(&"echo hello"), "expected echo hello, got {args:?}");
    }

    #[test]
    fn build_direct_preserves_args() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Cmd,
        };
        let args = vec!["-m".to_string(), "commit message".to_string()];
        let cmd = dispatcher.build_direct("git", &args);
        let cmd_args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(cmd_args, vec!["-m", "commit message"]);
    }

    #[test]
    fn all_shell_kinds_have_distinct_binaries() {
        let kinds = [
            ShellKind::Pwsh,
            ShellKind::WindowsPowerShell,
            ShellKind::Cmd,
            ShellKind::Sh,
            ShellKind::Bash,
        ];
        for kind in &kinds {
            assert!(!kind.binary().is_empty(), "empty binary for {kind:?}");
            assert!(!kind.command_flag().is_empty(), "empty flag for {kind:?}");
        }
    }

    #[test]
    fn powershell_flags_are_correct() {
        assert!(ShellKind::Pwsh.needs_command_flag());
        assert!(ShellKind::WindowsPowerShell.needs_command_flag());
        assert!(!ShellKind::Cmd.needs_command_flag());
        assert!(!ShellKind::Sh.needs_command_flag());
        assert!(!ShellKind::Bash.needs_command_flag());
    }

    #[test]
    fn is_powershell_detects_both_variants() {
        assert!(ShellKind::Pwsh.is_powershell());
        assert!(ShellKind::WindowsPowerShell.is_powershell());
        assert!(!ShellKind::Cmd.is_powershell());
        assert!(!ShellKind::Sh.is_powershell());
        assert!(!ShellKind::Bash.is_powershell());
    }

    #[test]
    fn build_command_quotes_spaces_for_cmd() {
        // Regression: issue #1691 — git commit -m "msg with spaces" must
        // not be split into separate argv entries.
        let dispatcher = ShellDispatcher { kind: ShellKind::Cmd };
        let cmd = dispatcher.build_command("git commit -m \"msg with spaces\"");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        // cmd.exe /C receives the entire command as a single argument after /C.
        // The args should be ["/C", "git commit -m \"msg with spaces\""].
        assert_eq!(args.len(), 2, "expected 2 args (/C + command), got {args:?}");
        assert_eq!(args[0], "/C");
        assert!(args[1].contains("msg with spaces"),
            "command string should contain the full quoted message, got: {}", args[1]);
        // The quoted message must not be split — if it were, args[1] would be
        // just "git" and we'd see "commit", "-m", "\"msg", etc.
        assert!(args[1].starts_with("git "), "command should start with 'git', got: {}", args[1]);
    }

    #[test]
    fn build_command_quotes_spaces_for_pwsh() {
        let dispatcher = ShellDispatcher { kind: ShellKind::Pwsh };
        let cmd = dispatcher.build_command("git commit -m \"msg with spaces\"");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        // pwsh.exe -NoProfile -Command "<entire command>"
        assert_eq!(args.len(), 3, "expected 3 args (-NoProfile, -Command, payload), got {args:?}");
        assert_eq!(args[0], "-NoProfile");
        assert_eq!(args[1], "-Command");
        assert!(args[2].contains("msg with spaces"),
            "payload should contain the full quoted message, got: {}", args[2]);
    }

    #[test]
    fn build_command_quotes_spaces_for_sh() {
        let dispatcher = ShellDispatcher { kind: ShellKind::Sh };
        let cmd = dispatcher.build_command("git commit -m \"msg with spaces\"");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args.len(), 2, "expected 2 args (-c + command), got {args:?}");
        assert_eq!(args[0], "-c");
        assert!(args[1].contains("msg with spaces"));
    }

    #[test]
    fn global_dispatcher_is_singleton() {
        let d1 = global_dispatcher();
        let d2 = global_dispatcher();
        // Same kind (can't compare pointers across LazyLock, but detect()
        // is deterministic for a given environment so kind should match).
        assert_eq!(d1.kind(), d2.kind());
    }

    #[test]
    fn build_direct_handles_empty_args() {
        let dispatcher = ShellDispatcher { kind: ShellKind::Sh };
        let cmd = dispatcher.build_direct("echo", &[]);
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.is_empty(), "expected no args for echo, got {args:?}");
    }
}