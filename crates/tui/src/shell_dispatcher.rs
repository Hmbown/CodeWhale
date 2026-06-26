//! Shell abstraction layer for DeepSeek TUI.
//!
//! Detects the user's shell at startup and provides a single entry point for
//! all command execution. DeepSeek TUI never calls `Command::new("cmd")` (or
//! `"sh"`, `"pwsh"`, ...) directly — it asks the [`ShellDispatcher`] to build
//! a correctly configured [`std::process::Command`].
//!
//! ## Responsibilities
//!
//! 1. **Shell detection** — find the user's actual shell (PowerShell, pwsh,
//!    bash via WSL / Git Bash, cmd.exe fallback on Windows, /bin/sh on Unix).
//! 2. **Quoting correctness** — each shell's argument-passing convention is
//!    respected so quoted strings survive the spawn boundary intact.
//! 3. **Terminal state** — foreground shell execution saves and restores
//!    crossterm raw-mode so the TUI input pipeline is not broken after a
//!    child process exits (issue #1690).

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

static LOG_MUTEX: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Shell kind
// ---------------------------------------------------------------------------

/// The concrete shell that the dispatcher will use.
#[allow(dead_code)] // ShellKind enum; wired in ShellDispatcher resolution (see #3490)
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
    /// Any other POSIX shell from $SHELL (zsh, fish, dash, ...).
    Custom { binary: String, flag: String },
}

impl ShellKind {
    /// Binary name for the shell. Appends `.exe` on Windows where needed.
    pub fn binary(&self) -> &str {
        match self {
            #[cfg(windows)]
            ShellKind::Pwsh => "pwsh.exe",
            #[cfg(not(windows))]
            ShellKind::Pwsh => "pwsh",

            #[cfg(windows)]
            ShellKind::WindowsPowerShell => "powershell.exe",
            #[cfg(not(windows))]
            ShellKind::WindowsPowerShell => "powershell",

            #[cfg(windows)]
            ShellKind::Cmd => "cmd.exe",
            #[cfg(not(windows))]
            ShellKind::Cmd => "cmd",

            ShellKind::Sh => "sh",
            ShellKind::Bash => "bash",
            ShellKind::Custom { binary, .. } => binary,
        }
    }

    /// Flag that tells the shell to execute the following argument as a
    /// command string.
    pub fn command_flag(&self) -> &str {
        match self {
            ShellKind::Pwsh | ShellKind::WindowsPowerShell => "-NoProfile",
            ShellKind::Cmd => "/C",
            ShellKind::Sh | ShellKind::Bash => "-c",
            ShellKind::Custom { flag, .. } => flag,
        }
    }

    /// Whether this shell needs an extra `-Command` flag after the profile
    /// flag (PowerShell-specific).
    pub fn needs_command_flag(&self) -> bool {
        matches!(self, ShellKind::Pwsh | ShellKind::WindowsPowerShell)
    }

    #[cfg(test)]
    /// Returns true when this is a PowerShell-family shell.
    pub fn is_powershell(&self) -> bool {
        matches!(self, ShellKind::Pwsh | ShellKind::WindowsPowerShell)
    }
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Central shell abstraction. Created once at startup via
/// [`ShellDispatcher::detect`] and then used everywhere a command needs to
/// be spawned.
#[derive(Debug, Clone)]
pub struct ShellDispatcher {
    kind: ShellKind,
}

#[allow(dead_code)] // ShellDispatcher public impl; wired in config-based shell selection (see #3490)
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
    /// 1. `$SHELL` — if it contains `bash`, use `Bash`; otherwise use the
    ///    actual binary path via `Custom`.
    /// 2. `/bin/sh` fallback.
    pub fn detect() -> Self {
        let kind = Self::detect_shell();
        Self::log_startup(&kind);
        ShellDispatcher { kind }
    }

    /// Log a shell execution line when `SHELL_DISPATCHER_LOG` is set.
    pub fn log_exec(command: &str) {
        if let Ok(path) = std::env::var("SHELL_DISPATCHER_LOG") {
            let _ = Self::append_log_static(&path, command);
        }
    }

    fn log_startup(kind: &ShellKind) {
        let _lock = LOG_MUTEX.lock();
        if let Ok(path) = std::env::var("SHELL_DISPATCHER_LOG") {
            let init_line = format!(
                "--- ShellDispatcher log started pid={} ---\n",
                std::process::id()
            );
            let _ = Self::append_log(&path, &init_line);
            let detect_line = format!("[{}] detect: {kind:?}\n", now_iso());
            let _ = Self::append_log(&path, &detect_line);
        }
    }

    fn append_log(path: &str, line: &str) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(Path::new(path))?;
        file.write_all(line.as_bytes())?;
        file.flush()
    }

    fn append_log_static(path: &str, command: &str) -> std::io::Result<()> {
        // Resolve kind outside the lock — `global_dispatcher()` may trigger
        // `detect()` which calls `log_startup()` which also acquires the mutex.
        let kind = global_dispatcher().kind();
        let _lock = LOG_MUTEX.lock();
        let line = format!("[{}] exec via {kind:?}: {command}\n", now_iso());
        Self::append_log(path, &line)
    }

    /// The detected shell kind.
    pub fn kind(&self) -> &ShellKind {
        &self.kind
    }

    // -- Public builders --------------------------------------------------

    /// Build a `std::process::Command` for the given shell command string.
    pub fn build_command(&self, shell_command: &str) -> Command {
        let mut cmd = Command::new(self.kind.binary());

        if self.kind.needs_command_flag() {
            cmd.arg(self.kind.command_flag());
            cmd.arg("-Command");
            cmd.arg(shell_command);
        } else if matches!(self.kind, ShellKind::Cmd) {
            cmd.arg(self.kind.command_flag());
            #[cfg(windows)]
            {
                cmd.raw_arg(shell_command);
            }
            #[cfg(not(windows))]
            {
                cmd.arg(shell_command);
            }
        } else {
            cmd.arg(self.kind.command_flag());
            cmd.arg(shell_command);
        }

        cmd
    }

    /// Build the program + args tuple. Useful when the caller needs to
    /// inspect or modify the args before passing them to `Command`.
    pub fn build_command_parts(&self, shell_command: &str) -> (String, Vec<String>) {
        let program = self.kind.binary().to_string();
        let args = if self.kind.needs_command_flag() {
            vec![
                self.kind.command_flag().to_string(),
                "-Command".to_string(),
                shell_command.to_string(),
            ]
        } else {
            vec![
                self.kind.command_flag().to_string(),
                shell_command.to_string(),
            ]
        };
        (program, args)
    }

    /// Build a `Command` from separate program + args (bypasses the shell).
    /// Used when the caller already has a resolved executable and argument
    /// vector — e.g. `ExecEnv` from the sandbox.
    #[cfg(test)]
    pub fn build_direct(&self, program: &str, args: &[String]) -> Command {
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd
    }

    /// Execute a foreground command with raw-mode save/restore.
    ///
    /// A scope guard ensures raw mode is restored even if the command fails
    /// to spawn or returns early (review feedback, issue #1690).
    pub fn run_foreground(
        &self,
        shell_command: &str,
        cwd: &std::path::Path,
    ) -> Result<String, anyhow::Error> {
        use anyhow::Context;

        // Log the execution
        {
            let _lock = LOG_MUTEX.lock();
            if let Ok(path) = std::env::var("SHELL_DISPATCHER_LOG") {
                let kind = self.kind();
                let line = format!("[{}] exec via {kind:?}: {shell_command}\n", now_iso());
                let _ = Self::append_log(&path, &line);
            }
        }

        // Disable raw mode; guard restores it only if it was already enabled.
        let raw_mode_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if raw_mode_was_enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }
        struct FgRawModeGuard {
            restore: bool,
        }
        impl Drop for FgRawModeGuard {
            fn drop(&mut self) {
                if self.restore {
                    let _ = crossterm::terminal::enable_raw_mode();
                }
            }
        }
        let _guard = FgRawModeGuard {
            restore: raw_mode_was_enabled,
        };

        let mut cmd = self.build_command(shell_command);
        cmd.current_dir(cwd);

        let output = cmd
            .output()
            .with_context(|| format!("failed to execute shell command: {shell_command}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "shell command failed (status={}): {}",
                output.status,
                stderr.trim()
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(stdout)
    }

    // -- Detection --------------------------------------------------------

    fn detect_shell() -> ShellKind {
        #[cfg(windows)]
        {
            // 1. $env:SHELL — WSL interop or Git Bash often set this.
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
            }

            if Self::find_exe("pwsh.exe") {
                return ShellKind::Pwsh;
            }
            if Self::find_exe("powershell.exe") {
                return ShellKind::WindowsPowerShell;
            }
            ShellKind::Cmd
        }

        #[cfg(not(windows))]
        {
            // 1. $SHELL environment variable (Unix)
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
                return ShellKind::Custom {
                    binary: shell,
                    flag: "-c".to_string(),
                };
            }

            ShellKind::Sh
        }
    }

    /// Check PATH first, then fall back to well-known install directories.
    #[cfg(windows)]
    fn find_exe(name: &str) -> bool {
        if Self::binary_on_path(name) {
            return true;
        }
        // Well-known install locations (order by preference).
        let known_dirs: &[&str] = &[
            r"C:\Program Files\PowerShell\7",
            r"C:\Windows\System32\WindowsPowerShell\v1.0",
        ];
        known_dirs
            .iter()
            .any(|dir| std::path::Path::new(dir).join(name).is_file())
    }

    #[cfg(windows)]
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

// -- Helpers ---------------------------------------------------------------

fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3f")
        .to_string()
}

/// Global dispatcher instance, detected once at startup.
///
/// Any code path that needs to spawn a shell command can use
/// `global_dispatcher()` instead of threading the dispatcher through
/// every function signature.
pub fn global_dispatcher() -> &'static ShellDispatcher {
    use std::sync::LazyLock;
    static DISPATCHER: LazyLock<ShellDispatcher> = LazyLock::new(ShellDispatcher::detect);
    &DISPATCHER
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_kind_binary_names() {
        #[cfg(windows)]
        {
            assert_eq!(ShellKind::Pwsh.binary(), "pwsh.exe");
            assert_eq!(ShellKind::WindowsPowerShell.binary(), "powershell.exe");
            assert_eq!(ShellKind::Cmd.binary(), "cmd.exe");
        }
        #[cfg(not(windows))]
        {
            assert_eq!(ShellKind::Pwsh.binary(), "pwsh");
            assert_eq!(ShellKind::WindowsPowerShell.binary(), "powershell");
            assert_eq!(ShellKind::Cmd.binary(), "cmd");
        }
        assert_eq!(ShellKind::Sh.binary(), "sh");
        assert_eq!(ShellKind::Bash.binary(), "bash");
    }

    #[test]
    fn detect_returns_some_shell() {
        let dispatcher = global_dispatcher();
        let _kind = dispatcher.kind();
    }

    #[test]
    fn powershell_build_command_includes_no_profile_and_command_flags() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Pwsh,
        };
        let cmd = dispatcher.build_command("echo hello");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.contains(&"-NoProfile"));
        assert!(args.contains(&"-Command"));
        assert!(args.contains(&"echo hello"));
    }

    #[test]
    fn cmd_build_command_uses_c_flag() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Cmd,
        };
        let cmd = dispatcher.build_command("echo hello");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.contains(&"/C"));
        assert!(args.contains(&"echo hello"));
    }

    #[test]
    fn sh_build_command_uses_dash_c() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Sh,
        };
        let cmd = dispatcher.build_command("echo hello");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.contains(&"-c"));
        assert!(args.contains(&"echo hello"));
    }

    #[cfg(test)]
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

    #[cfg(test)]
    #[test]
    fn powershell_flags_are_correct() {
        assert!(ShellKind::Pwsh.needs_command_flag());
        assert!(ShellKind::WindowsPowerShell.needs_command_flag());
        assert!(!ShellKind::Cmd.needs_command_flag());
        assert!(!ShellKind::Sh.needs_command_flag());
        assert!(!ShellKind::Bash.needs_command_flag());
    }

    #[cfg(test)]
    #[test]
    fn is_powershell_detects_both_variants() {
        assert!(ShellKind::Pwsh.is_powershell());
        assert!(ShellKind::WindowsPowerShell.is_powershell());
        assert!(!ShellKind::Cmd.is_powershell());
        assert!(!ShellKind::Sh.is_powershell());
        assert!(!ShellKind::Bash.is_powershell());
    }

    #[cfg(test)]
    #[test]
    fn build_command_quotes_spaces_for_cmd() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Cmd,
        };
        let cmd = dispatcher.build_command("git commit -m \"msg with spaces\"");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "/C");
        assert!(args[1].contains("msg with spaces"));
        assert!(args[1].starts_with("git "));
    }

    #[cfg(test)]
    #[test]
    fn build_command_quotes_spaces_for_pwsh() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Pwsh,
        };
        let cmd = dispatcher.build_command("git commit -m \"msg with spaces\"");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "-NoProfile");
        assert_eq!(args[1], "-Command");
        assert!(args[2].contains("msg with spaces"));
    }

    #[cfg(test)]
    #[test]
    fn build_direct_handles_empty_args() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Sh,
        };
        let cmd = dispatcher.build_direct("echo", &[]);
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn find_exe_finds_cmd_on_path() {
        // cmd.exe is always on PATH on Windows.
        assert!(ShellDispatcher::find_exe("cmd.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn find_exe_rejects_nonexistent_binary() {
        assert!(!ShellDispatcher::find_exe("nonexistent_xyz_12345.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn find_exe_falls_back_to_known_dirs() {
        // Verify the known-dirs fallback path actually exists on this system.
        let ps_path = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
        if std::path::Path::new(ps_path).is_file() {
            // The fallback directory exists — find_exe should locate it.
            assert!(ShellDispatcher::find_exe("powershell.exe"));
        } else {
            eprintln!("Skipping: {ps_path} not present on this system");
        }
    }

    #[test]
    fn custom_shell_uses_provided_binary_and_flag() {
        let kind = ShellKind::Custom {
            binary: "/bin/zsh".to_string(),
            flag: "-c".to_string(),
        };
        assert_eq!(kind.binary(), "/bin/zsh");
        assert_eq!(kind.command_flag(), "-c");
    }
}
