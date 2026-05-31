#![allow(dead_code)]

//! Sandbox policy definitions for command execution restrictions.
//!
//! This module defines the policies that control what resources a sandboxed
//! process can access. Policies range from full unrestricted access to
//! tightly controlled workspace-only write access.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

use super::{CommandSpec, ExecEnv};
use crate::command_safety::SafetyLevel;

/// Determines execution restrictions for shell commands.
///
/// The sandbox policy controls filesystem access, network access, and other
/// system resources for executed commands. Choose the most restrictive policy
/// that still allows your command to function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SandboxPolicy {
    /// No restrictions whatsoever. Use with extreme caution.
    ///
    /// This policy disables all sandboxing and allows full system access.
    /// Only use this when absolutely necessary and the command source is trusted.
    #[serde(rename = "danger-full-access")]
    DangerFullAccess,

    /// Read-only access to the entire filesystem.
    ///
    /// The process can read any file but cannot write anywhere.
    /// Useful for analysis tools that need broad read access.
    #[serde(rename = "read-only")]
    ReadOnly,

    /// Indicates the process is already running in an external sandbox.
    ///
    /// Use this when CodeWhale is itself running inside a container,
    /// VM, or other sandboxed environment. This avoids double-sandboxing
    /// which can cause issues.
    #[serde(rename = "external-sandbox")]
    ExternalSandbox {
        /// Whether network access is allowed in the external sandbox.
        #[serde(default)]
        network_access: bool,
    },

    /// Read-only filesystem access plus write access to specified directories.
    ///
    /// This is the default and recommended policy. It allows:
    /// - Read access to the entire filesystem (for tools, libraries, etc.)
    /// - Write access only to the current working directory and specified roots
    /// - Optional network access
    #[serde(rename = "workspace-write")]
    WorkspaceWrite {
        /// Additional directories where writes are allowed.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        writable_roots: Vec<PathBuf>,

        /// Whether outbound network connections are permitted.
        #[serde(default)]
        network_access: bool,

        /// Exclude TMPDIR from writable paths.
        #[serde(default)]
        exclude_tmpdir: bool,

        /// Exclude /tmp from writable paths.
        #[serde(default)]
        exclude_slash_tmp: bool,
    },
}

impl Default for SandboxPolicy {
    /// Returns the default policy: workspace-write with no extra roots and no network.
    fn default() -> Self {
        SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: false,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        }
    }
}

impl SandboxPolicy {
    /// Create a workspace-write policy with network access enabled.
    pub fn workspace_with_network() -> Self {
        SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: true,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        }
    }

    /// Create a workspace-write policy with additional writable directories.
    pub fn workspace_with_roots(roots: Vec<PathBuf>, network: bool) -> Self {
        SandboxPolicy::WorkspaceWrite {
            writable_roots: roots,
            network_access: network,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        }
    }

    /// Returns true if the policy allows reading any file on the filesystem.
    pub fn has_full_disk_read_access() -> bool {
        // All current policies allow full disk read access
        true
    }

    /// Returns true if the policy allows writing to any file on the filesystem.
    pub fn has_full_disk_write_access(&self) -> bool {
        matches!(
            self,
            SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. }
        )
    }

    /// Returns true if the policy allows outbound network connections.
    pub fn has_network_access(&self) -> bool {
        match self {
            SandboxPolicy::DangerFullAccess => true,
            SandboxPolicy::ReadOnly => false,
            SandboxPolicy::ExternalSandbox { network_access }
            | SandboxPolicy::WorkspaceWrite { network_access, .. } => *network_access,
        }
    }

    /// Returns true if the sandbox should be applied (not bypassed).
    pub fn should_sandbox(&self) -> bool {
        !matches!(
            self,
            SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. }
        )
    }

    /// Get the list of writable roots for this policy.
    ///
    /// This includes:
    /// - The current working directory
    /// - Any explicitly specified `writable_roots`
    /// - /tmp (unless excluded)
    /// - TMPDIR (unless excluded)
    ///
    /// For policies with full write access, returns an empty vec since
    /// there's no need to enumerate specific paths.
    pub fn get_writable_roots(&self, cwd: &Path) -> Vec<WritableRoot> {
        match self {
            // Full write access or read-only - no enumeration needed
            SandboxPolicy::DangerFullAccess
            | SandboxPolicy::ExternalSandbox { .. }
            | SandboxPolicy::ReadOnly => vec![],

            // Workspace write - enumerate all writable paths
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                exclude_tmpdir,
                exclude_slash_tmp,
                ..
            } => {
                let mut roots: Vec<PathBuf> = writable_roots.clone();

                // Add the current working directory
                if let Ok(canonical_cwd) = cwd.canonicalize() {
                    roots.push(canonical_cwd);
                } else {
                    roots.push(cwd.to_path_buf());
                }

                // Add /tmp unless excluded
                if !exclude_slash_tmp && let Ok(tmp) = Path::new("/tmp").canonicalize() {
                    roots.push(tmp);
                }

                // Add TMPDIR unless excluded
                if !exclude_tmpdir
                    && let Ok(tmpdir) = std::env::var("TMPDIR")
                    && let Ok(canonical) = Path::new(&tmpdir).canonicalize()
                {
                    roots.push(canonical);
                }

                // Convert to WritableRoot with read-only subpaths
                roots
                    .into_iter()
                    .map(|root| {
                        let mut read_only_subpaths = Vec::new();

                        // Protect .codewhale/ and .deepseek/ directories from modification
                        let codewhale_dir = root.join(".codewhale");
                        if codewhale_dir.is_dir() {
                            read_only_subpaths.push(codewhale_dir);
                        }
                        let deepseek_dir = root.join(".deepseek");
                        if deepseek_dir.is_dir() {
                            read_only_subpaths.push(deepseek_dir);
                        }

                        WritableRoot {
                            root,
                            read_only_subpaths,
                        }
                    })
                    .collect()
            }
        }
    }
}

/// A directory tree where writes are allowed, with optional read-only subpaths.
///
/// This allows fine-grained control like "allow writes to /project but not /project/.deepseek".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritableRoot {
    /// The root directory where writes are allowed.
    pub root: PathBuf,

    /// Subdirectories within root that should remain read-only.
    pub read_only_subpaths: Vec<PathBuf>,
}

impl WritableRoot {
    /// Create a new writable root with no read-only exceptions.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            read_only_subpaths: vec![],
        }
    }

    /// Create a writable root with specific read-only subpaths.
    pub fn with_exceptions(root: PathBuf, read_only: Vec<PathBuf>) -> Self {
        Self {
            root,
            read_only_subpaths: read_only,
        }
    }

    /// Check if a path is writable under this root.
    ///
    /// Returns true if the path is under the root and not under any read-only subpath.
    pub fn is_path_writable(&self, path: &Path) -> bool {
        // Must be under the root
        if !path.starts_with(&self.root) {
            return false;
        }

        // Must not be under any read-only subpath
        for subpath in &self.read_only_subpaths {
            if path.starts_with(subpath) {
                return false;
            }
        }

        true
    }
}

/// Unified trait for platform-specific sandbox executors (#2186).
///
/// Each platform module (seatbelt, landlock, windows) provides an
/// implementation of this trait. The `SandboxManager` dispatches through
/// the trait instead of calling platform-specific functions directly.
pub trait SandboxExecutor {
    /// Prepare a sandboxed execution environment from a command spec.
    ///
    /// Returns the transformed command, environment, and sandbox metadata
    /// needed to spawn the process.
    fn prepare(&self, spec: &CommandSpec) -> io::Result<ExecEnv>;

    /// Check if a command failure was caused by sandbox denial.
    fn was_denied(&self, exit_code: i32, stderr: &str) -> bool;

    /// Get a human-readable description of why the sandbox blocked the command.
    fn denial_message(&self, stderr: &str) -> String;

    /// Returns the type of sandbox this executor provides.
    fn sandbox_type(&self) -> super::SandboxType;
}

/// Map a command safety classification to the appropriate sandbox policy (#2186).
///
/// - `Safe` / `WorkspaceSafe` → use the default sandbox policy
/// - `RequiresApproval` → user must approve before execution (handled by caller)
/// - `Dangerous` → blocked unless in YOLO mode with trust
pub fn map_safety_level_to_behavior(
    level: SafetyLevel,
    default_policy: &SandboxPolicy,
) -> SandboxPolicyBehavior {
    match level {
        SafetyLevel::Safe | SafetyLevel::WorkspaceSafe => {
            SandboxPolicyBehavior::Sandboxed(default_policy.clone())
        }
        SafetyLevel::RequiresApproval => SandboxPolicyBehavior::RequiresApproval,
        SafetyLevel::Dangerous => SandboxPolicyBehavior::Blocked,
    }
}

/// Behavior decision for a sandboxed command based on safety level.
#[derive(Debug, Clone)]
pub enum SandboxPolicyBehavior {
    /// Execute with the given sandbox policy.
    Sandboxed(SandboxPolicy),
    /// User approval required before execution.
    RequiresApproval,
    /// Block execution entirely (unless YOLO+trust).
    Blocked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let policy = SandboxPolicy::default();
        assert!(matches!(policy, SandboxPolicy::WorkspaceWrite { .. }));
        assert!(!policy.has_network_access());
        assert!(policy.should_sandbox());
    }

    #[test]
    fn test_full_access_policy() {
        let policy = SandboxPolicy::DangerFullAccess;
        assert!(policy.has_full_disk_write_access());
        assert!(policy.has_network_access());
        assert!(!policy.should_sandbox());
    }

    #[test]
    fn test_read_only_policy() {
        let policy = SandboxPolicy::ReadOnly;
        assert!(!policy.has_full_disk_write_access());
        assert!(!policy.has_network_access());
        assert!(policy.should_sandbox());
    }

    #[test]
    fn test_workspace_with_network() {
        let policy = SandboxPolicy::workspace_with_network();
        assert!(policy.has_network_access());
        assert!(policy.should_sandbox());
    }

    #[test]
    fn test_writable_root_basic() {
        let root = WritableRoot::new(PathBuf::from("/project"));
        assert!(root.is_path_writable(Path::new("/project/src/main.rs")));
        assert!(!root.is_path_writable(Path::new("/other/file.txt")));
    }

    #[test]
    fn test_writable_root_with_exceptions() {
        let root = WritableRoot::with_exceptions(
            PathBuf::from("/project"),
            vec![PathBuf::from("/project/.deepseek")],
        );
        assert!(root.is_path_writable(Path::new("/project/src/main.rs")));
        assert!(!root.is_path_writable(Path::new("/project/.deepseek/config")));
    }

    #[test]
    fn test_safety_level_mapping() {
        let default = SandboxPolicy::default();

        // Safe commands get sandboxed
        assert!(matches!(
            map_safety_level_to_behavior(SafetyLevel::Safe, &default),
            SandboxPolicyBehavior::Sandboxed(_)
        ));
        assert!(matches!(
            map_safety_level_to_behavior(SafetyLevel::WorkspaceSafe, &default),
            SandboxPolicyBehavior::Sandboxed(_)
        ));

        // RequiresApproval gets RequiresApproval
        assert!(matches!(
            map_safety_level_to_behavior(SafetyLevel::RequiresApproval, &default),
            SandboxPolicyBehavior::RequiresApproval
        ));

        // Dangerous gets Blocked
        assert!(matches!(
            map_safety_level_to_behavior(SafetyLevel::Dangerous, &default),
            SandboxPolicyBehavior::Blocked
        ));
    }

    #[test]
    fn test_policy_serialization() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![PathBuf::from("/extra")],
            network_access: true,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        };

        let json = serde_json::to_string(&policy).unwrap();
        assert!(json.contains("workspace-write"));

        let parsed: SandboxPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, parsed);
    }
}
