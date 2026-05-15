//! macOS Seatbelt (sandbox-exec) profile generation.
//!
//! Seatbelt is Apple's mandatory access control framework that uses the
//! Scheme-based policy language to define what system resources a process
//! can access. This module generates sandbox profiles dynamically based
//! on the configured `SandboxPolicy`.
//!
//! # How it works
//!
//! 1. We generate a Seatbelt policy string in the SBPL format
//! 2. We invoke `/usr/bin/sandbox-exec -p <policy>` to run the command
//! 3. The kernel enforces the policy, blocking unauthorized operations
//!
//! # References
//!
//! - Apple's sandbox(7) man page
//! - <https://reverse.put.as/wp-content/uploads/2011/09/Apple-Sandbox-Guide-v1.0.pdf>

// Note: cfg(target_os = "macos") is already applied at the module level in mod.rs

use super::policy::SandboxPolicy;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Path to the sandbox-exec binary on macOS.
pub const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// Base seatbelt policy that provides minimal process functionality.
///
/// This policy:
/// - Denies everything by default
/// - Allows process execution and forking
/// - Allows signals within the same sandbox
/// - Allows reading user preferences (needed by many tools)
/// - Allows basic process introspection
/// - Allows writing to /dev/null
/// - Allows reading sysctl values
/// - Allows POSIX semaphores and pseudo-TTY operations
const SEATBELT_BASE_POLICY: &str = r#"
(version 1)
(deny default)

; Core process operations
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))

; User preferences (needed by many CLI tools)
(allow user-preference-read)

; Basic I/O to /dev/null
(allow file-write-data
  (require-all
    (path "/dev/null")
    (vnode-type CHARACTER-DEVICE)))

; System information
(allow sysctl-read)

; IPC primitives
(allow ipc-posix-sem)
(allow ipc-posix-shm-read*)
(allow ipc-posix-shm-write-create)
(allow ipc-posix-shm-write-data)
(allow ipc-posix-shm-write-unlink)

; Terminal support (essential for shell commands)
(allow pseudo-tty)
(allow file-read* file-write* file-ioctl (literal "/dev/ptmx"))
(allow file-read* file-write* file-ioctl (regex #"^/dev/ttys[0-9]+$"))

; macOS-specific device access
(allow file-read* (literal "/dev/urandom"))
(allow file-read* (literal "/dev/random"))
(allow file-ioctl (literal "/dev/dtracehelper"))

; Mach IPC (needed by many system services)
(allow mach-lookup)
"#;

/// Network access policy additions.
const SEATBELT_NETWORK_POLICY: &str = r"
; Network access
(allow network-outbound)
(allow network-inbound)
(allow system-socket)
(allow network-bind)
";

/// Check if sandbox-exec is available and permitted on this system.
pub fn is_available() -> bool {
    static SEATBELT_AVAILABLE: OnceLock<bool> = OnceLock::new();

    *SEATBELT_AVAILABLE.get_or_init(|| {
        if !Path::new(SANDBOX_EXEC_PATH).exists() {
            return false;
        }

        let output = Command::new(SANDBOX_EXEC_PATH)
            .args(["-p", "(version 1)(allow default)", "--", "/usr/bin/true"])
            .output();

        match output {
            Ok(result) => result.status.success(),
            Err(_) => false,
        }
    })
}

/// Create the command-line arguments for sandbox-exec.
///
/// Returns a Vec of arguments that should be prepended to the command.
/// The format is: `sandbox-exec -p <policy> -D KEY=VALUE ... -- <original command>`
pub fn create_seatbelt_args(
    command: Vec<String>,
    policy: &SandboxPolicy,
    sandbox_cwd: &Path,
) -> Vec<String> {
    let full_policy = generate_policy(policy, sandbox_cwd);
    let params = generate_params(policy, sandbox_cwd);

    let mut args = vec!["-p".to_string(), full_policy];

    // Add parameter definitions for variable substitution
    for (key, value) in params {
        args.push(format!("-D{}={}", key, value.to_string_lossy()));
    }

    // Separator between sandbox-exec args and the actual command
    args.push("--".to_string());
    args.extend(command);

    args
}

/// Generate the complete Seatbelt policy string for the given policy.
fn generate_policy(policy: &SandboxPolicy, cwd: &Path) -> String {
    let mut full_policy = SEATBELT_BASE_POLICY.to_string();

    // Add read access policy
    if SandboxPolicy::has_full_disk_read_access() {
        full_policy.push_str("\n; Full filesystem read access\n(allow file-read*)");
    }

    // Add write access policy
    let file_write_policy = generate_write_policy(policy, cwd);
    if !file_write_policy.is_empty() {
        full_policy.push_str("\n\n; Write access policy\n");
        full_policy.push_str(&file_write_policy);
    }

    // Add network policy if enabled
    if policy.has_network_access() {
        full_policy.push('\n');
        full_policy.push_str(SEATBELT_NETWORK_POLICY);
    }

    // Add Darwin user cache directory access (needed by many macOS tools)
    full_policy.push_str("\n\n; Darwin user cache directory\n");
    full_policy
        .push_str(r#"(allow file-read* file-write* (subpath (param "DARWIN_USER_CACHE_DIR")))"#);

    // Add common macOS directories that tools often need
    full_policy.push_str("\n\n; Common macOS directories\n");
    full_policy.push_str(r#"(allow file-read* (subpath "/usr/lib"))"#);
    full_policy.push('\n');
    full_policy.push_str(r#"(allow file-read* (subpath "/usr/share"))"#);
    full_policy.push('\n');
    full_policy.push_str(r#"(allow file-read* (subpath "/System/Library"))"#);
    full_policy.push('\n');
    full_policy.push_str(r#"(allow file-read* (subpath "/Library/Preferences"))"#);
    full_policy.push('\n');
    full_policy.push_str(r#"(allow file-read* (subpath "/private/var/db"))"#);

    // Cargo home (#558): cargo build/test/publish reach into ~/.cargo/registry
    // and ~/.cargo/git for crate metadata, downloaded tarballs, and unpacked
    // sources. Sandboxed workspace-write was previously rejecting these,
    // making `cargo publish` unrunnable from inside the TUI's shell tool.
    // Read access is always allowed; write access is granted whenever the
    // policy allows any write at all (the registry caches need to be
    // mutable for `cargo build` to populate them on a cache miss). Skipped
    // entirely when neither `CARGO_HOME` nor `HOME` is set — without one of
    // those we have no path to plumb into the policy params.
    if resolve_cargo_home().is_some() {
        full_policy.push_str("\n\n; Cargo home (~/.cargo) — registry/index/git caches\n");
        full_policy.push_str(r#"(allow file-read* (subpath (param "CARGO_HOME")))"#);
        if !matches!(policy, SandboxPolicy::ReadOnly) {
            full_policy.push('\n');
            full_policy.push_str(r#"(allow file-write* (subpath (param "CARGO_HOME_REGISTRY")))"#);
            full_policy.push('\n');
            full_policy.push_str(r#"(allow file-write* (subpath (param "CARGO_HOME_GIT")))"#);
        }
    }

    // npm cache (#1267): npx-based MCP servers write to ~/.npm when downloading
    // packages on first run. Without write access the npx subprocess fails
    // immediately with "Stdio transport closed", making all stdio MCP servers
    // broken on macOS under the default workspace-write policy.
    // Read access is always allowed; write access mirrors the cargo pattern —
    // granted for all policies that allow any write, skipped for ReadOnly.
    // Skipped entirely when neither `NPM_CONFIG_CACHE` nor `HOME` is set.
    if resolve_npm_cache_dir().is_some() {
        full_policy.push_str("\n\n; npm cache (~/.npm) — npx package downloads\n");
        full_policy.push_str(r#"(allow file-read* (subpath (param "NPM_CACHE_DIR")))"#);
        if !matches!(policy, SandboxPolicy::ReadOnly) {
            full_policy.push('\n');
            full_policy.push_str(r#"(allow file-write* (subpath (param "NPM_CACHE_DIR")))"#);
        }
    }

    full_policy
}

/// Resolve the user's cargo home — `CARGO_HOME` if set, else `$HOME/.cargo`.
/// Returns `None` only on hosts where neither env var is set (essentially
/// never on a real macOS user account; can happen in CI containers without
/// `HOME` exported).
fn resolve_cargo_home() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("CARGO_HOME")
        && !explicit.trim().is_empty()
    {
        return Some(PathBuf::from(explicit));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".cargo"))
}

/// Resolve the npm cache directory — `NPM_CONFIG_CACHE` if set, else `$HOME/.npm`.
/// Returns `None` only on hosts where neither env var is set.
fn resolve_npm_cache_dir() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("NPM_CONFIG_CACHE")
        && !explicit.trim().is_empty()
    {
        return Some(PathBuf::from(explicit));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".npm"))
}

/// Generate the write access portion of the Seatbelt policy.
fn generate_write_policy(policy: &SandboxPolicy, cwd: &Path) -> String {
    // Full disk write access
    if policy.has_full_disk_write_access() {
        return r#"(allow file-write* (regex #"^/"))"#.to_string();
    }

    // Read-only - no write policy needed
    if matches!(policy, SandboxPolicy::ReadOnly) {
        return String::new();
    }

    // Workspace write - enumerate allowed paths
    let writable_roots = policy.get_writable_roots(cwd);
    if writable_roots.is_empty() {
        return String::new();
    }

    let mut policies = Vec::new();

    for (index, root) in writable_roots.iter().enumerate() {
        let root_param = format!("WRITABLE_ROOT_{index}");

        if root.read_only_subpaths.is_empty() {
            // Simple case: entire subtree is writable
            policies.push(format!("(subpath (param \"{root_param}\"))"));
        } else {
            // Complex case: writable with read-only exceptions
            // Use require-all to combine subpath with require-not for each exception
            let mut parts = vec![format!("(subpath (param \"{}\"))", root_param)];

            for (subpath_index, _) in root.read_only_subpaths.iter().enumerate() {
                let ro_param = format!("WRITABLE_ROOT_{index}_RO_{subpath_index}");
                parts.push(format!("(require-not (subpath (param \"{ro_param}\")))"));
            }

            policies.push(format!("(require-all {})", parts.join(" ")));
        }
    }

    if policies.is_empty() {
        return String::new();
    }

    // Combine all write policies with allow
    format!("(allow file-write*\n  {})", policies.join("\n  "))
}

/// Generate parameter definitions for variable substitution in the policy.
///
/// sandbox-exec allows -DKEY=VALUE to substitute `(param "KEY")` in the policy.
fn generate_params(policy: &SandboxPolicy, cwd: &Path) -> Vec<(String, PathBuf)> {
    let mut params = Vec::new();

    // Add writable root parameters
    let writable_roots = policy.get_writable_roots(cwd);

    for (index, root) in writable_roots.iter().enumerate() {
        let canonical = root
            .root
            .canonicalize()
            .unwrap_or_else(|_| root.root.clone());
        params.push((format!("WRITABLE_ROOT_{index}"), canonical));

        // Add parameters for read-only subpaths
        for (subpath_index, subpath) in root.read_only_subpaths.iter().enumerate() {
            let canonical_subpath = subpath.canonicalize().unwrap_or_else(|_| subpath.clone());
            params.push((
                format!("WRITABLE_ROOT_{index}_RO_{subpath_index}"),
                canonical_subpath,
            ));
        }
    }

    // Add Darwin user cache directory
    if let Some(cache_dir) = get_darwin_user_cache_dir() {
        params.push(("DARWIN_USER_CACHE_DIR".to_string(), cache_dir));
    } else {
        // Fallback to a reasonable default
        if let Ok(home) = std::env::var("HOME") {
            params.push((
                "DARWIN_USER_CACHE_DIR".to_string(),
                PathBuf::from(format!("{home}/Library/Caches")),
            ));
        }
    }

    // Cargo home (#558): paired with the policy lines emitted by
    // `generate_policy` when `resolve_cargo_home()` succeeds. Both helpers
    // use the same fallback chain so the policy text and the -DKEY=VALUE
    // params stay in sync — emit one without the other and sandbox-exec
    // refuses to load the profile.
    if let Some(home) = resolve_cargo_home() {
        let canonical_home = home.canonicalize().unwrap_or_else(|_| home.clone());
        params.push((
            "CARGO_HOME_REGISTRY".to_string(),
            canonical_home.join("registry"),
        ));
        params.push(("CARGO_HOME_GIT".to_string(), canonical_home.join("git")));
        params.push(("CARGO_HOME".to_string(), canonical_home));
    }

    // npm cache (#1267): paired with the policy lines emitted by
    // `generate_policy` when `resolve_npm_cache_dir()` succeeds. Both helpers
    // use the same fallback chain so the policy text and the -DKEY=VALUE
    // params stay in sync.
    if let Some(npm_cache) = resolve_npm_cache_dir() {
        let canonical = npm_cache
            .canonicalize()
            .unwrap_or_else(|_| npm_cache.clone());
        params.push(("NPM_CACHE_DIR".to_string(), canonical));
    }

    params
}

/// Get the Darwin user cache directory using confstr.
///
/// This returns the per-user cache directory that macOS assigns,
/// typically something like /var/folders/xx/xxx.../C/
fn get_darwin_user_cache_dir() -> Option<PathBuf> {
    // Use libc to call confstr for _CS_DARWIN_USER_CACHE_DIR
    let mut buf = vec![0i8; (libc::PATH_MAX as usize) + 1];

    // Safety: `buf` is a writable buffer sized to PATH_MAX + 1 for confstr.
    let len =
        unsafe { libc::confstr(libc::_CS_DARWIN_USER_CACHE_DIR, buf.as_mut_ptr(), buf.len()) };

    if len == 0 {
        return None;
    }

    // Convert the C string to a Rust PathBuf
    // Safety: confstr guarantees a NUL-terminated string in `buf` when len > 0.
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    let path_str = cstr.to_str().ok()?;
    let path = PathBuf::from(path_str);

    // Try to canonicalize, but return the raw path if that fails
    path.canonicalize().ok().or(Some(path))
}

/// Detect sandbox denial from command output.
///
/// Returns true if the output suggests the sandbox blocked an operation.
pub fn detect_denial(exit_code: i32, stderr: &str) -> bool {
    if exit_code == 0 {
        return false;
    }

    // Common sandbox denial messages
    let denial_patterns = [
        "Operation not permitted",
        "sandbox-exec",
        "deny(",
        "Sandbox: ",
    ];

    denial_patterns.iter().any(|p| stderr.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests that mutate HOME/CARGO_HOME use crate::test_support::lock_test_env()
    // so they don't race with sibling tests in this crate that read those vars.
    #[test]
    fn test_is_available() {
        // This test just checks the function doesn't panic
        // On macOS it should return true, on other platforms false
        let _ = is_available();
    }

    #[test]
    fn test_generate_policy_default() {
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let result = generate_policy(&policy, cwd);

        assert!(result.contains("(version 1)"));
        assert!(result.contains("(deny default)"));
        assert!(result.contains("(allow file-read*)"));
        assert!(result.contains("file-write*"));
        // Default policy has no network
        assert!(!result.contains("network-outbound"));
    }

    #[test]
    fn test_generate_policy_with_network() {
        let policy = SandboxPolicy::workspace_with_network();
        let cwd = Path::new("/tmp/test");
        let result = generate_policy(&policy, cwd);

        assert!(result.contains("network-outbound"));
        assert!(result.contains("network-inbound"));
    }

    #[test]
    fn test_generate_policy_read_only() {
        let policy = SandboxPolicy::ReadOnly;
        let cwd = Path::new("/tmp/test");
        let result = generate_policy(&policy, cwd);

        assert!(result.contains("(allow file-read*)"));
        // Should not have workspace write rules
        assert!(!result.contains("WRITABLE_ROOT"));
    }

    #[test]
    fn test_generate_params() {
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let params = generate_params(&policy, cwd);

        // Should have at least the cache dir param
        assert!(params.iter().any(|(k, _)| k == "DARWIN_USER_CACHE_DIR"));
    }

    /// #558: cargo publish reaches into ~/.cargo/registry; the seatbelt has
    /// to allow read+write inside it. Both the policy text and the param
    /// table must be in sync — emitting one without the other makes
    /// sandbox-exec refuse to load the profile.
    #[test]
    fn test_cargo_home_paths_emitted_in_policy_and_params_when_home_set() {
        let _guard = crate::test_support::lock_test_env();

        // SAFETY: HOME / CARGO_HOME are process-global. lock_test_env
        // serializes tests that mutate them, and we always restore the
        // prior value before returning.
        let saved_home = std::env::var_os("HOME");
        let saved_cargo = std::env::var_os("CARGO_HOME");
        unsafe {
            std::env::set_var("HOME", "/tmp/seatbelt-cargo-test");
            std::env::remove_var("CARGO_HOME");
        }

        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");

        let policy_text = generate_policy(&policy, cwd);
        assert!(policy_text.contains(r#"(allow file-read* (subpath (param "CARGO_HOME")))"#));
        assert!(policy_text.contains("CARGO_HOME_REGISTRY"));
        assert!(policy_text.contains("CARGO_HOME_GIT"));

        let params = generate_params(&policy, cwd);
        assert!(params.iter().any(|(k, _)| k == "CARGO_HOME"));
        assert!(params.iter().any(|(k, _)| k == "CARGO_HOME_REGISTRY"));
        assert!(params.iter().any(|(k, _)| k == "CARGO_HOME_GIT"));

        // Read-only policy should still emit CARGO_HOME read rule but skip writes.
        let read_only_text = generate_policy(&SandboxPolicy::ReadOnly, cwd);
        assert!(
            read_only_text.contains(r#"(allow file-read* (subpath (param "CARGO_HOME")))"#),
            "read-only mode should still allow reading the cargo registry: {read_only_text}"
        );
        assert!(
            !read_only_text
                .contains(r#"(allow file-write* (subpath (param "CARGO_HOME_REGISTRY")))"#),
            "read-only mode must NOT grant write access to the cargo registry"
        );

        // Restore.
        // SAFETY: restoring the prior value the test stashed at entry.
        unsafe {
            match saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match saved_cargo {
                Some(v) => std::env::set_var("CARGO_HOME", v),
                None => std::env::remove_var("CARGO_HOME"),
            }
        }
    }

    /// #558: if neither `CARGO_HOME` nor `HOME` is set, the cargo lines and
    /// their params must both be omitted — emitting one without the other
    /// would crash sandbox-exec on profile load.
    #[test]
    fn test_cargo_home_skipped_when_no_env() {
        let _guard = crate::test_support::lock_test_env();

        let saved_home = std::env::var_os("HOME");
        let saved_cargo = std::env::var_os("CARGO_HOME");
        // SAFETY: HOME/CARGO_HOME are process-global; lock_test_env serializes
        // mutations here and we restore the prior values before returning.
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("CARGO_HOME");
        }

        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let policy_text = generate_policy(&policy, cwd);
        let params = generate_params(&policy, cwd);

        assert!(!policy_text.contains("CARGO_HOME"));
        assert!(!params.iter().any(|(k, _)| k.starts_with("CARGO_HOME")));

        // Restore.
        // SAFETY: restoring the prior values the test stashed at entry.
        unsafe {
            match saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match saved_cargo {
                Some(v) => std::env::set_var("CARGO_HOME", v),
                None => std::env::remove_var("CARGO_HOME"),
            }
        }
    }

    /// #1267: npx MCP servers write to ~/.npm on first run; the seatbelt must
    /// allow writes to the npm cache directory. Both the policy text and the
    /// param table must be in sync — emitting one without the other makes
    /// sandbox-exec refuse to load the profile.
    #[test]
    fn test_npm_cache_paths_emitted_in_policy_and_params_when_home_set() {
        let _guard = crate::test_support::lock_test_env();

        let saved_home = std::env::var_os("HOME");
        let saved_npm = std::env::var_os("NPM_CONFIG_CACHE");
        // SAFETY: HOME/NPM_CONFIG_CACHE are process-global; lock_test_env
        // serializes mutations here, and we always restore the prior value.
        unsafe {
            std::env::set_var("HOME", "/tmp/seatbelt-npm-test");
            std::env::remove_var("NPM_CONFIG_CACHE");
        }

        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");

        let policy_text = generate_policy(&policy, cwd);
        assert!(
            policy_text.contains(r#"(allow file-read* (subpath (param "NPM_CACHE_DIR")))"#),
            "npm cache read rule missing from policy"
        );
        assert!(
            policy_text.contains(r#"(allow file-write* (subpath (param "NPM_CACHE_DIR")))"#),
            "npm cache write rule missing from default policy"
        );

        let params = generate_params(&policy, cwd);
        assert!(
            params.iter().any(|(k, _)| k == "NPM_CACHE_DIR"),
            "NPM_CACHE_DIR param missing"
        );

        // ReadOnly policy: read access allowed, write access must be absent.
        let read_only_text = generate_policy(&SandboxPolicy::ReadOnly, cwd);
        assert!(
            read_only_text.contains(r#"(allow file-read* (subpath (param "NPM_CACHE_DIR")))"#),
            "read-only mode should allow reading the npm cache"
        );
        assert!(
            !read_only_text.contains(r#"(allow file-write* (subpath (param "NPM_CACHE_DIR")))"#),
            "read-only mode must NOT grant write access to the npm cache"
        );

        // Restore.
        // SAFETY: restoring the prior values the test stashed at entry.
        unsafe {
            match saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match saved_npm {
                Some(v) => std::env::set_var("NPM_CONFIG_CACHE", v),
                None => std::env::remove_var("NPM_CONFIG_CACHE"),
            }
        }
    }

    /// #1267: if neither `NPM_CONFIG_CACHE` nor `HOME` is set, the npm lines
    /// and their param must both be omitted.
    #[test]
    fn test_npm_cache_skipped_when_no_env() {
        let _guard = crate::test_support::lock_test_env();

        let saved_home = std::env::var_os("HOME");
        let saved_npm = std::env::var_os("NPM_CONFIG_CACHE");
        // SAFETY: HOME/NPM_CONFIG_CACHE are process-global; lock_test_env
        // serializes mutations here and we restore the prior values before returning.
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("NPM_CONFIG_CACHE");
        }

        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let policy_text = generate_policy(&policy, cwd);
        let params = generate_params(&policy, cwd);

        assert!(!policy_text.contains("NPM_CACHE_DIR"));
        assert!(!params.iter().any(|(k, _)| k == "NPM_CACHE_DIR"));

        // Restore.
        // SAFETY: restoring the prior values the test stashed at entry.
        unsafe {
            match saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match saved_npm {
                Some(v) => std::env::set_var("NPM_CONFIG_CACHE", v),
                None => std::env::remove_var("NPM_CONFIG_CACHE"),
            }
        }
    }

    #[test]
    fn test_create_seatbelt_args() {
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let command = vec!["echo".to_string(), "hello".to_string()];

        let args = create_seatbelt_args(command, &policy, cwd);

        // Should start with -p and the policy
        assert_eq!(args[0], "-p");
        assert!(args[1].contains("(version 1)"));

        // Should contain the separator
        assert!(args.contains(&"--".to_string()));

        // Should end with the original command
        assert!(args.contains(&"echo".to_string()));
        assert!(args.contains(&"hello".to_string()));
    }

    #[test]
    fn test_detect_denial() {
        assert!(detect_denial(1, "Operation not permitted"));
        assert!(detect_denial(1, "Sandbox: ls denied file-write*"));
        assert!(!detect_denial(0, "Operation not permitted"));
        assert!(!detect_denial(1, "File not found"));
    }
}
