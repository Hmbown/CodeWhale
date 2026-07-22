#![allow(dead_code)]

//! Sandbox policy definitions for command execution restrictions.
//!
//! This module defines the policies that control what resources a sandboxed
//! process can access. Policies range from full unrestricted access to
//! tightly controlled workspace-only write access.

use serde::{Deserialize, Serialize};
use std::fs;
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

                // Git worktrees keep mutable metadata outside the worktree
                // directory. Allow only the gitdir and commondir derived from
                // a workspace `.git` pointer, preserving the workspace boundary
                // for all other external paths.
                for root in roots.clone() {
                    roots.extend(resolve_git_worktree_writable_roots(&root));
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

fn resolve_git_worktree_writable_roots(root: &Path) -> Vec<PathBuf> {
    let Some(pointer) = resolve_gitdir_pointer(root) else {
        return Vec::new();
    };
    let git_dir = pointer.git_dir;
    let Some(common_dir) = resolve_git_common_dir(&git_dir) else {
        return Vec::new();
    };
    if !git_dir.starts_with(common_dir.join("worktrees")) {
        return Vec::new();
    }
    if !worktree_metadata_points_back_to_workspace(&git_dir, &pointer.git_file) {
        return Vec::new();
    }

    vec![git_dir, common_dir]
}

#[derive(Debug)]
struct GitDirPointer {
    git_dir: PathBuf,
    git_file: PathBuf,
}

fn resolve_gitdir_pointer(root: &Path) -> Option<GitDirPointer> {
    let search_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for ancestor in search_root.ancestors() {
        let git_file = ancestor.join(".git");
        if !git_file.is_file() {
            continue;
        }

        let contents = fs::read_to_string(&git_file).ok()?;
        let value = contents
            .lines()
            .find_map(|line| line.strip_prefix("gitdir:"))?
            .trim();
        if value.is_empty() {
            return None;
        }

        let path = PathBuf::from(value);
        let resolved = if path.is_absolute() {
            path
        } else {
            ancestor.join(path)
        };

        return Some(GitDirPointer {
            git_dir: resolved.canonicalize().ok()?,
            git_file: git_file.canonicalize().ok()?,
        });
    }

    None
}

fn resolve_git_common_dir(git_dir: &Path) -> Option<PathBuf> {
    let contents = fs::read_to_string(git_dir.join("commondir")).ok()?;
    let value = contents.lines().next()?.trim();
    if value.is_empty() {
        return None;
    }

    let path = PathBuf::from(value);
    let resolved = if path.is_absolute() {
        path
    } else {
        git_dir.join(path)
    };

    resolved.canonicalize().ok()
}

fn worktree_metadata_points_back_to_workspace(git_dir: &Path, expected_git_file: &Path) -> bool {
    let Some(actual_git_file) = resolve_gitdir_back_pointer(git_dir) else {
        return false;
    };
    actual_git_file == expected_git_file
}

fn resolve_gitdir_back_pointer(git_dir: &Path) -> Option<PathBuf> {
    let contents = fs::read_to_string(git_dir.join("gitdir")).ok()?;
    let value = contents.lines().next()?.trim();
    if value.is_empty() {
        return None;
    }

    let path = PathBuf::from(value);
    let resolved = if path.is_absolute() {
        path
    } else {
        git_dir.join(path)
    };

    resolved.canonicalize().ok()
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
/// Platform implementations can use this trait to convert a policy into
/// wrapper-specific rules. The current `SandboxManager` command path does not
/// dispatch through this trait yet.
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
    fn workspace_write_includes_git_worktree_metadata_roots() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let common_git_dir = tmp.path().join("main-repo").join(".git");
        let worktree_git_dir = common_git_dir.join("worktrees").join("feature");
        let worktree = tmp.path().join("feature-worktree");
        std::fs::create_dir_all(&worktree_git_dir).expect("mkdir gitdir");
        std::fs::create_dir_all(&worktree).expect("mkdir worktree");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git_dir.display()),
        )
        .expect("write git pointer");
        std::fs::write(worktree_git_dir.join("commondir"), "../..").expect("write commondir");
        std::fs::write(
            worktree_git_dir.join("gitdir"),
            worktree.join(".git").display().to_string(),
        )
        .expect("write gitdir back pointer");

        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![worktree.clone()],
            network_access: true,
            exclude_tmpdir: true,
            exclude_slash_tmp: true,
        };

        let root_paths: Vec<PathBuf> = policy
            .get_writable_roots(&worktree)
            .into_iter()
            .map(|root| root.root)
            .collect();

        assert!(root_paths.contains(&worktree.canonicalize().expect("canonical worktree")));
        assert!(root_paths.contains(&worktree_git_dir.canonicalize().expect("canonical gitdir")));
        assert!(root_paths.contains(&common_git_dir.canonicalize().expect("canonical common git")));
    }

    #[test]
    fn workspace_write_resolves_git_worktree_metadata_from_subdirectory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let common_git_dir = tmp.path().join("main-repo").join(".git");
        let worktree_git_dir = common_git_dir.join("worktrees").join("feature");
        let worktree = tmp.path().join("feature-worktree");
        let nested = worktree.join("crates").join("cli");
        std::fs::create_dir_all(&worktree_git_dir).expect("mkdir gitdir");
        std::fs::create_dir_all(&nested).expect("mkdir nested worktree path");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git_dir.display()),
        )
        .expect("write git pointer");
        std::fs::write(worktree_git_dir.join("commondir"), "../..").expect("write commondir");
        std::fs::write(
            worktree_git_dir.join("gitdir"),
            worktree.join(".git").display().to_string(),
        )
        .expect("write gitdir back pointer");

        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: true,
            exclude_tmpdir: true,
            exclude_slash_tmp: true,
        };

        let root_paths: Vec<PathBuf> = policy
            .get_writable_roots(&nested)
            .into_iter()
            .map(|root| root.root)
            .collect();

        assert!(root_paths.contains(&nested.canonicalize().expect("canonical nested cwd")));
        assert!(root_paths.contains(&worktree_git_dir.canonicalize().expect("canonical gitdir")));
        assert!(root_paths.contains(&common_git_dir.canonicalize().expect("canonical common git")));
    }

    #[test]
    fn workspace_write_rejects_non_reciprocal_git_worktree_metadata() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let common_git_dir = tmp.path().join("main-repo").join(".git");
        let worktree_git_dir = common_git_dir.join("worktrees").join("feature");
        let worktree = tmp.path().join("feature-worktree");
        let other_worktree = tmp.path().join("other-worktree");
        std::fs::create_dir_all(&worktree_git_dir).expect("mkdir gitdir");
        std::fs::create_dir_all(&worktree).expect("mkdir worktree");
        std::fs::create_dir_all(&other_worktree).expect("mkdir other worktree");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git_dir.display()),
        )
        .expect("write git pointer");
        std::fs::write(worktree_git_dir.join("commondir"), "../..").expect("write commondir");
        std::fs::write(
            worktree_git_dir.join("gitdir"),
            other_worktree.join(".git").display().to_string(),
        )
        .expect("write mismatched gitdir back pointer");
        std::fs::write(
            other_worktree.join(".git"),
            "gitdir: /tmp/not-this-worktree\n",
        )
        .expect("write other git pointer");

        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![worktree.clone()],
            network_access: true,
            exclude_tmpdir: true,
            exclude_slash_tmp: true,
        };

        let root_paths: Vec<PathBuf> = policy
            .get_writable_roots(&worktree)
            .into_iter()
            .map(|root| root.root)
            .collect();

        assert!(root_paths.contains(&worktree.canonicalize().expect("canonical worktree")));
        assert!(!root_paths.contains(&worktree_git_dir.canonicalize().expect("canonical gitdir")));
        assert!(
            !root_paths.contains(&common_git_dir.canonicalize().expect("canonical common git"))
        );
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
