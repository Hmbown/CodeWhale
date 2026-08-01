//! Shell abstraction layer for Codewhale.
//!
//! Detects the user's shell at startup and provides a single entry point for
//! all command execution. Codewhale never calls `Command::new("cmd")` (or
//! `"sh"`, `"pwsh"`, ...) directly — it asks the [`ShellDispatcher`] to build
//! a correctly configured [`std::process::Command`].
//!
//! ## Responsibilities
//!
//! 1. **Shell detection** — find the user's actual shell (PowerShell, pwsh,
//!    bash via WSL / Git Bash, cmd.exe fallback on Windows, /bin/sh on Unix).
//!    On Windows, prefer PowerShell 7 (`pwsh`) over Windows PowerShell 5.1.
//! 2. **Quoting correctness** — each shell's argument-passing convention is
//!    respected so quoted strings survive the spawn boundary intact.
//! 3. **PowerShell safety** — non-interactive flags, temporary `.ps1` files
//!    for multiline scripts, and explicit native `$LASTEXITCODE` capture.
//! 4. **Terminal state** — foreground shell execution saves and restores
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

#[cfg(test)]
#[allow(dead_code)] // Direct integration-harness inclusion only needs the read barrier.
#[path = "test_env_lock.rs"]
pub(crate) mod test_env_lock;

// ---------------------------------------------------------------------------
// Shell kind
// ---------------------------------------------------------------------------

/// The concrete shell that the dispatcher will use.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellKind {
    /// PowerShell 7+ (`pwsh.exe`).
    Pwsh,
    /// Windows PowerShell 5.1 (`powershell.exe`).
    WindowsPowerShell,
    /// Command Prompt (`cmd.exe`).
    Cmd,
    /// Unix `/bin/sh` fallback.
    Sh,
    /// Bash — detected via `$SHELL` on WSL/Git Bash, or constructed explicitly.
    Bash,
    /// The exact shell executable selected by Unix `$SHELL`.
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

            #[cfg(windows)]
            ShellKind::Sh => "sh",
            #[cfg(not(windows))]
            ShellKind::Sh => "/bin/sh",
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
    /// flag (PowerShell-specific). Only exercised by shell-flag unit tests.
    #[cfg(test)]
    pub fn needs_command_flag(&self) -> bool {
        matches!(self, ShellKind::Pwsh | ShellKind::WindowsPowerShell)
    }

    /// Returns true when this is a PowerShell-family shell.
    pub fn is_powershell(&self) -> bool {
        match self {
            ShellKind::Pwsh | ShellKind::WindowsPowerShell => true,
            ShellKind::Custom { binary, .. } => Path::new(binary)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    let name = name.to_ascii_lowercase();
                    name.contains("pwsh") || name.contains("powershell")
                }),
            ShellKind::Cmd | ShellKind::Sh | ShellKind::Bash => false,
        }
    }
}

/// Multiline, nested-quote, or non-ASCII PowerShell scripts are safer as a
/// temporary `-File` script than as a single `-Command` string.
fn powershell_prefers_script_file(shell_command: &str) -> bool {
    shell_command.contains('\n')
        || shell_command.contains('\r')
        || !shell_command.is_ascii()
        || shell_command.matches('"').count() >= 4
        || shell_command.contains("'''")
        || shell_command.contains("@'")
        || shell_command.contains("@\"")
}

/// Wrap a model/user PowerShell command so native program failures surface
/// through `$LASTEXITCODE` without using `Invoke-Expression`.
fn powershell_exit_aware_command(shell_command: &str) -> String {
    // Keep simple expressions as-is; only wrap when the payload looks like it
    // may invoke a native executable (contains a path or known separators).
    if shell_command.trim().is_empty() {
        return shell_command.to_string();
    }
    // The exit-code check goes on its own line: a trailing unquoted `#`
    // comment in the payload would otherwise swallow a `;`-joined check to
    // end-of-line and silently report success for failing native commands.
    // `-Command` accepts embedded newlines inside one argv string.
    format!(
        "$ErrorActionPreference = 'Continue'; {shell_command}\nif ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}"
    )
}

/// Tail appended to every temp `-File` script: capture the native exit code,
/// remove the script itself (PowerShell reads the whole file before running,
/// so self-deletion is safe), then propagate the exit code.
const TEMP_PS1_TAIL: &str = concat!(
    "$__codewhaleExit = if ($null -ne $LASTEXITCODE) { $LASTEXITCODE } else { 0 }\n",
    "Remove-Item -LiteralPath $MyInvocation.MyCommand.Path -Force ",
    "-ErrorAction SilentlyContinue\n",
    "if ($__codewhaleExit -ne 0) { exit $__codewhaleExit }\n",
);

fn write_temp_ps1(shell_command: &str) -> std::io::Result<String> {
    use std::io::Write;
    let dir = std::env::temp_dir();
    sweep_stale_temp_ps1(&dir);
    let name = format!(
        "codewhale-shell-{}-{}.ps1",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path)?;
    // UTF-8 with BOM helps Windows PowerShell 5.1 decode non-ASCII scripts.
    file.write_all(&[0xEF, 0xBB, 0xBF])?;
    file.write_all(shell_command.as_bytes())?;
    if !shell_command.ends_with('\n') {
        file.write_all(b"\n")?;
    }
    // Native exit-code propagation plus self-cleanup for the script form.
    file.write_all(TEMP_PS1_TAIL.as_bytes())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Best-effort removal of leftover `codewhale-shell-*.ps1` scripts (for
/// example after a killed process, which skips the in-script self-delete).
/// Only files older than one hour are touched so a concurrently starting
/// invocation is never raced.
fn sweep_stale_temp_ps1(dir: &std::path::Path) {
    const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60 * 60);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with("codewhale-shell-") || !name.ends_with(".ps1") {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_AFTER);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
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

#[allow(dead_code)]
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
    /// 1. `$SHELL` — preserve its actual executable via `Custom`; bare names
    ///    are resolved against the current `PATH` once at detection time.
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
        let (program, args) = self.build_command_parts(shell_command);
        let mut cmd = Command::new(program);
        if matches!(self.kind, ShellKind::Cmd) {
            #[cfg(windows)]
            {
                // Preserve quotes for `cmd /C <payload>` (issue #1691).
                if args.len() == 2 && args[0].eq_ignore_ascii_case("/C") {
                    cmd.raw_arg(&args[0]);
                    cmd.raw_arg(&args[1]);
                    return cmd;
                }
            }
        }
        cmd.args(args);
        cmd
    }

    /// Build the program + args tuple. Useful when the caller needs to
    /// inspect or modify the args before passing them to `Command`.
    pub fn build_command_parts(&self, shell_command: &str) -> (String, Vec<String>) {
        let program = self.kind.binary().to_string();
        if self.kind.is_powershell() {
            let mut args = vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
            ];
            if powershell_prefers_script_file(shell_command) {
                // Complex multiline / heavily quoted scripts: write a temp
                // .ps1 and invoke with -File so quoting stays structured.
                match write_temp_ps1(shell_command) {
                    Ok(path) => {
                        args.push("-File".to_string());
                        args.push(path);
                        return (program, args);
                    }
                    Err(_) => {
                        // Fall through to -Command if the temp file cannot be
                        // created; execution still proceeds.
                    }
                }
            }
            args.push("-Command".to_string());
            args.push(powershell_exit_aware_command(shell_command));
            return (program, args);
        }
        let args = if matches!(self.kind, ShellKind::Cmd) {
            vec!["/C".to_string(), shell_command.to_string()]
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
        #[cfg(test)]
        {
            test_env_lock::with_test_env_lock(Self::detect_shell_unlocked)
        }
        #[cfg(not(test))]
        {
            Self::detect_shell_unlocked()
        }
    }

    fn detect_shell_unlocked() -> ShellKind {
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
            if let Ok(shell) = std::env::var("SHELL")
                && let Some(kind) = Self::unix_shell_kind(&shell)
            {
                return kind;
            }

            ShellKind::Sh
        }
    }

    #[cfg(not(windows))]
    fn unix_shell_kind(shell: &str) -> Option<ShellKind> {
        let shell = shell.trim();
        if shell.is_empty() {
            return None;
        }
        let path = Path::new(shell);
        let binary = if path.is_absolute() || path.components().count() > 1 {
            shell.to_string()
        } else {
            std::env::var_os("PATH")
                .and_then(|path| {
                    std::env::split_paths(&path)
                        .map(|dir| dir.join(shell))
                        .find(|candidate| candidate.is_file())
                })
                .map_or_else(
                    || shell.to_string(),
                    |path| path.to_string_lossy().into_owned(),
                )
        };
        Some(ShellKind::Custom {
            binary,
            flag: "-c".to_string(),
        })
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
        #[cfg(windows)]
        assert_eq!(ShellKind::Sh.binary(), "sh");
        #[cfg(not(windows))]
        assert_eq!(ShellKind::Sh.binary(), "/bin/sh");
        assert_eq!(ShellKind::Bash.binary(), "bash");
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_shell_detection_preserves_absolute_executable_paths() {
        let bash = ShellDispatcher::unix_shell_kind("/bin/bash").expect("bash shell");
        assert_eq!(
            bash,
            ShellKind::Custom {
                binary: "/bin/bash".to_string(),
                flag: "-c".to_string(),
            }
        );

        let pwsh =
            ShellDispatcher::unix_shell_kind("/opt/homebrew/bin/pwsh").expect("PowerShell path");
        assert!(pwsh.is_powershell());
        assert_eq!(pwsh.binary(), "/opt/homebrew/bin/pwsh");

        let dispatcher = ShellDispatcher {
            kind: ShellDispatcher::unix_shell_kind("/bin/sh").expect("POSIX shell"),
        };
        let mut command = dispatcher.build_command("printf path-independent");
        command.env_clear();
        let output = command.output().expect("absolute shell must not need PATH");
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"path-independent");
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
        assert!(args.contains(&"-NoLogo"));
        assert!(args.contains(&"-NoProfile"));
        assert!(args.contains(&"-NonInteractive"));
        assert!(args.contains(&"-Command"));
        assert!(
            args.iter().any(|a| a.contains("echo hello")),
            "command payload missing: {args:?}"
        );
        assert!(
            args.iter().any(|a| a.contains("$LASTEXITCODE")),
            "native exit-code capture missing: {args:?}"
        );
    }

    #[test]
    fn powershell_multiline_uses_temp_file_invocation() {
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Pwsh,
        };
        let script = "Write-Output 'line1'\nWrite-Output 'line2'";
        let (program, args) = dispatcher.build_command_parts(script);
        assert!(program.contains("pwsh"));
        assert!(args.iter().any(|a| a == "-File"), "{args:?}");
        let path = args
            .iter()
            .find(|a| a.ends_with(".ps1"))
            .unwrap_or_else(|| panic!("expected temp .ps1 path: {args:?}"));
        // The script must clean up after itself and still propagate the
        // native exit code — self-delete before the exit line, so a nonzero
        // exit cannot skip the removal.
        let contents = std::fs::read_to_string(path).expect("read temp script");
        let remove_at = contents
            .find("Remove-Item -LiteralPath $MyInvocation.MyCommand.Path")
            .expect("self-delete line present");
        let exit_at = contents
            .find("if ($__codewhaleExit -ne 0) { exit $__codewhaleExit }")
            .expect("exit propagation present");
        assert!(remove_at < exit_at, "self-delete must precede exit");
        // Cleanup temp script created by the builder (the test never runs it).
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn powershell_trailing_comment_cannot_swallow_exit_capture() {
        // An unquoted `#` in a single-line payload comments to end-of-line;
        // the appended $LASTEXITCODE check must live on its own line so a
        // failing native command can never silently report success.
        let dispatcher = ShellDispatcher {
            kind: ShellKind::Pwsh,
        };
        let (_, args) = dispatcher.build_command_parts("git log --oneline -5 # recent");
        let payload = args.last().expect("command payload");
        assert!(payload.contains("# recent"), "{payload}");
        assert!(
            payload.contains("\nif ($null -ne $LASTEXITCODE"),
            "exit-code capture must start on a fresh line: {payload}"
        );
    }

    #[test]
    fn stale_temp_ps1_scripts_are_swept() {
        let dir = std::env::temp_dir();
        let stale = dir.join("codewhale-shell-0-stale-test.ps1");
        std::fs::write(&stale, "Write-Output 'stale'\n").expect("write stale script");
        // Backdate the file beyond the sweep horizon.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 60 * 60);
        let file = std::fs::File::options()
            .append(true)
            .open(&stale)
            .expect("open stale script");
        file.set_modified(old).expect("backdate stale script");
        drop(file);

        sweep_stale_temp_ps1(&dir);
        assert!(!stale.exists(), "stale script should be removed");
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
        assert!(args.contains(&"-NoLogo"));
        assert!(args.contains(&"-NoProfile"));
        assert!(args.contains(&"-NonInteractive"));
        assert!(args.contains(&"-Command"));
        assert!(
            args.iter().any(|a| a.contains("msg with spaces")),
            "quoted payload missing: {args:?}"
        );
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
