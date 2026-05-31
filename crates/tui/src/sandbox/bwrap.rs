//! Bubblewrap (bwrap) passthrough for Linux sandbox (#2184).
//!
//! Bubblewrap is a setuid-less container runtime used by Flatpak and other
//! projects. It creates a new mount namespace with configurable bind mounts,
//! providing filesystem isolation without requiring root privileges.
//!
//! # How it works
//!
//! When `/usr/bin/bwrap` is present AND the config key `[sandbox] prefer_bwrap`
//! is set to `true`, exec_shell commands are routed through bwrap instead of
//! relying solely on Landlock. The bwrap invocation looks like:
//!
//! ```text
//! bwrap \
//!   --ro-bind / / \
//!   --bind <cwd> <cwd> \
//!   --chdir <cwd> \
//!   --unshare-all \
//!   -- <program> <args>
//! ```
//!
//! This creates a read-only view of the entire filesystem with write access
//! limited to the working directory.
//!
//! # Important
//!
//! We do NOT vendor bwrap. The user must install it themselves:
//!
//! - Ubuntu/Debian: `apt install bubblewrap`
//! - Fedora: `dnf install bubblewrap`
//! - Arch: `pacman -S bubblewrap`
//!
//! If bwrap is not installed, we fall back to Landlock.

/// Canonical path to the bubblewrap binary.
#[cfg(target_os = "linux")]
pub const BWRAP_PATH: &str = "/usr/bin/bwrap";

/// Check if bubblewrap is installed and executable.
#[cfg(target_os = "linux")]
pub fn is_available() -> bool {
    std::path::Path::new(BWRAP_PATH).exists()
}

#[cfg(not(target_os = "linux"))]
pub fn is_available() -> bool {
    false
}

/// Build a bwrap command that wraps the given program and arguments.
///
/// The returned command vector is suitable for use as `ExecEnv.command` —
/// it replaces the normal program+args with a bwrap invocation that sets
/// up a read-only root filesystem with write access only to the specified
/// working directory.
///
/// # Arguments
///
/// - `cwd` — working directory that gets writable bind-mount
/// - `program` — the program to run inside the container
/// - `args` — arguments to pass to the program
///
/// # Returns
///
/// A `Vec<String>` representing the full bwrap invocation.
#[cfg(target_os = "linux")]
pub fn build_bwrap_command(cwd: &std::path::Path, program: &str, args: &[String]) -> Vec<String> {
    let mut cmd: Vec<String> = Vec::with_capacity(10 + args.len());

    cmd.push(BWRAP_PATH.to_string());

    // Read-only bind-mount the entire root filesystem.
    cmd.push("--ro-bind".to_string());
    cmd.push("/".to_string());
    cmd.push("/".to_string());

    // Bind-mount the working directory with read-write access.
    let cwd_str = cwd.to_string_lossy().to_string();
    cmd.push("--bind".to_string());
    cmd.push(cwd_str.clone());
    cmd.push(cwd_str.clone());

    // Change to the working directory inside the container.
    cmd.push("--chdir".to_string());
    cmd.push(cwd_str);

    // Unshare all namespaces for maximum isolation.
    cmd.push("--unshare-all".to_string());

    // Separator between bwrap args and the command to run.
    cmd.push("--".to_string());

    // The actual program and its arguments.
    cmd.push(program.to_string());
    cmd.extend(args.iter().cloned());

    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available_does_not_panic() {
        let _ = is_available();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_build_bwrap_command_structure() {
        let cwd = std::path::Path::new("/home/user/project");
        let cmd = build_bwrap_command(cwd, "sh", &["-c".to_string(), "echo hi".to_string()]);

        // Should start with bwrap
        assert_eq!(cmd[0], "/usr/bin/bwrap");

        // Should have ro-bind for root
        assert!(cmd.contains(&"--ro-bind".to_string()));

        // Should have --chdir
        assert!(cmd.contains(&"--chdir".to_string()));

        // Should end with the command
        assert_eq!(cmd[cmd.len() - 1], "echo hi");
        assert_eq!(cmd[cmd.len() - 2], "-c");
        assert_eq!(cmd[cmd.len() - 3], "sh");
    }
}
