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

#[cfg(target_os = "macos")]
pub mod seatbelt;

#[cfg(target_os = "linux")]
pub mod landlock;

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
        #[cfg(windows)]
        let (program, args) = {
            // Force UTF-8 output on Windows by running `chcp 65001` before the
            // actual command. Without this, subprocesses output in the system's
            // ANSI code page (e.g. GBK for Chinese locales), causing garbled
            // text in the shell output panel. See issue #982.
            let cmd = format!("chcp 65001 >NUL & {command}");
            ("cmd".to_string(), vec!["/C".to_string(), cmd])
        };
        #[cfg(not(windows))]
        let (program, args) = (
            "sh".to_string(),
            vec!["-c".to_string(), command.to_string()],
        );

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
        if self.program == "sh" && self.args.len() == 2 && self.args[0] == "-c" {
            // For shell commands, show the actual command
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
    #[cfg(target_os = "linux")]
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
            #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
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
    #[allow(dead_code)]
    forced_sandbox: Option<SandboxType>,
}

impl SandboxManager {
    /// Create a new `SandboxManager`.
    pub fn new() -> Self {
        Self {
            sandbox_available: None,
            forced_sandbox: None,
        }
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

            #[cfg(target_os = "linux")]
            SandboxType::LinuxLandlock => Self::prepare_landlock(spec),

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
    /// Note: Landlock restricts the current process, so for subprocess sandboxing
    /// we would need a helper binary. For now, this prepares the environment with
    /// appropriate markers but doesn't actually apply Landlock (would need helper).
    #[cfg(target_os = "linux")]
    fn prepare_landlock(spec: &CommandSpec) -> ExecEnv {
        // Build the original command
        let mut command = vec![spec.program.clone()];
        command.extend(spec.args.clone());

        // Add sandbox indicator to environment
        let mut env = spec.env.clone();
        env.insert("DEEPSEEK_SANDBOX".to_string(), "landlock".to_string());

        // Note: Full Landlock implementation would use a helper binary that:
        // 1. Sets up the Landlock ruleset based on policy
        // 2. Applies restrictions to itself
        // 3. Execs the target command
        //
        // For now, we just mark that Landlock would be used

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
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = (exit_code, stderr);

        match sandbox_type {
            SandboxType::None => false,

            #[cfg(target_os = "macos")]
            SandboxType::MacosSeatbelt => seatbelt::detect_denial(exit_code, stderr),

            #[cfg(target_os = "linux")]
            SandboxType::LinuxLandlock => landlock::detect_denial(exit_code, stderr),

            #[cfg(target_os = "windows")]
            SandboxType::Windows => windows::detect_denial(exit_code, stderr),
        }
    }

    /// Get a human-readable description of why a command was blocked.
    pub fn denial_message(sandbox_type: SandboxType, stderr: &str) -> String {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
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

            #[cfg(target_os = "linux")]
            SandboxType::LinuxLandlock => {
                if stderr.contains("Permission denied") {
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

    fn expected_shell_command(command: &str) -> Vec<String> {
        #[cfg(windows)]
        {
            vec![
                "cmd".to_string(),
                "/C".to_string(),
                format!("chcp 65001 >NUL & {command}"),
            ]
        }
        #[cfg(not(windows))]
        {
            vec!["sh".to_string(), "-c".to_string(), command.to_string()]
        }
    }

    #[test]
    fn test_command_spec_shell() {
        let spec = CommandSpec::shell("echo hello", PathBuf::from("/tmp"), Duration::from_secs(30));

        #[cfg(windows)]
        {
            assert_eq!(spec.program, "cmd");
            assert_eq!(spec.args, vec!["/C", "chcp 65001 >NUL & echo hello"]);
        }
        #[cfg(not(windows))]
        {
            assert_eq!(spec.program, "sh");
            assert_eq!(spec.args, vec!["-c", "echo hello"]);
        }
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

        #[cfg(windows)]
        {
            assert_eq!(spec.program, "cmd");
            assert_eq!(
                spec.args,
                vec!["/C".to_string(), format!("chcp 65001 >NUL & {cmd}")]
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(spec.program, "sh");
            assert_eq!(spec.args, vec!["-c".to_string(), cmd.to_string()]);
            // The quoted message is intact in a single argv slot — `sh -c`
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

        assert_eq!(env.sandbox_type, SandboxType::None);
        assert_eq!(env.command, expected_shell_command("echo test"));
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
}
