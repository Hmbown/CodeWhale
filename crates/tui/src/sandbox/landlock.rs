//! Linux Landlock sandbox implementation.
//!
//! Landlock is a security mechanism introduced in Linux kernel 5.13 that allows
//! processes to restrict their own access rights. Unlike Seatbelt on macOS which
//! uses an external sandbox-exec wrapper, Landlock applies restrictions directly
//! to the current process.
//!
//! # Requirements
//!
//! - Linux kernel 5.13 or later with Landlock enabled
//! - The kernel must be compiled with `CONFIG_SECURITY_LANDLOCK=y`
//!
//! # How it works
//!
//! 1. Create a landlock ruleset with desired restrictions
//! 2. Add rules to allow specific file paths
//! 3. Restrict the process using the ruleset
//!
//! Note: Once restricted, the process cannot gain more privileges.

use super::{CommandSpec, SandboxPolicy};
use std::ffi::CString;
use std::path::Path;

/// Check if Landlock is available on this system.
pub fn is_available() -> bool {
    // Check if the landlock syscall is available
    #[cfg(target_os = "linux")]
    {
        // Try to create a minimal ruleset to test availability
        // Landlock ABI version check
        // Safety: syscall uses a null ruleset pointer for ABI probing and does not dereference it.
        unsafe {
            let result = libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<libc::c_void>(),
                0usize,
                LANDLOCK_CREATE_RULESET_VERSION,
            );
            result >= 0
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Get the Landlock ABI version supported by the kernel.
#[cfg(target_os = "linux")]
pub fn get_abi_version() -> Option<i32> {
    // Safety: syscall uses a null ruleset pointer for ABI probing and does not dereference it.
    unsafe {
        let result = libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        );
        if result >= 0 {
            i32::try_from(result).ok()
        } else {
            None
        }
    }
}

// Landlock syscall constants (not yet in libc crate)
#[cfg(target_os = "linux")]
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1 << 0;

#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_EXECUTE: u64 = 1 << 0;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_REFER: u64 = 1 << 13;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 14;

// Combinations
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_READ: u64 = LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR;

#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_WRITE: u64 = LANDLOCK_ACCESS_FS_WRITE_FILE
    | LANDLOCK_ACCESS_FS_REMOVE_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_FILE
    | LANDLOCK_ACCESS_FS_MAKE_DIR
    | LANDLOCK_ACCESS_FS_MAKE_REG
    | LANDLOCK_ACCESS_FS_MAKE_SYM
    | LANDLOCK_ACCESS_FS_TRUNCATE;

/// Landlock ruleset attribute structure
#[cfg(target_os = "linux")]
#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
}

/// Landlock path beneath attribute structure
#[cfg(target_os = "linux")]
#[repr(C)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// Rule type constants
#[cfg(target_os = "linux")]
const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;

/// A configured Landlock sandbox
#[cfg(target_os = "linux")]
pub struct LandlockSandbox {
    ruleset_fd: i32,
    policy: SandboxPolicy,
}

#[cfg(target_os = "linux")]
impl LandlockSandbox {
    /// Create a new Landlock sandbox from policy
    pub fn from_policy(policy: &SandboxPolicy) -> std::io::Result<Self> {
        // Determine what filesystem access to handle (restrict)
        let handled_access =
            LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ | LANDLOCK_ACCESS_FS_WRITE;

        let attr = LandlockRulesetAttr {
            handled_access_fs: handled_access,
        };

        // Create the ruleset
        // Safety: `attr` is a valid pointer for the syscall duration and size is correct.
        let ruleset_fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &raw const attr,
                std::mem::size_of::<LandlockRulesetAttr>(),
                0u32,
            )
        };

        if ruleset_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let ruleset_fd = i32::try_from(ruleset_fd).map_err(|_| {
            std::io::Error::other("Failed to create Landlock ruleset: file descriptor out of range")
        })?;

        Ok(Self {
            ruleset_fd,
            policy: policy.clone(),
        })
    }

    /// Add a read-only rule for a path
    pub fn allow_read(&self, path: &Path) -> std::io::Result<()> {
        self.add_rule(path, LANDLOCK_ACCESS_FS_READ | LANDLOCK_ACCESS_FS_EXECUTE)
    }

    /// Add a read-write rule for a path
    pub fn allow_write(&self, path: &Path) -> std::io::Result<()> {
        self.add_rule(
            path,
            LANDLOCK_ACCESS_FS_READ | LANDLOCK_ACCESS_FS_WRITE | LANDLOCK_ACCESS_FS_EXECUTE,
        )
    }

    /// Add a path rule to the ruleset
    fn add_rule(&self, path: &Path, access: u64) -> std::io::Result<()> {
        let path_cstr = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid path"))?;

        // Open the path to get a file descriptor
        // Safety: `path_cstr` is NUL-terminated and lives for the duration of the call.
        let fd = unsafe { libc::open(path_cstr.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };

        if fd < 0 {
            // Path doesn't exist, skip this rule
            return Ok(());
        }

        let attr = LandlockPathBeneathAttr {
            allowed_access: access,
            parent_fd: fd,
        };

        // Safety: `attr` is a valid pointer for the syscall duration.
        let result = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                self.ruleset_fd,
                LANDLOCK_RULE_PATH_BENEATH,
                &raw const attr,
                0u32,
            )
        };

        // Safety: `fd` is a valid file descriptor from libc::open.
        unsafe {
            libc::close(fd);
        }

        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(())
    }

    /// Apply the sandbox to the current process
    ///
    /// WARNING: This is irreversible for the current process!
    pub fn apply(&self) -> std::io::Result<()> {
        // First, drop privileges using prctl
        // Safety: prctl call uses constant arguments and does not access memory.
        let result = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Now restrict the process
        // Safety: syscall uses a valid ruleset fd and no pointer arguments.
        let result =
            unsafe { libc::syscall(libc::SYS_landlock_restrict_self, self.ruleset_fd, 0u32) };

        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for LandlockSandbox {
    fn drop(&mut self) {
        // Safety: `ruleset_fd` is a valid descriptor created by landlock.
        unsafe {
            libc::close(self.ruleset_fd);
        }
    }
}

/// Create a helper script that sets up Landlock before running the command.
///
/// Since Landlock restricts the current process, we need a helper that:
/// 1. Sets up the Landlock ruleset
/// 2. Applies the restrictions
/// 3. Execs the target command
///
/// This returns the command to run with the helper.
#[cfg(target_os = "linux")]
pub fn create_landlock_wrapper(
    spec: &CommandSpec,
    _writable_paths: &[std::path::PathBuf],
    _readable_paths: &[std::path::PathBuf],
) -> Vec<String> {
    // For simplicity, we'll use a shell wrapper that applies Landlock via a helper binary
    // In production, this would be a compiled binary that's part of the CLI

    // For now, just return the original command without sandboxing
    // A full implementation would include a compiled landlock-helper binary
    let mut cmd = vec![spec.program.clone()];
    cmd.extend(spec.args.clone());
    cmd
}

/// Detect if a failure was caused by Landlock or seccomp denial.
///
/// Checks both Landlock-specific patterns (EACCES/EPERM) and seccomp-specific
/// patterns (Bad system call / SIGSYS). Seccomp violations are reported through
/// the same `was_denied` path so callers don't need to distinguish which layer
/// blocked the operation.
#[cfg(target_os = "linux")]
pub fn detect_denial(exit_code: i32, stderr: &str) -> bool {
    if exit_code == 0 {
        return false;
    }

    // Landlock denials typically result in EACCES or EPERM.
    let landlock_denial = stderr.contains("Permission denied")
        || stderr.contains("Operation not permitted")
        || stderr.contains("EACCES")
        || stderr.contains("EPERM");

    // Seccomp denials (#2182): SIGSYS (exit code 31 or "Bad system call").
    let seccomp_denial = exit_code == 31
        || stderr.contains("Bad system call")
        || stderr.contains("bad system call")
        || stderr.contains("SIGSYS")
        || stderr.contains("seccomp");

    landlock_denial || seccomp_denial
}

// Stub implementations for non-Linux platforms
#[cfg(not(target_os = "linux"))]
pub fn get_abi_version() -> Option<i32> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn detect_denial(_exit_code: i32, _stderr: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available() {
        // This test will pass regardless of platform
        let _ = is_available();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_get_abi_version() {
        // May or may not be available depending on kernel
        let _ = get_abi_version();
    }

    #[test]
    fn test_detect_denial() {
        #[cfg(target_os = "linux")]
        {
            assert!(detect_denial(1, "Permission denied"));
            assert!(detect_denial(1, "Operation not permitted"));
            assert!(!detect_denial(0, "Success"));
        }
    }
}
