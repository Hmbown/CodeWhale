#![allow(dead_code)]

//! Sandbox module for secure command execution.
//!
//! This module provides sandboxing capabilities for shell commands executed by
//! CodeWhale. Sandboxing restricts what system resources a command can access,
//! preventing accidental or malicious damage to the system.
//!
//! # Platform Support
//!
//! - **macOS**: Uses Seatbelt (sandbox-exec) for mandatory access control
//! - **Linux**: Uses Landlock (kernel 5.13+) for filesystem access control
//! - **Windows**: No OS sandbox is advertised yet. The planned first helper
//!   contract is process-tree containment only via a Windows Job Object; it
//!   must not claim filesystem, network, registry, or AppContainer isolation.
//!
//! # Usage
//!
//! ```rust,ignore
//! use sandbox::{SandboxManager, CommandSpec, SandboxPolicy};
//!
//! let manager = SandboxManager::new();
//! let spec = CommandSpec::shell("ls -la", PathBuf::from("."), Duration::from_secs(30))
//!     .with_policy(SandboxPolicy::default());
//!
//! let exec_env = manager.prepare(&spec);
//! // exec_env.command now contains the sandboxed command
//! ```

pub mod backend;
pub mod opensandbox;
pub mod policy;
pub mod process_hardening;

#[cfg(target_os = "macos")]
pub mod seatbelt;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
pub mod landlock;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
pub mod seccomp;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
pub mod bwrap;

#[cfg(target_os = "windows")]
pub mod windows;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

pub use policy::SandboxPolicy;

/// Specification for a command to be executed, potentially within a sandbox.
///
/// This struct captures all the information needed to execute a command:
/// the program and arguments, working directory, environment variables,
/// timeout, and sandbox policy.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    /// The program to execute (e.g., "sh", "python", "cargo").
    pub program: String,

    /// Arguments to pass to the program.
    pub args: Vec<String>,

    /// Working directory for the command.
    pub cwd: PathBuf,

    /// Additional environment variables to set.
    pub env: HashMap<String, String>,

    /// Maximum execution time before the command is killed.
    pub timeout: Duration,

    /// Sandbox policy controlling resource access.
    pub sandbox_policy: SandboxPolicy,

    /// Optional justification for why this command needs to run.
    /// Used for logging and audit purposes.
    pub justification: Option<String>,
}

impl CommandSpec {
    /// Create a `CommandSpec` for running a shell command via the platform shell.
    pub fn shell(command: &str, cwd: PathBuf, timeout: Duration) -> Self {
        let dispatcher = crate::shell_dispatcher::global_dispatcher();

        #[cfg(windows)]
        let (program, args) = {
            // Force UTF-8 output. cmd.exe uses chcp; PowerShell sets the
            // console output encoding directly. See issue #982.
            let kind = dispatcher.kind();
            let cmd = if matches!(
                kind,
                crate::shell_dispatcher::ShellKind::Pwsh
                    | crate::shell_dispatcher::ShellKind::WindowsPowerShell
            ) {
                format!("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {command}")
            } else if matches!(kind, crate::shell_dispatcher::ShellKind::Cmd) {
                format!("chcp 65001 >NUL & {command}")
            } else {
                command.to_string()
            };
            dispatcher.build_command_parts(&cmd)
        };
        #[cfg(not(windows))]
        let (program, args) = dispatcher.build_command_parts(command);

        Self {
            program,
            args,
            cwd,
            env: HashMap::new(),
            timeout,
            sandbox_policy: SandboxPolicy::default(),
            justification: None,
        }
    }

    /// Create a `CommandSpec` for running a program directly.
    pub fn program(program: &str, args: Vec<String>, cwd: PathBuf, timeout: Duration) -> Self {
        Self {
            program: program.to_string(),
            args,
            cwd,
            env: HashMap::new(),
            timeout,
            sandbox_policy: SandboxPolicy::default(),
            justification: None,
        }
    }

    /// Set the sandbox policy for this command.
    pub fn with_policy(mut self, policy: SandboxPolicy) -> Self {
        self.sandbox_policy = policy;
        self
    }

    /// Add environment variables for this command.
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Add a single environment variable.
    pub fn with_env_var(mut self, key: &str, value: &str) -> Self {
        self.env.insert(key.to_string(), value.to_string());
        self
    }

    /// Set a justification for this command (for logging/audit).
    pub fn with_justification(mut self, justification: &str) -> Self {
        self.justification = Some(justification.to_string());
        self
    }

    /// Get the original command as a single string (for display).
    pub fn display_command(&self) -> String {
        if self.args.len() == 2
            && self.args[0] == "-c"
            && matches!(
                self.program.as_str(),
                "sh" | "bash" | "/bin/sh" | "/bin/bash" | "/usr/bin/sh" | "/usr/bin/bash"
            )
        {
            // For shell commands, show the actual command
            self.args[1].clone()
        } else if self.args.len() == 2
            && self.args[0] == "-c"
            && !self.program.eq_ignore_ascii_case("cmd")
            && !self.program.eq_ignore_ascii_case("pwsh")
            && !self.program.eq_ignore_ascii_case("pwsh.exe")
            && !self.program.eq_ignore_ascii_case("powershell")
            && !self.program.eq_ignore_ascii_case("powershell.exe")
        {
            self.args[1].clone()
        } else if self.program.eq_ignore_ascii_case("cmd")
            && self.args.len() == 2
            && self.args[0].eq_ignore_ascii_case("/C")
        {
            // Strip the `chcp 65001 >NUL & ` prefix we add on Windows for
            // UTF-8 output (issue #982).
            let raw = &self.args[1];
            raw.strip_prefix("chcp 65001 >NUL & ")
                .unwrap_or(raw)
                .to_string()
        } else if {
            let program = self.program.to_ascii_lowercase();
            program == "pwsh"
                || program == "pwsh.exe"
                || program == "powershell"
                || program == "powershell.exe"
        } && self.args.len() >= 3
            && self.args[0].eq_ignore_ascii_case("-NoProfile")
            && self.args[1].eq_ignore_ascii_case("-Command")
        {
            // Strip the PowerShell encoding prefix.
            let raw = &self.args[2];
            raw.strip_prefix("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; ")
                .unwrap_or(raw)
                .to_string()
        } else {
            // For other commands, join program and args
            let mut parts = vec![self.program.clone()];
            parts.extend(self.args.clone());
            parts.join(" ")
        }
    }
}

/// The type of sandbox being used for execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SandboxType {
    /// No sandboxing - command runs with full permissions.
    #[default]
    None,

    /// macOS Seatbelt (sandbox-exec) sandboxing.
    #[cfg(target_os = "macos")]
    MacosSeatbelt,

    /// Linux Landlock sandboxing (kernel 5.13+).
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    LinuxLandlock,

    /// Windows process-containment helper.
    ///
    /// Not advertised until a helper enforces Job Object cleanup. This does
    /// not imply filesystem, network, registry, or AppContainer isolation.
    #[cfg(target_os = "windows")]
    Windows,
}

impl std::fmt::Display for SandboxType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxType::None => write!(f, "none"),
            #[cfg(target_os = "macos")]
            SandboxType::MacosSeatbelt => write!(f, "macos-seatbelt"),
            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            SandboxType::LinuxLandlock => write!(f, "linux-landlock"),
            #[cfg(target_os = "windows")]
            SandboxType::Windows => write!(f, "windows-sandbox"),
        }
    }
}

/// The execution environment after sandbox transformation.
///
/// This contains the actual command to run (which may include sandbox wrapper
/// commands) and all necessary environment configuration.
#[derive(Debug)]
pub struct ExecEnv {
    /// The full command to execute (may include sandbox wrapper).
    pub command: Vec<String>,

    /// Working directory for execution.
    pub cwd: PathBuf,

    /// Environment variables to set.
    pub env: HashMap<String, String>,

    /// Timeout for the command.
    pub timeout: Duration,

    /// The type of sandbox being used.
    pub sandbox_type: SandboxType,

    /// The original policy (for reference).
    pub policy: SandboxPolicy,
}

impl ExecEnv {
    /// Get the program to execute (first element of command).
    pub fn program(&self) -> &str {
        self.command
            .first()
            .map_or("sh", std::string::String::as_str)
    }

    /// Get the arguments (all elements after the first).
    pub fn args(&self) -> &[String] {
        if self.command.len() > 1 {
            &self.command[1..]
        } else {
            &[]
        }
    }

    /// Check if this execution is sandboxed.
    pub fn is_sandboxed(&self) -> bool {
        !matches!(self.sandbox_type, SandboxType::None)
    }
}

/// Detect what sandbox technology is available on the current platform.
pub fn get_platform_sandbox() -> Option<SandboxType> {
    #[cfg(target_os = "macos")]
    {
        if seatbelt::is_available() {
            return Some(SandboxType::MacosSeatbelt);
        }
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    {
        if landlock::is_available() {
            return Some(SandboxType::LinuxLandlock);
        }
    }

    #[cfg(target_os = "windows")]
    {
        if windows::is_available() {
            return Some(SandboxType::Windows);
        }
    }

    None
}

/// Check if sandboxing is available on this platform.
pub fn is_sandbox_available() -> bool {
    get_platform_sandbox().is_some()
}

/// Manager for sandbox operations.
///
/// The `SandboxManager` is responsible for:
/// - Detecting available sandbox technologies
/// - Transforming `CommandSpecs` into sandboxed `ExecEnvs`
/// - Detecting sandbox denials from command output
#[derive(Debug, Default)]
pub struct SandboxManager {
    /// Cached sandbox availability check.
    sandbox_available: Option<bool>,

    /// Force a specific sandbox type (for testing).
    #[allow(dead_code)] // Testing aid; needed when sandbox tests are enabled (see #3490)
    forced_sandbox: Option<SandboxType>,

    /// When true and bwrap is available on Linux, route commands through
    /// bubblewrap instead of Landlock alone (#2184).
    prefer_bwrap: bool,
}

impl SandboxManager {
    /// Create a new `SandboxManager`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new `SandboxManager` with bwrap preference (#2184).
    ///
    /// When `prefer_bwrap` is true and `/usr/bin/bwrap` is present on Linux,
    /// exec_shell commands will be routed through bubblewrap.
    pub fn with_bwrap_preference(prefer_bwrap: bool) -> Self {
        Self {
            prefer_bwrap,
            ..Self::default()
        }
    }

    /// Set the bwrap preference (#2184).
    pub fn set_prefer_bwrap(&mut self, prefer: bool) {
        self.prefer_bwrap = prefer;
    }

    /// Check if sandboxing is available.
    pub fn is_available(&mut self) -> bool {
        if let Some(available) = self.sandbox_available {
            return available;
        }

        let available = is_sandbox_available();
        self.sandbox_available = Some(available);
        available
    }

    /// Select the appropriate sandbox type for the given policy.
    pub fn select_sandbox(&self, policy: &SandboxPolicy) -> SandboxType {
        // If the policy doesn't want sandboxing, return None
        if !policy.should_sandbox() {
            return SandboxType::None;
        }

        // Check for forced sandbox (testing)
        if let Some(forced) = self.forced_sandbox {
            return forced;
        }

        // Use platform default
        get_platform_sandbox().unwrap_or(SandboxType::None)
    }

    /// Transform a `CommandSpec` into a sandboxed `ExecEnv`.
    ///
    /// This is the main entry point for sandboxing. It takes a command
    /// specification and returns the actual command to run, which may
    /// include sandbox wrapper commands.
    pub fn prepare(&self, spec: &CommandSpec) -> ExecEnv {
        let sandbox_type = self.select_sandbox(&spec.sandbox_policy);

        match sandbox_type {
            SandboxType::None => Self::prepare_unsandboxed(spec),

            #[cfg(target_os = "macos")]
            SandboxType::MacosSeatbelt => Self::prepare_seatbelt(spec),

            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            SandboxType::LinuxLandlock => self.prepare_landlock(spec),

            #[cfg(target_os = "windows")]
            SandboxType::Windows => Self::prepare_windows(spec),
        }
    }

    /// Prepare an unsandboxed execution environment.
    fn prepare_unsandboxed(spec: &CommandSpec) -> ExecEnv {
        let mut command = vec![spec.program.clone()];
        command.extend(spec.args.clone());

        ExecEnv {
            command,
            cwd: spec.cwd.clone(),
            env: spec.env.clone(),
            timeout: spec.timeout,
            sandbox_type: SandboxType::None,
            policy: spec.sandbox_policy.clone(),
        }
    }

    /// Prepare a Seatbelt-sandboxed execution environment (macOS).
    #[cfg(target_os = "macos")]
    fn prepare_seatbelt(spec: &CommandSpec) -> ExecEnv {
        // Build the original command
        let mut original_command = vec![spec.program.clone()];
        original_command.extend(spec.args.clone());

        // Generate sandbox-exec arguments
        let seatbelt_args =
            seatbelt::create_seatbelt_args(original_command, &spec.sandbox_policy, &spec.cwd);

        // Prepend sandbox-exec to the command
        let mut command = vec![seatbelt::SANDBOX_EXEC_PATH.to_string()];
        command.extend(seatbelt_args);

        // Add sandbox indicator to environment
        let mut env = spec.env.clone();
        env.insert("DEEPSEEK_SANDBOX".to_string(), "seatbelt".to_string());

        ExecEnv {
            command,
            cwd: spec.cwd.clone(),
            env,
            timeout: spec.timeout,
            sandbox_type: SandboxType::MacosSeatbelt,
            policy: spec.sandbox_policy.clone(),
        }
    }

    /// Prepare a Landlock-sandboxed execution environment (Linux).
    ///
    /// If `prefer_bwrap` is set and `/usr/bin/bwrap` is available, routes the
    /// command through bubblewrap for stronger filesystem isolation (#2184).
    /// Otherwise falls back to Landlock markers.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    fn prepare_landlock(&self, spec: &CommandSpec) -> ExecEnv {
        // Check if bwrap passthrough should be used (#2184).
        if self.prefer_bwrap && bwrap::is_available() {
            let command = bwrap::build_bwrap_command(&spec.cwd, &spec.program, &spec.args);

            let mut env = spec.env.clone();
            env.insert("DEEPSEEK_SANDBOX".to_string(), "bwrap".to_string());

            return ExecEnv {
                command,
                cwd: spec.cwd.clone(),
                env,
                timeout: spec.timeout,
                sandbox_type: SandboxType::LinuxLandlock,
                policy: spec.sandbox_policy.clone(),
            };
        }

        // Fall back to Landlock (marker only — full implementation needs a helper).
        let mut command = vec![spec.program.clone()];
        command.extend(spec.args.clone());

        let mut env = spec.env.clone();
        env.insert("DEEPSEEK_SANDBOX".to_string(), "landlock".to_string());

        ExecEnv {
            command,
            cwd: spec.cwd.clone(),
            env,
            timeout: spec.timeout,
            sandbox_type: SandboxType::LinuxLandlock,
            policy: spec.sandbox_policy.clone(),
        }
    }

    /// Prepare a Windows helper execution environment.
    ///
    /// Windows support is currently not advertised by `get_platform_sandbox`.
    /// This branch only exists for forced tests and future helper wiring.
    /// The first supported helper contract is process-tree containment only;
    /// it must not be presented as filesystem or network isolation.
    #[cfg(target_os = "windows")]
    fn prepare_windows(spec: &CommandSpec) -> ExecEnv {
        let mut command = vec![spec.program.clone()];
        command.extend(spec.args.clone());

        let mut env = spec.env.clone();
        let kind = windows::select_best_kind(&spec.sandbox_policy, &spec.cwd);
        env.insert("DEEPSEEK_SANDBOX".to_string(), format!("windows:{kind}"));
        if !spec.sandbox_policy.has_network_access() {
            env.insert(
                "DEEPSEEK_SANDBOX_BLOCK_NETWORK".to_string(),
                "1".to_string(),
            );
        }

        ExecEnv {
            command,
            cwd: spec.cwd.clone(),
            env,
            timeout: spec.timeout,
            sandbox_type: SandboxType::Windows,
            policy: spec.sandbox_policy.clone(),
        }
    }

    /// Check if a command failure was due to sandbox denial.
    ///
    /// This helps distinguish between legitimate command failures and
    /// sandbox-blocked operations.
    pub fn was_denied(sandbox_type: SandboxType, exit_code: i32, stderr: &str) -> bool {
        #[cfg(not(any(
            target_os = "macos",
            all(target_os = "linux", not(target_env = "ohos"))
        )))]
        let _ = (exit_code, stderr);

        match sandbox_type {
            SandboxType::None => false,

            #[cfg(target_os = "macos")]
            SandboxType::MacosSeatbelt => seatbelt::detect_denial(exit_code, stderr),

            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            SandboxType::LinuxLandlock => landlock::detect_denial(exit_code, stderr),

            #[cfg(target_os = "windows")]
            SandboxType::Windows => windows::detect_denial(exit_code, stderr),
        }
    }

    /// Get a human-readable description of why a command was blocked.
    pub fn denial_message(sandbox_type: SandboxType, stderr: &str) -> String {
        #[cfg(not(any(
            target_os = "macos",
            all(target_os = "linux", not(target_env = "ohos"))
        )))]
        let _ = stderr;

        match sandbox_type {
            SandboxType::None => "Command failed (no sandbox)".to_string(),

            #[cfg(target_os = "macos")]
            SandboxType::MacosSeatbelt => {
                if stderr.contains("file-write") {
                    "Sandbox blocked write access. The command tried to write to a protected location.".to_string()
                } else if stderr.contains("network") {
                    "Sandbox blocked network access. Enable network_access in sandbox policy if needed.".to_string()
                } else {
                    format!(
                        "Sandbox blocked operation: {}",
                        stderr.lines().next().unwrap_or("unknown")
                    )
                }
            }

            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            SandboxType::LinuxLandlock => {
                // Seccomp patterns checked first because they are more specific (#2182).
                if stderr.contains("Bad system call")
                    || stderr.contains("bad system call")
                    || stderr.contains("SIGSYS")
                    || stderr.contains("seccomp")
                {
                    "Seccomp blocked a disallowed system call (e.g., ptrace, mount, kexec)."
                        .to_string()
                } else if stderr.contains("Permission denied") {
                    "Landlock blocked access. The command tried to access a restricted path."
                        .to_string()
                } else {
                    format!(
                        "Landlock blocked operation: {}",
                        stderr.lines().next().unwrap_or("unknown")
                    )
                }
            }

            #[cfg(target_os = "windows")]
            SandboxType::Windows => {
                if stderr.contains("Access is denied") {
                    "Windows sandbox blocked access. The command lacked required privileges."
                        .to_string()
                } else if stderr.contains("network") {
                    "Windows sandbox blocked network access. Enable network_access in policy if needed."
                        .to_string()
                } else {
                    format!(
                        "Windows sandbox blocked operation: {}",
                        stderr.lines().next().unwrap_or("unknown")
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_spec_shell() {
        let spec = CommandSpec::shell("echo hello", PathBuf::from("/tmp"), Duration::from_secs(30));

        // Program and args depend on the detected shell.
        assert!(!spec.program.is_empty(), "program must not be empty");
        assert!(!spec.args.is_empty(), "args must not be empty");
        assert_eq!(spec.display_command(), "echo hello");
    }

    #[test]
    fn test_command_spec_shell_custom_posix_path_display() {
        let spec = CommandSpec {
            program: "/bin/zsh".to_string(),
            args: vec!["-c".to_string(), "echo hello".to_string()],
            cwd: PathBuf::from("/tmp"),
            env: HashMap::new(),
            timeout: Duration::from_secs(30),
            sandbox_policy: SandboxPolicy::default(),
            justification: None,
        };

        assert_eq!(spec.display_command(), "echo hello");
    }

    #[test]
    fn test_command_spec_shell_quoted_arg_not_split() {
        // Regression for #1691: a `-m` message containing spaces must remain a
        // single, unsplit argv entry. The shell command string is passed
        // verbatim as ONE argument (`sh -c <cmd>` / `cmd /C <payload>`); we
        // must never tokenize it ourselves into `feat:` / `complete` /
        // `sub-pages"`.
        let cmd = r#"git commit -m "feat: complete sub-pages""#;
        let spec = CommandSpec::shell(cmd, PathBuf::from("/tmp"), Duration::from_secs(30));

        let dispatcher = crate::shell_dispatcher::global_dispatcher();
        assert_eq!(spec.program, dispatcher.kind().binary());
        if dispatcher.kind().is_powershell() {
            assert_eq!(
                spec.args,
                vec![
                    dispatcher.kind().command_flag().to_string(),
                    "-Command".to_string(),
                    format!("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {cmd}")
                ]
            );
        } else {
            let expected = if matches!(dispatcher.kind(), crate::shell_dispatcher::ShellKind::Cmd) {
                vec!["/C".to_string(), format!("chcp 65001 >NUL & {cmd}")]
            } else {
                vec![
                    dispatcher.kind().command_flag().to_string(),
                    cmd.to_string(),
                ]
            };
            assert_eq!(spec.args, expected);
            // The quoted message is intact in a single argv slot — shell `-c`
            // performs POSIX tokenization, yielding the correct argv:
            // ["git","commit","-m","feat: complete sub-pages"].
            assert_eq!(spec.args.len(), 2);
            assert!(spec.args[1].contains(r#""feat: complete sub-pages""#));
        }
        assert_eq!(spec.display_command(), cmd);
    }

    #[test]
    fn test_command_spec_program() {
        let spec = CommandSpec::program(
            "cargo",
            vec!["build".to_string(), "--release".to_string()],
            PathBuf::from("/project"),
            Duration::from_secs(300),
        );

        assert_eq!(spec.program, "cargo");
        assert_eq!(spec.display_command(), "cargo build --release");
    }

    #[test]
    fn test_command_spec_builder() {
        let spec = CommandSpec::shell("test", PathBuf::from("."), Duration::from_secs(10))
            .with_policy(SandboxPolicy::ReadOnly)
            .with_env_var("FOO", "bar")
            .with_justification("Testing");

        assert!(matches!(spec.sandbox_policy, SandboxPolicy::ReadOnly));
        assert_eq!(spec.env.get("FOO"), Some(&"bar".to_string()));
        assert_eq!(spec.justification, Some("Testing".to_string()));
    }

    #[test]
    fn test_sandbox_manager_new() {
        let manager = SandboxManager::new();
        assert!(manager.sandbox_available.is_none());
    }

    #[test]
    fn test_sandbox_manager_select_sandbox() {
        let manager = SandboxManager::new();

        // DangerFullAccess should never sandbox
        let no_sandbox = manager.select_sandbox(&SandboxPolicy::DangerFullAccess);
        assert_eq!(no_sandbox, SandboxType::None);

        // ExternalSandbox should never sandbox
        let external = manager.select_sandbox(&SandboxPolicy::ExternalSandbox {
            network_access: true,
        });
        assert_eq!(external, SandboxType::None);
    }

    #[test]
    fn test_prepare_unsandboxed() {
        let manager = SandboxManager::new();
        let spec = CommandSpec::shell("echo test", PathBuf::from("/tmp"), Duration::from_secs(30))
            .with_policy(SandboxPolicy::DangerFullAccess);

        let env = manager.prepare(&spec);
        let dispatcher = crate::shell_dispatcher::global_dispatcher();

        assert_eq!(env.sandbox_type, SandboxType::None);
        if dispatcher.kind().is_powershell() {
            assert_eq!(
                env.command,
                vec![
                    dispatcher.kind().binary().to_string(),
                    dispatcher.kind().command_flag().to_string(),
                    "-Command".to_string(),
                    "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; echo test"
                        .to_string(),
                ]
            );
        } else if matches!(dispatcher.kind(), crate::shell_dispatcher::ShellKind::Cmd) {
            assert_eq!(
                env.command,
                vec![
                    dispatcher.kind().binary().to_string(),
                    "/C".to_string(),
                    "chcp 65001 >NUL & echo test".to_string(),
                ]
            );
        } else {
            assert_eq!(
                env.command,
                vec![
                    dispatcher.kind().binary().to_string(),
                    dispatcher.kind().command_flag().to_string(),
                    "echo test".to_string(),
                ]
            );
        }
        assert!(!env.is_sandboxed());
    }

    #[test]
    fn test_exec_env_helpers() {
        let env = ExecEnv {
            command: vec![
                "sandbox-exec".to_string(),
                "-p".to_string(),
                "policy".to_string(),
                "--".to_string(),
                "echo".to_string(),
                "hello".to_string(),
            ],
            cwd: PathBuf::from("/tmp"),
            env: HashMap::new(),
            timeout: Duration::from_secs(30),
            sandbox_type: SandboxType::None,
            policy: SandboxPolicy::default(),
        };

        assert_eq!(env.program(), "sandbox-exec");
        assert_eq!(env.args().len(), 5);
    }

    #[test]
    fn test_sandbox_type_display() {
        assert_eq!(format!("{}", SandboxType::None), "none");

        #[cfg(target_os = "macos")]
        assert_eq!(format!("{}", SandboxType::MacosSeatbelt), "macos-seatbelt");
    }

    // ── Parity tests (#2187) ──────────────────────────────────────────────

    #[test]
    fn test_parity_platform_sandbox_detection() {
        let sandbox_type = get_platform_sandbox();
        let available = is_sandbox_available();
        if available {
            assert!(sandbox_type.is_some());
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_parity_macos_seatbelt_available() {
        let st = get_platform_sandbox();
        assert!(matches!(st, Some(SandboxType::MacosSeatbelt)));
    }

    #[test]
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    fn test_parity_linux_landlock_available() {
        let st = get_platform_sandbox();
        assert!(matches!(st, Some(SandboxType::LinuxLandlock)));
    }

    #[test]
    fn test_parity_denial_zero_exit_never_denied() {
        assert!(!SandboxManager::was_denied(
            SandboxType::None,
            0,
            "anything"
        ));
        #[cfg(target_os = "macos")]
        assert!(!SandboxManager::was_denied(
            SandboxType::MacosSeatbelt,
            0,
            ""
        ));
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        assert!(!SandboxManager::was_denied(
            SandboxType::LinuxLandlock,
            0,
            ""
        ));
        #[cfg(target_os = "windows")]
        assert!(!SandboxManager::was_denied(SandboxType::Windows, 0, ""));
    }

    #[test]
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    fn test_parity_seccomp_sigsys_detected() {
        assert!(SandboxManager::was_denied(
            SandboxType::LinuxLandlock,
            31,
            ""
        ));
        assert!(SandboxManager::was_denied(
            SandboxType::LinuxLandlock,
            1,
            "Bad system call"
        ));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_parity_seatbelt_file_write_detected() {
        // Seatbelt patterns use "Sandbox: <cmd> denied <operation>" format.
        assert!(SandboxManager::was_denied(
            SandboxType::MacosSeatbelt,
            1,
            "Sandbox: ls denied file-write*"
        ));
        assert!(SandboxManager::was_denied(
            SandboxType::MacosSeatbelt,
            1,
            "Operation not permitted"
        ));
    }

    #[test]
    fn test_parity_manager_default_no_bwrap() {
        let manager = SandboxManager::default();
        let spec = CommandSpec::shell("true", PathBuf::from("/tmp"), Duration::from_secs(5))
            .with_policy(SandboxPolicy::default());
        let env = manager.prepare(&spec);
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            let marker = env.env.get("DEEPSEEK_SANDBOX");
            assert!(marker.is_none_or(|v| v != "bwrap"));
        }
        let _ = env;
    }

    #[test]
    fn test_parity_manager_with_bwrap() {
        let manager = SandboxManager::with_bwrap_preference(true);
        let spec = CommandSpec::shell("true", PathBuf::from("/tmp"), Duration::from_secs(5))
            .with_policy(SandboxPolicy::default());
        let env = manager.prepare(&spec);
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            if crate::sandbox::bwrap::is_available() {
                let marker = env.env.get("DEEPSEEK_SANDBOX");
                assert_eq!(marker.map(String::as_str), Some("bwrap"));
            }
        }
        let _ = env;
    }

    #[test]
    fn test_parity_exec_env_for_all_policies() {
        let manager = SandboxManager::new();
        let policies = [
            SandboxPolicy::DangerFullAccess,
            SandboxPolicy::ReadOnly,
            SandboxPolicy::workspace_with_network(),
            SandboxPolicy::default(),
        ];
        for policy in &policies {
            let spec = CommandSpec::shell("true", PathBuf::from("/tmp"), Duration::from_secs(5))
                .with_policy(policy.clone());
            let env = manager.prepare(&spec);
            assert_eq!(env.policy, *policy);
        }
    }
}
