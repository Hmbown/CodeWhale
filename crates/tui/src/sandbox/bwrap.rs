//! Bubblewrap (bwrap) passthrough for Linux sandbox (#2184).
//!
//! Bubblewrap is a setuid-less container runtime used by Flatpak and other
//! projects. It creates a new mount namespace with configurable bind mounts,
//! providing filesystem isolation without requiring root privileges.
//!
//! # How it works
//!
//! When `/usr/bin/bwrap` is executable AND the top-level config key
//! `prefer_bwrap` is set to `true`, exec_shell commands are routed through
//! bwrap. The bwrap invocation looks like:
//!
//! ```text
//! bwrap \
//!   --unshare-all \
//!   --ro-bind / / \
//!   --bind <writable-root> <writable-root> \
//!   --chdir <cwd> \
//!   -- <program> <args>
//! ```
//!
//! This creates a read-only view of the entire filesystem with write access
//! limited to the policy-derived writable roots. Policies that allow network
//! access add `--share-net` after `--unshare-all`.
//!
//! # Important
//!
//! We do NOT vendor bwrap. The user must install it themselves:
//!
//! - Ubuntu/Debian: `apt install bubblewrap`
//! - Fedora: `dnf install bubblewrap`
//! - Arch: `pacman -S bubblewrap`
//!
//! If bwrap is not executable, Codewhale reports no Linux OS sandbox and runs
//! the command without an OS wrapper. It never labels that fallback as
//! sandboxed.

#[cfg(target_os = "linux")]
use super::policy::WritableRoot;
#[cfg(target_os = "linux")]
use std::collections::BTreeSet;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

/// Canonical path to the bubblewrap binary.
#[cfg(target_os = "linux")]
pub const BWRAP_PATH: &str = "/usr/bin/bwrap";

/// Check if bubblewrap is installed and executable.
#[cfg(target_os = "linux")]
pub fn is_available() -> bool {
    is_executable(std::path::Path::new(BWRAP_PATH))
}

#[cfg(target_os = "linux")]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
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
/// policy roots.
///
/// # Arguments
///
/// - `cwd` — working directory and sandbox chdir target
/// - `program` — the program to run inside the container
/// - `args` — arguments to pass to the program
/// - `writable_roots` — policy-derived directories to remount read-write
/// - `network_access` — whether to retain the caller's network namespace
///
/// # Returns
///
/// A `Vec<String>` representing the full bwrap invocation.
#[cfg(target_os = "linux")]
pub fn build_bwrap_command(
    cwd: &std::path::Path,
    program: &str,
    args: &[String],
    writable_roots: &[WritableRoot],
    network_access: bool,
) -> Vec<String> {
    let (writable_mounts, read_only_mounts) = safe_mounts(writable_roots);
    let mut cmd: Vec<String> =
        Vec::with_capacity(10 + args.len() + 3 * (writable_mounts.len() + read_only_mounts.len()));

    cmd.push(BWRAP_PATH.to_string());

    // Isolate every supported namespace by default. `--share-net` selectively
    // retains only the network namespace when the resolved policy allows it.
    cmd.push("--unshare-all".to_string());
    if network_access {
        cmd.push("--share-net".to_string());
    }

    // Read-only bind-mount the entire root filesystem.
    cmd.push("--ro-bind".to_string());
    cmd.push("/".to_string());
    cmd.push("/".to_string());

    for root in writable_mounts {
        let root = root.to_string_lossy().into_owned();
        cmd.push("--bind".to_string());
        cmd.push(root.clone());
        cmd.push(root);
    }

    // Re-apply protected descendants after all writable parents so a broad
    // writable root cannot make .codewhale/.deepseek exceptions writable.
    for root in read_only_mounts {
        let root = root.to_string_lossy().into_owned();
        cmd.push("--ro-bind".to_string());
        cmd.push(root.clone());
        cmd.push(root);
    }

    // Change to the working directory inside the container.
    let cwd_str = cwd.to_string_lossy().to_string();
    cmd.push("--chdir".to_string());
    cmd.push(cwd_str);

    // Separator between bwrap args and the command to run.
    cmd.push("--".to_string());

    // The actual program and its arguments.
    cmd.push(program.to_string());
    cmd.extend(args.iter().cloned());

    cmd
}

#[cfg(target_os = "linux")]
fn safe_mounts(writable_roots: &[WritableRoot]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut writable = BTreeSet::new();
    let mut read_only = BTreeSet::new();

    for root in writable_roots {
        let Some(canonical_root) = safe_existing_directory(&root.root) else {
            continue;
        };
        writable.insert(canonical_root.clone());

        for exception in &root.read_only_subpaths {
            let Some(canonical_exception) = existing_directory(exception) else {
                continue;
            };
            if canonical_exception.starts_with(&canonical_root) {
                read_only.insert(canonical_exception);
            }
        }
    }

    (
        writable.into_iter().collect(),
        read_only.into_iter().collect(),
    )
}

#[cfg(target_os = "linux")]
fn safe_existing_directory(path: &Path) -> Option<PathBuf> {
    let canonical = existing_directory(path)?;
    (canonical != Path::new("/")).then_some(canonical)
}

#[cfg(target_os = "linux")]
fn existing_directory(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    canonical.is_dir().then_some(canonical)
}

/// Detect a failure attributable to the bubblewrap boundary.
#[cfg(target_os = "linux")]
pub fn detect_denial(exit_code: i32, stderr: &str) -> bool {
    exit_code != 0
        && (stderr
            .lines()
            .any(|line| line.trim_start().starts_with("bwrap:"))
            || stderr.contains("Read-only file system"))
}

#[cfg(not(target_os = "linux"))]
pub fn detect_denial(_exit_code: i32, _stderr: &str) -> bool {
    false
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
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path();
        let cmd = build_bwrap_command(
            cwd,
            "sh",
            &["-c".to_string(), "echo hi".to_string()],
            &[WritableRoot::new(cwd.to_path_buf())],
            false,
        );

        // Should start with bwrap
        assert_eq!(cmd[0], "/usr/bin/bwrap");

        // Should have ro-bind for root
        assert!(cmd.contains(&"--ro-bind".to_string()));

        // Should have --chdir
        assert!(cmd.contains(&"--chdir".to_string()));

        // Network stays isolated unless the policy explicitly allows it.
        assert!(cmd.contains(&"--unshare-all".to_string()));
        assert!(!cmd.contains(&"--share-net".to_string()));

        // Should end with the command
        assert_eq!(cmd[cmd.len() - 1], "echo hi");
        assert_eq!(cmd[cmd.len() - 2], "-c");
        assert_eq!(cmd[cmd.len() - 3], "sh");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn read_only_command_does_not_remount_the_working_directory_writable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path();
        let cmd = build_bwrap_command(cwd, "true", &[], &[], false);

        assert!(!cmd.iter().any(|arg| arg == "--bind"));
        assert!(!cmd.iter().any(|arg| arg == "--share-net"));
        assert!(
            cmd.windows(2)
                .any(|args| args[0] == "--chdir" && args[1] == cwd.to_string_lossy())
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn workspace_write_mounts_every_safe_root_and_protects_read_only_descendants() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("workspace");
        let extra = dir.path().join("extra");
        let protected = workspace.join(".codewhale");
        std::fs::create_dir_all(&protected).expect("protected directory");
        std::fs::create_dir_all(&extra).expect("extra directory");

        let roots = vec![
            WritableRoot::with_exceptions(workspace.clone(), vec![protected.clone()]),
            WritableRoot::new(extra.clone()),
            WritableRoot::new(dir.path().join("missing")),
            WritableRoot::new(PathBuf::from("/")),
        ];
        let cmd = build_bwrap_command(&workspace, "true", &[], &roots, true);

        for root in [&workspace, &extra] {
            let canonical = root.canonicalize().expect("canonical root");
            assert!(has_mount(&cmd, "--bind", &canonical));
        }
        assert!(has_mount(
            &cmd,
            "--ro-bind",
            &protected.canonicalize().expect("canonical protected path")
        ));
        assert!(!has_mount(&cmd, "--bind", Path::new("/")));
        assert!(!cmd.iter().any(|arg| arg.ends_with("/missing")));

        let unshare = cmd
            .iter()
            .position(|arg| arg == "--unshare-all")
            .expect("unshare all");
        let share = cmd
            .iter()
            .position(|arg| arg == "--share-net")
            .expect("share net");
        assert!(share > unshare);
    }

    #[cfg(target_os = "linux")]
    fn has_mount(command: &[String], flag: &str, path: &Path) -> bool {
        let path = path.to_string_lossy();
        command.windows(3).any(|args| {
            args[0] == flag
                && args[1].as_str() == path.as_ref()
                && args[2].as_str() == path.as_ref()
        })
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn executable_probe_requires_a_regular_executable_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bwrap");
        std::fs::write(&path, b"fixture").expect("write fixture");
        assert!(!is_executable(&path));

        let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("set executable bit");
        assert!(is_executable(&path));
        assert!(!is_executable(dir.path()));
    }

    #[test]
    fn denial_detection_requires_a_failed_sandbox_signal() {
        assert!(!detect_denial(0, "bwrap: ignored on success"));
        #[cfg(target_os = "linux")]
        {
            assert!(detect_denial(1, "bwrap: Creating new namespace failed"));
            assert!(detect_denial(1, "Read-only file system"));
            assert!(!detect_denial(1, "child output mentions bwrap: casually"));
            assert!(!detect_denial(1, "Permission denied"));
            assert!(!detect_denial(1, "Operation not permitted"));
            assert!(!detect_denial(1, "ordinary command failure"));
        }
    }
}
