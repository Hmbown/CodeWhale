//! Sanitized environment handling for child processes.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};

/// Convert a string env map into owned OS strings for child env helpers.
pub fn string_map_env(
    env: &HashMap<String, String>,
) -> impl Iterator<Item = (OsString, OsString)> + '_ {
    env.iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
}

/// Return the environment for a child process after dropping parent secrets.
///
/// `overrides` are trusted call-site values, such as sandbox markers, hook
/// variables, MCP server config, or RLM context path. They are applied after the
/// parent allowlist so explicit values win.
pub fn sanitized_child_env<I, K, V>(overrides: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut env = Vec::new();
    for (key, value) in std::env::vars_os() {
        if is_allowed_parent_env_key(&key) {
            upsert_env(&mut env, key, value);
        }
    }
    for (key, value) in overrides {
        upsert_env(
            &mut env,
            key.as_ref().to_os_string(),
            value.as_ref().to_os_string(),
        );
    }
    env
}

pub fn apply_to_command<I, K, V>(cmd: &mut std::process::Command, overrides: I)
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    cmd.env_clear();
    for (key, value) in sanitized_child_env(overrides) {
        cmd.env(key, value);
    }
}

pub fn apply_to_tokio_command<I, K, V>(cmd: &mut tokio::process::Command, overrides: I)
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    cmd.env_clear();
    for (key, value) in sanitized_child_env(overrides) {
        cmd.env(key, value);
    }
}

pub fn apply_to_pty_command<I, K, V>(cmd: &mut portable_pty::CommandBuilder, overrides: I)
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    cmd.env_clear();
    for (key, value) in sanitized_child_env(overrides) {
        cmd.env(key, value);
    }
}

/// Build the sanitized child environment used for MCP stdio servers.
///
/// MCP stdio servers are user-configured integrations declared in
/// `~/.deepseek/mcp.json` (or equivalent). They are not arbitrary processes
/// the agent decided to launch on its own. To avoid breaking common
/// `npx ...` / `uvx ...` / `python -m mcp_server_*` setups (#1244), the
/// MCP-launch allowlist is wider than the base shell-tool allowlist: it
/// also passes through Node, npm, Python, Ruby, Java, proxy, and CA-bundle
/// bootstrap variables. It still drops arbitrary parent env so secret-bearing
/// vars (`AWS_*`, `*_API_KEY`, `GITHUB_TOKEN`, …) are not silently exported.
pub fn sanitized_mcp_env<I, K, V>(overrides: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut env = Vec::new();
    for (key, value) in std::env::vars_os() {
        if is_allowed_mcp_env_key(&key) {
            upsert_env(&mut env, key, value);
        }
    }
    for (key, value) in overrides {
        upsert_env(
            &mut env,
            key.as_ref().to_os_string(),
            value.as_ref().to_os_string(),
        );
    }
    env
}

pub fn apply_to_tokio_command_mcp<I, K, V>(cmd: &mut tokio::process::Command, overrides: I)
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    cmd.env_clear();
    for (key, value) in sanitized_mcp_env(overrides) {
        cmd.env(key, value);
    }
}

fn is_allowed_parent_env_key(key: &OsStr) -> bool {
    let key = key.to_string_lossy();
    let normalized = key.to_ascii_uppercase();
    matches!(
        normalized.as_str(),
        "PATH"
            | "HOME"
            | "USER"
            | "USERNAME"
            | "LOGNAME"
            | "LANG"
            | "LANGUAGE"
            | "LC_ALL"
            | "LC_CTYPE"
            | "LC_MESSAGES"
            | "TERM"
            | "COLORTERM"
            | "NO_COLOR"
            | "FORCE_COLOR"
            | "SHELL"
            | "TMPDIR"
            | "TMP"
            | "TEMP"
            | "__CF_USER_TEXT_ENCODING"
            | "SYSTEMROOT"
            | "WINDIR"
            | "COMSPEC"
            | "PATHEXT"
            | "USERPROFILE"
            | "HOMEDRIVE"
            | "HOMEPATH"
    ) || normalized.starts_with("LC_")
}

/// Allowlist for MCP stdio launches. Strict superset of
/// `is_allowed_parent_env_key`. See `sanitized_mcp_env` for rationale.
fn is_allowed_mcp_env_key(key: &OsStr) -> bool {
    if is_allowed_parent_env_key(key) {
        return true;
    }
    let key_str = key.to_string_lossy();
    let normalized = key_str.to_ascii_uppercase();
    if matches!(
        normalized.as_str(),
        // Node.js / npm / npx / pnpm / yarn / volta / corepack
        "NVM_DIR"
            | "NVM_BIN"
            | "NVM_INC"
            | "VOLTA_HOME"
            | "COREPACK_HOME"
            | "NODE_PATH"
            | "NODE_OPTIONS"
            | "NODE_EXTRA_CA_CERTS"
            // Python ecosystem
            | "PYTHONPATH"
            | "PYTHONHOME"
            | "PYTHONDONTWRITEBYTECODE"
            | "PYTHONUNBUFFERED"
            | "VIRTUAL_ENV"
            | "POETRY_HOME"
            | "PIPX_HOME"
            | "PIPX_BIN_DIR"
            // Ruby ecosystem
            | "GEM_HOME"
            | "GEM_PATH"
            | "BUNDLE_PATH"
            | "BUNDLE_GEMFILE"
            // Java
            | "JAVA_HOME"
            // Network proxies (uppercase form; lowercase handled below)
            | "HTTP_PROXY"
            | "HTTPS_PROXY"
            | "NO_PROXY"
            | "ALL_PROXY"
            | "FTP_PROXY"
            // Custom CA bundles for corporate TLS interception
            | "SSL_CERT_FILE"
            | "SSL_CERT_DIR"
            | "REQUESTS_CA_BUNDLE"
            | "CURL_CA_BUNDLE"
    ) {
        return true;
    }
    // npm config namespace (NPM_CONFIG_PREFIX, NPM_CONFIG_CACHE, …) and
    // uv (UV_CACHE_DIR, UV_PYTHON, …) — both ecosystems use a stable prefix
    // for their bootstrap configuration, so allow the whole namespace.
    if normalized.starts_with("NPM_CONFIG_") || normalized.starts_with("UV_") {
        return true;
    }
    false
}

fn upsert_env(env: &mut Vec<(OsString, OsString)>, key: OsString, value: OsString) {
    let normalized = normalize_key(&key);
    env.retain(|(existing, _)| normalize_key(existing) != normalized);
    env.push((key, value));
}

fn normalize_key(key: &OsStr) -> String {
    key.to_string_lossy().to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn mcp_env_allowlist_inherits_base_keys() {
        for key in ["PATH", "HOME", "USER", "TERM", "LANG", "SHELL"] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should inherit base key {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_includes_node_bootstrap_keys() {
        for key in [
            "NVM_DIR",
            "NVM_BIN",
            "NVM_INC",
            "NODE_PATH",
            "NODE_OPTIONS",
            "NODE_EXTRA_CA_CERTS",
            "VOLTA_HOME",
            "COREPACK_HOME",
        ] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_includes_npm_config_prefix() {
        for key in [
            "NPM_CONFIG_PREFIX",
            "NPM_CONFIG_CACHE",
            "NPM_CONFIG_REGISTRY",
            "NPM_CONFIG_USERCONFIG",
        ] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include npm config key {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_includes_proxy_keys_either_case() {
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "NO_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "no_proxy",
            "all_proxy",
        ] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include proxy key {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_includes_python_bootstrap_keys() {
        for key in [
            "PYTHONPATH",
            "PYTHONHOME",
            "VIRTUAL_ENV",
            "PIPX_HOME",
            "PIPX_BIN_DIR",
            "POETRY_HOME",
        ] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include python bootstrap key {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_includes_uv_prefixed_keys() {
        for key in ["UV_CACHE_DIR", "UV_INDEX_URL", "UV_PYTHON"] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include uv prefixed key {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_includes_ca_bundles() {
        for key in [
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "REQUESTS_CA_BUNDLE",
            "CURL_CA_BUNDLE",
        ] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include CA bundle key {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_excludes_secrets_and_creds() {
        for key in [
            "AWS_SECRET_ACCESS_KEY",
            "AWS_ACCESS_KEY_ID",
            "GITHUB_TOKEN",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "DEEPSEEK_API_KEY",
            "SLACK_TOKEN",
            "MY_RANDOM_SECRET",
        ] {
            assert!(
                !is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist must NOT include {key}"
            );
        }
    }

    #[test]
    fn sanitized_mcp_env_passes_through_node_bootstrap() {
        let _guard = env_lock().lock().expect("env lock");
        let prev = std::env::var_os("NVM_DIR");
        unsafe {
            std::env::set_var("NVM_DIR", "/tmp/test-nvm");
        }

        let env = sanitized_mcp_env(std::iter::empty::<(OsString, OsString)>());

        match prev {
            Some(value) => unsafe { std::env::set_var("NVM_DIR", value) },
            None => unsafe { std::env::remove_var("NVM_DIR") },
        }

        let nvm_dir = env
            .iter()
            .find(|(key, _)| normalize_key(key) == "NVM_DIR")
            .map(|(_, value)| value.clone());
        assert_eq!(nvm_dir, Some(OsString::from("/tmp/test-nvm")));
    }

    #[test]
    fn sanitized_mcp_env_drops_unrelated_secret_like_values() {
        let _guard = env_lock().lock().expect("env lock");
        let prev = std::env::var_os("DEEPSEEK_MCP_TEST_SECRET");
        unsafe {
            std::env::set_var("DEEPSEEK_MCP_TEST_SECRET", "should-not-leak");
        }

        let env = sanitized_mcp_env(std::iter::empty::<(OsString, OsString)>());

        match prev {
            Some(value) => unsafe {
                std::env::set_var("DEEPSEEK_MCP_TEST_SECRET", value);
            },
            None => unsafe {
                std::env::remove_var("DEEPSEEK_MCP_TEST_SECRET");
            },
        }

        assert!(
            env.iter().all(|(key, _)| key != "DEEPSEEK_MCP_TEST_SECRET"),
            "MCP env should not pass arbitrary parent vars"
        );
    }

    #[test]
    fn sanitized_child_env_drops_parent_secret_like_values() {
        let _guard = env_lock().lock().expect("env lock");
        let previous = std::env::var_os("DEEPSEEK_CHILD_ENV_TEST_SECRET");
        unsafe {
            std::env::set_var("DEEPSEEK_CHILD_ENV_TEST_SECRET", "parent-secret");
        }

        let env = sanitized_child_env(std::iter::empty::<(OsString, OsString)>());

        match previous {
            Some(value) => unsafe {
                std::env::set_var("DEEPSEEK_CHILD_ENV_TEST_SECRET", value);
            },
            None => unsafe {
                std::env::remove_var("DEEPSEEK_CHILD_ENV_TEST_SECRET");
            },
        }

        assert!(
            env.iter()
                .all(|(key, _)| key != "DEEPSEEK_CHILD_ENV_TEST_SECRET")
        );
    }

    #[test]
    fn explicit_child_env_values_win_over_parent_allowlist() {
        let _guard = env_lock().lock().expect("env lock");
        let previous = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", "/parent/bin");
        }

        let env = sanitized_child_env([(OsString::from("PATH"), OsString::from("/explicit/bin"))]);

        match previous {
            Some(value) => unsafe {
                std::env::set_var("PATH", value);
            },
            None => unsafe {
                std::env::remove_var("PATH");
            },
        }

        let path = env
            .iter()
            .find(|(key, _)| normalize_key(key) == "PATH")
            .map(|(_, value)| value);
        assert_eq!(path, Some(&OsString::from("/explicit/bin")));
    }
}
