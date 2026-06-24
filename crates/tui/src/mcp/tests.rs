use super::headers::{MCP_HTTP_ACCEPT, is_safe_custom_header, with_default_mcp_http_headers};
use super::*;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock};

fn test_http_client() -> reqwest::Client {
    let _ = rustls::crypto::ring::default_provider().install_default();
    crate::tls::reqwest_client()
}

async fn lock_mcp_loopback_tests() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

struct WorkspaceTrustConfigGuard {
    config_path: PathBuf,
    _codewhale_config_path: crate::test_support::EnvVarGuard,
    _deepseek_config_path: crate::test_support::EnvVarGuard,
    _env_lock: std::sync::MutexGuard<'static, ()>,
}

fn workspace_trust_config_guard(workspace: &Path) -> WorkspaceTrustConfigGuard {
    let env_lock = crate::test_support::lock_test_env();
    let config_path = workspace
        .parent()
        .unwrap_or(workspace)
        .join("user-config")
        .join("config.toml");
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let codewhale_config_path =
        crate::test_support::EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let deepseek_config_path = crate::test_support::EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

    WorkspaceTrustConfigGuard {
        config_path,
        _codewhale_config_path: codewhale_config_path,
        _deepseek_config_path: deepseek_config_path,
        _env_lock: env_lock,
    }
}

fn write_workspace_trust_config(config_path: &Path, workspace: &Path) {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let key = workspace
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    fs::write(
        config_path,
        format!("[projects.\"{key}\"]\ntrust_level = \"trusted\"\n"),
    )
    .unwrap();
}

fn mark_workspace_trusted(workspace: &Path) -> WorkspaceTrustConfigGuard {
    let guard = workspace_trust_config_guard(workspace);
    write_workspace_trust_config(&guard.config_path, workspace);
    guard
}

#[test]
fn test_mcp_config_defaults() {
    let config = McpConfig::default();
    assert_eq!(config.timeouts.connect_timeout, 10);
    assert_eq!(config.timeouts.execute_timeout, 60);
    assert_eq!(config.timeouts.read_timeout, 120);
    assert!(config.servers.is_empty());
}

#[test]
fn test_mcp_config_parse() {
    let json = r#"{
        "timeouts": {
            "connect_timeout": 15,
            "execute_timeout": 90
        },
        "servers": {
            "test": {
                "command": "node",
                "args": ["server.js"],
                "env": {"FOO": "bar"}
            }
        }
    }"#;

    let config: McpConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.timeouts.connect_timeout, 15);
    assert_eq!(config.timeouts.execute_timeout, 90);
    assert_eq!(config.timeouts.read_timeout, 120); // default
    assert!(config.servers.contains_key("test"));

    let server = config.servers.get("test").unwrap();
    assert_eq!(server.command, Some("node".to_string()));
    assert_eq!(server.args, vec!["server.js"]);
    assert_eq!(server.env.get("FOO"), Some(&"bar".to_string()));
}

#[test]
fn mcp_pool_parse_prefixed_name_preserves_registered_underscored_server() {
    let config: McpConfig = serde_json::from_str(
        r#"{
            "servers": {
                "my": {"command": "node"},
                "my_db": {"command": "node"}
            }
        }"#,
    )
    .unwrap();
    let pool = McpPool::new(config);

    let (server, tool) = pool
        .parse_prefixed_name("mcp_my_db_execute_sql")
        .expect("registered underscored server should parse");

    assert_eq!(server, "my_db");
    assert_eq!(tool, "execute_sql");
}

#[test]
fn mcp_server_config_parses_custom_headers() {
    let json = r#"{
        "servers": {
            "hf": {
                "url": "https://example.invalid/mcp",
                "headers": {
                    "Authorization": "Bearer tok",
                    "X-Org": "anthropic"
                }
            }
        }
    }"#;
    let cfg: McpConfig = serde_json::from_str(json).unwrap();
    let hf = cfg.servers.get("hf").expect("server present");
    assert_eq!(
        hf.headers.get("Authorization"),
        Some(&"Bearer tok".to_string())
    );
    assert_eq!(hf.headers.get("X-Org"), Some(&"anthropic".to_string()));
}

#[test]
fn mcp_server_config_omits_headers_when_empty() {
    // Empty headers map should not appear in the serialized output —
    // older mcp.json files written before v0.8.31 must round-trip
    // unchanged so a `mcp save` from a fresh install doesn't add
    // dead keys.
    let cfg = McpServerConfig {
        command: Some("node".into()),
        args: vec!["server.js".into()],
        env: HashMap::new(),
        cwd: None,
        url: None,
        transport: None,
        connect_timeout: None,
        execute_timeout: None,
        read_timeout: None,
        disabled: false,
        enabled: true,
        required: false,
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        headers: HashMap::new(),
    };
    let serialized = serde_json::to_string(&cfg).unwrap();
    assert!(
        !serialized.contains("\"headers\""),
        "empty headers must be omitted: {serialized}"
    );
}

#[test]
fn is_safe_custom_header_accepts_normal_auth_pairs() {
    assert!(is_safe_custom_header("Authorization", "Bearer tok"));
    assert!(is_safe_custom_header("X-Api-Key", "deadbeef"));
    assert!(is_safe_custom_header("x-org", "anthropic"));
}

#[test]
fn is_safe_custom_header_rejects_empty_or_whitespace_key() {
    assert!(!is_safe_custom_header("", "value"));
    assert!(!is_safe_custom_header("   ", "value"));
}

#[test]
fn is_safe_custom_header_rejects_response_splitting_values() {
    assert!(
        !is_safe_custom_header("X-Foo", "abc\r\nSet-Cookie: evil=1"),
        "CRLF in value must reject — response-splitting defense"
    );
    assert!(
        !is_safe_custom_header("X-Foo", "abc\nbar"),
        "bare LF in value must reject"
    );
    assert!(
        !is_safe_custom_header("X-Foo", "abc\rbar"),
        "bare CR in value must reject"
    );
}

#[test]
fn is_safe_custom_header_rejects_protocol_framing_overrides() {
    // The MCP Streamable HTTP transport relies on its own
    // Accept / Content-Type values for protocol negotiation;
    // a stray user override would silently break tool discovery.
    assert!(!is_safe_custom_header("Accept", "text/plain"));
    assert!(!is_safe_custom_header("accept", "text/plain"));
    assert!(!is_safe_custom_header("Content-Type", "text/plain"));
    assert!(!is_safe_custom_header("CONTENT-TYPE", "x/y"));
}

#[test]
fn default_mcp_http_get_accepts_json_and_event_stream() {
    let client = test_http_client();
    let request = with_default_mcp_http_headers(client.get("https://example.invalid/mcp"), false)
        .build()
        .unwrap();
    assert_eq!(
        request.headers().get(ACCEPT).and_then(|v| v.to_str().ok()),
        Some(MCP_HTTP_ACCEPT)
    );
    assert!(
        request.headers().get(CONTENT_TYPE).is_none(),
        "SSE GET requests should not advertise a JSON request body"
    );
}

#[test]
fn default_mcp_http_post_accepts_json_and_event_stream() {
    let client = test_http_client();
    let request = with_default_mcp_http_headers(client.post("https://example.invalid/mcp"), true)
        .build()
        .unwrap();
    assert_eq!(
        request.headers().get(ACCEPT).and_then(|v| v.to_str().ok()),
        Some(MCP_HTTP_ACCEPT)
    );
    assert_eq!(
        request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
}

#[test]
fn streamable_http_transport_stores_headers() {
    let client = test_http_client();
    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), "Bearer xyz".to_string());
    let transport = StreamableHttpTransport::new(
        client,
        "https://example.invalid/mcp".to_string(),
        headers.clone(),
    );
    assert_eq!(transport.headers, headers);
}

#[test]
fn test_mcp_config_parse_mcp_servers_alias_and_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    fs::write(
        &path,
        r#"{
          "mcpServers": {
            "disabled": {
              "command": "node",
              "args": ["server.js"],
              "disabled": true
            }
          }
        }"#,
    )
    .unwrap();

    let cfg = load_config(&path).unwrap();
    assert!(cfg.servers.contains_key("disabled"));
    let snapshot = manager_snapshot_from_config(&path, true).unwrap();
    assert!(snapshot.restart_required);
    assert_eq!(snapshot.servers[0].name, "disabled");
    assert!(!snapshot.servers[0].enabled);
    assert_eq!(snapshot.servers[0].error.as_deref(), Some("disabled"));
}

#[test]
fn workspace_mcp_config_merges_with_project_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&project_dir).unwrap();
    let _trust = mark_workspace_trusted(&workspace);
    fs::write(
        &global_path,
        r#"{
          "servers": {
            "global": {"command": "node", "args": ["global.js"]},
            "shared": {"command": "node", "args": ["global-shared.js"]}
          }
        }"#,
    )
    .unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{
          "servers": {
            "project": {"command": "php", "args": ["artisan", "boost:mcp"]},
            "shared": {"command": "php", "args": ["artisan", "shared:mcp"]}
          }
        }"#,
    )
    .unwrap();

    let cfg = load_config_with_workspace(&global_path, &workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();

    assert!(cfg.servers.contains_key("global"));
    let project = cfg.servers.get("project").unwrap();
    assert_eq!(project.command.as_deref(), Some("php"));
    assert_eq!(project.cwd.as_deref(), Some(workspace.as_path()));
    let shared = cfg.servers.get("shared").unwrap();
    assert_eq!(shared.args, vec!["artisan", "shared:mcp"]);
    assert_eq!(shared.cwd.as_deref(), Some(workspace.as_path()));
}

#[test]
fn workspace_manager_snapshot_counts_global_and_project_servers() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&project_dir).unwrap();
    let _trust = mark_workspace_trusted(&workspace);
    fs::write(
        &global_path,
        r#"{
          "servers": {
            "chrome-devtools": {"command": "npx", "args": ["-y", "chrome-devtools-mcp@latest"]},
            "context7": {"command": "npx", "args": ["-y", "@upstash/context7-mcp@latest"]}
          }
        }"#,
    )
    .unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{
          "servers": {
            "laravel-boost": {"command": "php", "args": ["artisan", "boost:mcp"]}
          }
        }"#,
    )
    .unwrap();

    let plain = manager_snapshot_from_config(&global_path, false).unwrap();
    let merged =
        manager_snapshot_from_config_with_workspace(&global_path, &workspace, false).unwrap();

    assert_eq!(plain.servers.len(), 2);
    assert_eq!(merged.servers.len(), 3);
    assert!(
        merged
            .servers
            .iter()
            .any(|server| server.name == "laravel-boost"),
        "workspace-aware snapshots must include trusted project MCP servers"
    );
}

#[test]
fn workspace_mcp_config_ignores_project_file_until_workspace_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(
        &global_path,
        r#"{"servers": {"global": {"command": "node", "args": ["global.js"]}}}"#,
    )
    .unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{"servers": {"project": {"command": "php", "args": ["artisan", "boost:mcp"]}}}"#,
    )
    .unwrap();

    let cfg = load_config_with_workspace(&global_path, &workspace).unwrap();

    assert!(cfg.servers.contains_key("global"));
    assert!(!cfg.servers.contains_key("project"));
}

#[test]
fn workspace_mcp_config_ignores_project_local_legacy_trust_marker() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&project_dir).unwrap();
    fs::create_dir_all(workspace.join(".deepseek")).unwrap();
    fs::write(workspace.join(".deepseek").join("trusted"), "").unwrap();
    fs::write(
        &global_path,
        r#"{"servers": {"global": {"command": "node", "args": ["global.js"]}}}"#,
    )
    .unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{"servers": {"project": {"command": "php", "args": ["artisan", "boost:mcp"]}}}"#,
    )
    .unwrap();

    let cfg = load_config_with_workspace(&global_path, &workspace).unwrap();

    assert!(cfg.servers.contains_key("global"));
    assert!(!cfg.servers.contains_key("project"));
}

#[test]
fn workspace_mcp_config_ignores_invalid_untrusted_project_file() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(&global_path, r#"{"servers": {}}"#).unwrap();
    fs::write(project_dir.join("mcp.json"), "{ not json").unwrap();

    let cfg = load_config_with_workspace(&global_path, &workspace).unwrap();

    assert!(cfg.servers.is_empty());
}

#[test]
fn workspace_mcp_config_rejects_parent_components() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&project_dir).unwrap();
    let _trust = mark_workspace_trusted(&workspace);
    fs::write(&global_path, r#"{"servers": {}}"#).unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{"servers": {"project": {"command": "node", "args": ["server.js"]}}}"#,
    )
    .unwrap();

    let workspace_with_parent = workspace.join("..").join("workspace");
    let err = load_config_with_workspace(&global_path, &workspace_with_parent)
        .expect_err("parent components in workspace should fail closed");

    assert!(
        format!("{err:#}").contains("workspace path cannot contain '..'"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn workspace_mcp_config_resolves_relative_cwd_from_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&project_dir).unwrap();
    let _trust = mark_workspace_trusted(&workspace);
    fs::write(&global_path, r#"{"servers": {}}"#).unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{"servers": {"project": {"command": "node", "args": ["server.js"], "cwd": "tools/mcp"}}}"#,
    )
    .unwrap();

    let cfg = load_config_with_workspace(&global_path, &workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();

    let project = cfg.servers.get("project").unwrap();
    assert_eq!(
        project.cwd.as_deref(),
        Some(workspace.join("tools/mcp").as_path())
    );
}

#[test]
fn workspace_mcp_config_rejects_project_cwd_escape() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&project_dir).unwrap();
    let _trust = mark_workspace_trusted(&workspace);
    fs::write(&global_path, r#"{"servers": {}}"#).unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{"servers": {"project": {"command": "node", "args": ["server.js"], "cwd": "../outside"}}}"#,
    )
    .unwrap();

    let err = load_config_with_workspace(&global_path, &workspace)
        .expect_err("project MCP cwd escape must be rejected");

    assert!(
        err.to_string()
            .contains("Project MCP server cwd must stay within workspace"),
        "unexpected error: {err}"
    );
}

#[cfg(unix)]
#[test]
fn workspace_mcp_config_rejects_symlinked_project_cwd_escape() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&project_dir).unwrap();
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, workspace.join("tools")).unwrap();
    let _trust = mark_workspace_trusted(&workspace);
    fs::write(&global_path, r#"{"servers": {}}"#).unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{"servers": {"project": {"command": "node", "args": ["server.js"], "cwd": "tools"}}}"#,
    )
    .unwrap();

    let err = load_config_with_workspace(&global_path, &workspace)
        .expect_err("project MCP symlink cwd escape must be rejected");

    assert!(
        err.to_string()
            .contains("Project MCP server cwd must stay within workspace"),
        "unexpected error: {err}"
    );
}

#[test]
fn workspace_mcp_config_rejects_workspace_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let bad_workspace = workspace.join("..").join("outside");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(&global_path, r#"{"servers": {}}"#).unwrap();

    let err = load_config_with_workspace(&global_path, &bad_workspace)
        .expect_err("workspace traversal should fail");
    assert!(
        format!("{err:#}").contains("workspace path cannot contain '..'"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn workspace_mcp_pool_reload_picks_up_project_config_creation() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&workspace).unwrap();
    let _trust = mark_workspace_trusted(&workspace);
    fs::write(
        &global_path,
        r#"{"servers": {"global": {"command": "node", "args": ["global.js"]}}}"#,
    )
    .unwrap();

    let mut pool = McpPool::from_config_path_with_workspace(&global_path, &workspace).unwrap();
    assert_eq!(pool.server_names(), vec!["global"]);

    fs::create_dir_all(&project_dir).unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{"servers": {"project": {"command": "php", "args": ["artisan", "boost:mcp"]}}}"#,
    )
    .unwrap();

    assert!(pool.reload_if_config_changed().await.unwrap());
    let names: std::collections::BTreeSet<_> = pool.server_names().into_iter().collect();
    let expected: std::collections::BTreeSet<_> = ["global", "project"].into_iter().collect();
    assert_eq!(names, expected);
}

#[tokio::test]
async fn workspace_mcp_pool_reload_picks_up_project_config_after_workspace_trust() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&project_dir).unwrap();
    let trust_env = workspace_trust_config_guard(&workspace);
    fs::write(
        &global_path,
        r#"{"servers": {"global": {"command": "node", "args": ["global.js"]}}}"#,
    )
    .unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{"servers": {"project": {"command": "php", "args": ["artisan", "boost:mcp"]}}}"#,
    )
    .unwrap();

    let mut pool = McpPool::from_config_path_with_workspace(&global_path, &workspace).unwrap();
    assert_eq!(pool.server_names(), vec!["global"]);

    write_workspace_trust_config(&trust_env.config_path, &workspace);

    assert!(pool.reload_if_config_changed().await.unwrap());
    let names: std::collections::BTreeSet<_> = pool.server_names().into_iter().collect();
    let expected: std::collections::BTreeSet<_> = ["global", "project"].into_iter().collect();
    assert_eq!(names, expected);
}

#[tokio::test]
async fn workspace_mcp_pool_reload_drops_project_config_after_workspace_trust_removed() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&project_dir).unwrap();
    let trust = mark_workspace_trusted(&workspace);
    fs::write(
        &global_path,
        r#"{"servers": {"global": {"command": "node", "args": ["global.js"]}}}"#,
    )
    .unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{"servers": {"project": {"command": "php", "args": ["artisan", "boost:mcp"]}}}"#,
    )
    .unwrap();

    let mut pool = McpPool::from_config_path_with_workspace(&global_path, &workspace).unwrap();
    let names: std::collections::BTreeSet<_> = pool.server_names().into_iter().collect();
    let expected: std::collections::BTreeSet<_> = ["global", "project"].into_iter().collect();
    assert_eq!(names, expected);

    fs::remove_file(&trust.config_path).unwrap();

    assert!(pool.reload_if_config_changed().await.unwrap());
    assert_eq!(pool.server_names(), vec!["global"]);
}

#[tokio::test]
async fn workspace_mcp_pool_reload_drops_project_config_after_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let global_path = dir.path().join("global-mcp.json");
    let workspace = dir.path().join("workspace");
    let project_dir = workspace.join(".codewhale");
    fs::create_dir_all(&project_dir).unwrap();
    let _trust = mark_workspace_trusted(&workspace);
    fs::write(
        &global_path,
        r#"{"servers": {"global": {"command": "node", "args": ["global.js"]}}}"#,
    )
    .unwrap();
    let project_path = project_dir.join("mcp.json");
    fs::write(
        &project_path,
        r#"{"servers": {"project": {"command": "php", "args": ["artisan", "boost:mcp"]}}}"#,
    )
    .unwrap();

    let mut pool = McpPool::from_config_path_with_workspace(&global_path, &workspace).unwrap();
    let names: std::collections::BTreeSet<_> = pool.server_names().into_iter().collect();
    let expected: std::collections::BTreeSet<_> = ["global", "project"].into_iter().collect();
    assert_eq!(names, expected);

    fs::remove_file(project_path).unwrap();

    assert!(pool.reload_if_config_changed().await.unwrap());
    assert_eq!(pool.server_names(), vec!["global"]);
}

#[test]
fn test_mcp_config_rejects_traversal_path() {
    let err = load_config(Path::new("../mcp.json")).expect_err("traversal path should fail");
    assert!(
        format!("{err:#}").contains("cannot contain '..'"),
        "got: {err:#}"
    );
}

#[cfg(unix)]
#[test]
fn mcp_config_rejects_symlinked_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target-mcp.json");
    let link = dir.path().join("mcp.json");
    fs::write(&target, r#"{"servers": {}}"#).expect("write target config");
    std::os::unix::fs::symlink(&target, &link).expect("symlink mcp config");

    let err = load_config(&link).expect_err("symlinked MCP config should fail");

    assert!(format!("{err:#}").contains("regular file"), "got: {err:#}");
}

#[test]
fn init_mcp_config_rejects_traversal_before_parent_creation() {
    let dir = tempfile::tempdir().unwrap();
    let outside_dir = dir.path().join("outside");
    let path = dir
        .path()
        .join("allowed")
        .join("..")
        .join("outside")
        .join("mcp.json");

    let err = init_config(&path, false).expect_err("traversal path should fail");

    assert!(
        format!("{err:#}").contains("cannot contain '..'"),
        "got: {err:#}"
    );
    assert!(
        !outside_dir.exists(),
        "init_config must validate before creating parent directories"
    );
}

#[test]
fn test_mcp_config_manager_actions_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");

    assert_eq!(init_config(&path, false).unwrap(), McpWriteStatus::Created);
    assert_eq!(
        init_config(&path, false).unwrap(),
        McpWriteStatus::SkippedExists
    );

    add_server_config(
        &path,
        "local".to_string(),
        Some("node".to_string()),
        None,
        vec!["server.js".to_string()],
        None,
    )
    .unwrap();
    set_server_enabled(&path, "local", false).unwrap();
    let disabled = manager_snapshot_from_config(&path, true).unwrap();
    let local = disabled
        .servers
        .iter()
        .find(|server| server.name == "local")
        .unwrap();
    assert!(!local.enabled);
    assert_eq!(local.transport, "stdio");

    remove_server_config(&path, "local").unwrap();
    let removed = manager_snapshot_from_config(&path, true).unwrap();
    assert!(removed.servers.iter().all(|server| server.name != "local"));
}

#[test]
fn test_mcp_config_adds_explicit_sse_transport() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");

    add_server_config(
        &path,
        "legacy".to_string(),
        None,
        Some("https://example.com/v1/mcp/sse".to_string()),
        Vec::new(),
        Some("sse".to_string()),
    )
    .unwrap();

    let cfg = load_config(&path).unwrap();
    assert_eq!(
        cfg.servers
            .get("legacy")
            .and_then(|server| server.transport.as_deref()),
        Some("sse")
    );

    let snapshot = manager_snapshot_from_config(&path, false).unwrap();
    assert_eq!(snapshot.servers[0].transport, "sse");
}

#[test]
fn test_mcp_config_rejects_unknown_transport() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");

    let err = add_server_config(
        &path,
        "bad".to_string(),
        None,
        Some("https://example.com/mcp".to_string()),
        Vec::new(),
        Some("streamable".to_string()),
    )
    .expect_err("unknown transport should fail");

    assert!(
        format!("{err:#}").contains("Unsupported MCP transport"),
        "got: {err:#}"
    );
}

#[test]
fn test_server_effective_timeouts() {
    let global = McpTimeouts::default();

    let server_with_override = McpServerConfig {
        command: Some("test".to_string()),
        args: vec![],
        env: HashMap::new(),
        cwd: None,
        url: None,
        transport: None,
        connect_timeout: Some(20),
        execute_timeout: None,
        read_timeout: Some(180),
        disabled: false,
        enabled: true,
        required: false,
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        headers: HashMap::new(),
    };

    assert_eq!(server_with_override.effective_connect_timeout(&global), 20);
    assert_eq!(server_with_override.effective_execute_timeout(&global), 60); // global default
    assert_eq!(server_with_override.effective_read_timeout(&global), 180);
}

#[test]
fn test_mcp_pool_is_mcp_tool() {
    assert!(McpPool::is_mcp_tool("mcp_filesystem_read"));
    assert!(McpPool::is_mcp_tool("mcp_git_status"));
    assert!(McpPool::is_mcp_tool("list_mcp_resources"));
    assert!(McpPool::is_mcp_tool("list_mcp_resource_templates"));
    assert!(McpPool::is_mcp_tool("read_mcp_resource"));
    assert!(!McpPool::is_mcp_tool("read_file"));
    assert!(!McpPool::is_mcp_tool("exec_shell"));
}

#[test]
fn test_format_tool_result_text() {
    let result = serde_json::json!({
        "content": [
            {"type": "text", "text": "Hello, world!"}
        ]
    });
    assert_eq!(format_tool_result(&result), "Hello, world!");
}

#[test]
fn test_format_tool_result_error() {
    let result = serde_json::json!({
        "isError": true,
        "content": [
            {"type": "text", "text": "Something went wrong"}
        ]
    });
    assert_eq!(format_tool_result(&result), "Error: Something went wrong");
}

#[test]
fn test_format_tool_result_multiple_content() {
    let result = serde_json::json!({
        "content": [
            {"type": "text", "text": "Line 1"},
            {"type": "text", "text": "Line 2"},
            {"type": "image", "data": "base64..."}
        ]
    });
    let formatted = format_tool_result(&result);
    assert!(formatted.contains("Line 1"));
    assert!(formatted.contains("Line 2"));
    assert!(formatted.contains("[image content]"));
}

struct ScriptedValueTransport {
    sent: Arc<Mutex<Vec<serde_json::Value>>>,
    responses: VecDeque<Vec<u8>>,
}

#[async_trait::async_trait]
impl McpTransport for ScriptedValueTransport {
    async fn send(&mut self, msg: Vec<u8>) -> Result<()> {
        self.sent
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&msg)?);
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
        self.responses
            .pop_front()
            .context("scripted transport exhausted")
    }
}

struct HangingValueTransport {
    sent: Arc<Mutex<Vec<serde_json::Value>>>,
}

#[async_trait::async_trait]
impl McpTransport for HangingValueTransport {
    async fn send(&mut self, msg: Vec<u8>) -> Result<()> {
        self.sent
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&msg)?);
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
        std::future::pending().await
    }
}

struct DropCountingTransport {
    drops: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl McpTransport for DropCountingTransport {
    async fn send(&mut self, _msg: Vec<u8>) -> Result<()> {
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
        std::future::pending().await
    }
}

impl Drop for DropCountingTransport {
    fn drop(&mut self) {
        self.drops.fetch_add(1, AtomicOrdering::SeqCst);
    }
}

fn test_server_config() -> McpServerConfig {
    McpServerConfig {
        command: Some("mock".to_string()),
        args: Vec::new(),
        env: HashMap::new(),
        cwd: None,
        url: None,
        transport: None,
        connect_timeout: None,
        execute_timeout: None,
        read_timeout: None,
        disabled: false,
        enabled: true,
        required: false,
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        headers: HashMap::new(),
    }
}

fn test_connection(transport: Box<dyn McpTransport>) -> McpConnection {
    McpConnection {
        name: "mock".to_string(),
        transport,
        tools: Vec::new(),
        resources: Vec::new(),
        resource_templates: Vec::new(),
        prompts: Vec::new(),
        request_id: AtomicU64::new(1),
        state: ConnectionState::Ready,
        config: test_server_config(),
        read_timeout_secs: default_read_timeout(),
        cancel_token: tokio_util::sync::CancellationToken::new(),
    }
}

fn json_frame(value: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&value).unwrap()
}

#[tokio::test]
async fn call_method_skips_notifications_and_unmatched_responses() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let transport = ScriptedValueTransport {
        sent: Arc::clone(&sent),
        responses: VecDeque::from([
            json_frame(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {"progress": 0.5}
            })),
            json_frame(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 99,
                "result": {"ignored": true}
            })),
            json_frame(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"ok": true}
            })),
        ]),
    };
    let mut conn = test_connection(Box::new(transport));

    let result = conn
        .call_method("tools/call", serde_json::json!({"name": "echo"}), 1)
        .await
        .unwrap();

    assert_eq!(result, serde_json::json!({"ok": true}));
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["jsonrpc"], "2.0");
    assert_eq!(sent[0]["id"], "1");
    assert_eq!(sent[0]["method"], "tools/call");
}

#[tokio::test]
async fn call_method_invalid_json_includes_server_output_preview() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let transport = ScriptedValueTransport {
        sent: Arc::clone(&sent),
        responses: VecDeque::from([b"Allow Burp MCP connection? [y/N]".to_vec()]),
    };
    let mut conn = test_connection(Box::new(transport));

    let err = conn
        .call_method("tools/call", serde_json::json!({"name": "burp"}), 1)
        .await
        .expect_err("non-json MCP stdout should fail");
    let msg = err.to_string();

    assert!(msg.contains("Invalid MCP JSON-RPC message from server 'mock'"));
    assert!(msg.contains("Allow Burp MCP connection"));
    assert_eq!(conn.state(), ConnectionState::Disconnected);
}

#[tokio::test]
async fn recv_times_out_waiting_for_mcp_response_and_disconnects() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut conn = test_connection(Box::new(HangingValueTransport {
        sent: Arc::clone(&sent),
    }));
    conn.read_timeout_secs = 0;

    let err = conn
        .recv("1".to_string())
        .await
        .expect_err("hung transport should time out inside recv");

    assert!(
        err.to_string()
            .contains("Timed out waiting for MCP JSON-RPC response from server 'mock' after 0s"),
        "unexpected error: {err:#}"
    );
    assert_eq!(conn.state(), ConnectionState::Disconnected);
}

#[tokio::test]
async fn call_method_times_out_while_waiting_for_response() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut conn = test_connection(Box::new(HangingValueTransport {
        sent: Arc::clone(&sent),
    }));

    let err = conn
        .call_method("tools/call", serde_json::json!({"name": "echo"}), 0)
        .await
        .expect_err("hung receive should time out");

    assert!(
        err.to_string()
            .contains("MCP method 'tools/call' on server 'mock' timed out after 0s"),
        "unexpected error: {err:#}"
    );
    assert_eq!(sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_mcp_pool_empty_config() {
    let pool = McpPool::new(McpConfig::default());
    assert!(pool.server_names().is_empty());
    assert!(pool.all_tools().is_empty());
}

/// #1267 part 2: a pool built without a source path has no file to watch,
/// so `reload_if_config_changed` must short-circuit instead of trying
/// to stat `/`.
#[tokio::test]
async fn reload_if_config_changed_is_noop_without_source_path() {
    let mut pool = McpPool::new(McpConfig::default());
    let reloaded = pool.reload_if_config_changed().await.unwrap();
    assert!(!reloaded, "no source path → no reload");
}

/// #1267 part 2: when the on-disk config is byte-unchanged, the lazy
/// reload must not drop connections — every call to `get_or_connect`
/// would otherwise pay a full reconnect cycle on networked filesystems
/// where mtime granularity is coarse.
#[tokio::test]
async fn reload_if_config_changed_skips_when_content_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    std::fs::write(&path, r#"{"servers":{}}"#).unwrap();
    let mut pool = McpPool::from_config_path(&path).unwrap();
    // Force the mtime to advance without changing content.
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&path, r#"{"servers":{}}"#).unwrap();
    let reloaded = pool.reload_if_config_changed().await.unwrap();
    assert!(
        !reloaded,
        "content-unchanged config must not trigger a reload"
    );
}

/// #1267 part 2: when the on-disk config changes content, the next
/// `reload_if_config_changed` call must swap in the new config and
/// (would) drop all live connections. We can't stand up a real
/// `McpConnection` in a unit test, so we observe the swap via the
/// publicly-readable side: server names go from empty to non-empty.
#[tokio::test]
async fn reload_if_config_changed_swaps_config_on_content_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    std::fs::write(&path, r#"{"servers":{}}"#).unwrap();
    let mut pool = McpPool::from_config_path(&path).unwrap();
    assert!(pool.server_names().is_empty());
    // Mutate the file so both the mtime and the hash change.
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(
        &path,
        r#"{"servers":{"new":{"command":"echo","args":["hi"]}}}"#,
    )
    .unwrap();
    let reloaded = pool.reload_if_config_changed().await.unwrap();
    assert!(reloaded, "content-changed config must trigger reload");
    let names = pool.server_names();
    assert!(
        names.contains(&"new"),
        "expected new server in pool after reload, got {names:?}"
    );
}

#[tokio::test]
async fn reload_if_config_changed_drops_live_connections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    std::fs::write(
        &path,
        r#"{"servers":{"local":{"command":"node","args":["server.js"]}}}"#,
    )
    .unwrap();
    let mut pool = McpPool::from_config_path(&path).unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    let mut conn = test_connection(Box::new(DropCountingTransport {
        drops: Arc::clone(&drops),
    }));
    conn.name = "local".to_string();
    conn.config = pool.config.servers.get("local").unwrap().clone();
    pool.connections.insert("local".to_string(), conn);

    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(
        &path,
        r#"{"servers":{"local":{"command":"node","args":["server-v2.js"]}}}"#,
    )
    .unwrap();

    let reloaded = pool.reload_if_config_changed().await.unwrap();
    assert!(reloaded, "content-changed config must trigger reload");
    assert_eq!(
        drops.load(AtomicOrdering::SeqCst),
        1,
        "reload must drop the stale live transport"
    );
    assert!(
        !pool.connections.contains_key("local"),
        "stale connection must not survive config reload"
    );
    assert_eq!(
        pool.config.servers.get("local").unwrap().args,
        vec!["server-v2.js".to_string()]
    );
}

/// #1267 part 2: hash-based comparison must be stable for byte-identical
/// configs and distinct for differing configs.
#[test]
fn hash_mcp_config_is_stable_and_change_sensitive() {
    let a = McpConfig::default();
    let b = McpConfig::default();
    assert_eq!(hash_mcp_config(&a), hash_mcp_config(&b));
    let mut c = McpConfig::default();
    c.servers.insert(
        "x".into(),
        McpServerConfig {
            command: Some("/bin/echo".into()),
            args: vec!["hi".into()],
            env: Default::default(),
            cwd: None,
            url: None,
            transport: None,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: false,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: HashMap::new(),
        },
    );
    assert_ne!(
        hash_mcp_config(&a),
        hash_mcp_config(&c),
        "hash must change when servers map changes"
    );
}

/// #1319: discovered tools must be sorted by name so the prompt prefix
/// is stable across runs (cache-hit stability), even when the server
/// returns them in arbitrary or paginated order.
#[tokio::test]
async fn discover_tools_sorts_by_name_for_cache_stability() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let transport = ScriptedValueTransport {
        sent: Arc::clone(&sent),
        responses: VecDeque::from([
            json_frame(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "tools": [
                        { "name": "zeta", "inputSchema": {} },
                        { "name": "alpha", "inputSchema": {} }
                    ],
                    "nextCursor": "page-2"
                }
            })),
            json_frame(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "tools": [
                        { "name": "mu", "inputSchema": {} },
                        { "name": "beta", "inputSchema": {} }
                    ]
                }
            })),
        ]),
    };
    let mut conn = test_connection(Box::new(transport));
    conn.discover_tools().await.expect("discover");

    let names: Vec<&str> = conn.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["alpha", "beta", "mu", "zeta"],
        "tools must be sorted by name regardless of server order or pagination"
    );
}

#[tokio::test]
async fn mcp_pool_call_tool_preserves_tool_names_with_dashes() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let transport = ScriptedValueTransport {
        sent: Arc::clone(&sent),
        responses: VecDeque::from([json_frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"ok": true}
        }))]),
    };
    let mut conn = test_connection(Box::new(transport));
    conn.name = "dephy".to_string();
    conn.tools = vec![McpTool {
        name: "company--search".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    }];

    let mut pool = McpPool::new(McpConfig {
        timeouts: McpTimeouts::default(),
        servers: HashMap::new(),
    });
    pool.connections.insert("dephy".to_string(), conn);

    let result = pool
        .call_tool(
            "mcp_dephy_company--search",
            serde_json::json!({"query": "dephy"}),
        )
        .await
        .unwrap();

    assert_eq!(result, serde_json::json!({"ok": true}));
    let sent = sent.lock().unwrap();
    assert_eq!(sent[0]["method"], "tools/call");
    assert_eq!(sent[0]["params"]["name"], "company--search");
    assert_eq!(
        sent[0]["params"]["arguments"],
        serde_json::json!({"query": "dephy"})
    );
}

#[tokio::test]
async fn mcp_pool_call_tool_preserves_server_names_with_underscores() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let transport = ScriptedValueTransport {
        sent: Arc::clone(&sent),
        responses: VecDeque::from([json_frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"ok": true}
        }))]),
    };
    let mut conn = test_connection(Box::new(transport));
    conn.name = "my_db".to_string();
    conn.tools = vec![McpTool {
        name: "execute_sql".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    }];

    let mut pool = McpPool::new(McpConfig {
        timeouts: McpTimeouts::default(),
        servers: HashMap::new(),
    });
    pool.connections.insert("my_db".to_string(), conn);

    let result = pool
        .call_tool(
            "mcp_my_db_execute_sql",
            serde_json::json!({"query": "select 1"}),
        )
        .await
        .unwrap();

    assert_eq!(result, serde_json::json!({"ok": true}));
    let sent = sent.lock().unwrap();
    assert_eq!(sent[0]["method"], "tools/call");
    assert_eq!(sent[0]["params"]["name"], "execute_sql");
    assert_eq!(
        sent[0]["params"]["arguments"],
        serde_json::json!({"query": "select 1"})
    );
}

#[tokio::test]
async fn mcp_pool_call_tool_prefers_longest_matching_server_name() {
    let sent_short = Arc::new(Mutex::new(Vec::new()));
    let short_transport = ScriptedValueTransport {
        sent: Arc::clone(&sent_short),
        responses: VecDeque::from([json_frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"short": true}
        }))]),
    };
    let mut short_conn = test_connection(Box::new(short_transport));
    short_conn.name = "my".to_string();
    short_conn.tools = vec![McpTool {
        name: "db_execute_sql".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    }];

    let sent_long = Arc::new(Mutex::new(Vec::new()));
    let long_transport = ScriptedValueTransport {
        sent: Arc::clone(&sent_long),
        responses: VecDeque::from([json_frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"long": true}
        }))]),
    };
    let mut long_conn = test_connection(Box::new(long_transport));
    long_conn.name = "my_db".to_string();
    long_conn.tools = vec![McpTool {
        name: "execute_sql".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    }];

    let mut pool = McpPool::new(McpConfig {
        timeouts: McpTimeouts::default(),
        servers: HashMap::new(),
    });
    pool.connections.insert("my".to_string(), short_conn);
    pool.connections.insert("my_db".to_string(), long_conn);

    let result = pool
        .call_tool(
            "mcp_my_db_execute_sql",
            serde_json::json!({"query": "select 1"}),
        )
        .await
        .unwrap();

    assert_eq!(result, serde_json::json!({"long": true}));
    assert!(
        sent_short.lock().unwrap().is_empty(),
        "the shorter server name must not receive the tool call"
    );
    let sent_long = sent_long.lock().unwrap();
    assert_eq!(sent_long[0]["method"], "tools/call");
    assert_eq!(sent_long[0]["params"]["name"], "execute_sql");
    assert_eq!(
        sent_long[0]["params"]["arguments"],
        serde_json::json!({"query": "select 1"})
    );
}

#[tokio::test]
async fn json_rpc_session_error_is_marked_stale() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let transport = ScriptedValueTransport {
        sent: Arc::clone(&sent),
        responses: VecDeque::from([json_frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32001,
                "message": "MCP session expired"
            }
        }))]),
    };
    let mut conn = test_connection(Box::new(transport));

    let err = conn
        .call_tool("search", serde_json::json!({"query": "dephy"}), 1)
        .await
        .expect_err("session error should fail");

    assert!(
        is_mcp_stale_session_error(&err),
        "JSON-RPC session error should be retryable, got: {err:#}"
    );
}

#[test]
fn sse_transport_closed_is_retryable() {
    let err = anyhow::anyhow!("SSE transport closed");
    assert!(
        is_mcp_stale_session_error(&err),
        "closed SSE stream should force reconnect before retry"
    );
}

#[test]
fn legacy_sse_post_disconnect_is_retryable() {
    let err = anyhow::anyhow!(
        "MCP SSE POST send failed (transport=sse endpoint=http://127.0.0.1:123/messages): connection closed before message completed"
    );
    assert!(
        is_mcp_stale_session_error(&err),
        "closed legacy SSE POST should force reconnect before retry"
    );

    let err = anyhow::anyhow!(
        "MCP SSE POST send failed (transport=sse endpoint=http://127.0.0.1:123/messages): connection reset by peer"
    );
    assert!(
        is_mcp_stale_session_error(&err),
        "reset legacy SSE POST should force reconnect before retry"
    );

    let err = anyhow::anyhow!(
        "MCP SSE POST send failed (transport=sse endpoint=http://127.0.0.1:123/messages): An existing connection was forcibly closed by the remote host."
    );
    assert!(
        is_mcp_stale_session_error(&err),
        "Windows reset wording should force reconnect before retry"
    );
}

#[tokio::test]
async fn discover_all_ignores_unsupported_optional_capabilities() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let transport = ScriptedValueTransport {
        sent: Arc::clone(&sent),
        responses: VecDeque::from([
            json_frame(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "tools": [
                        { "name": "search", "inputSchema": {} }
                    ]
                }
            })),
            json_frame(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "error": {
                    "code": -32601,
                    "message": "resources not supported"
                }
            })),
            json_frame(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "error": {
                    "code": -32601,
                    "message": "resource templates not supported"
                }
            })),
            json_frame(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 4,
                "error": {
                    "code": -32601,
                    "message": "prompts not supported"
                }
            })),
        ]),
    };
    let mut conn = test_connection(Box::new(transport));

    conn.discover_all().await.expect("discover");

    assert_eq!(conn.tools.len(), 1);
    assert_eq!(conn.tools[0].name, "search");
    assert!(conn.resources.is_empty());
    assert!(conn.resource_templates.is_empty());
    assert!(conn.prompts.is_empty());
}

/// #1244: when an MCP stdio server fails to spawn, the underlying OS
/// error (e.g. ENOENT for a missing binary) must reach the user via the
/// snapshot.error string. Regression test for `err.to_string()` dropping
/// the anyhow chain — without `{err:#}` the user sees only the opaque
/// wrapper "MCP stdio spawn failed (...)" and has nothing to act on.
#[tokio::test]
async fn discover_snapshot_includes_underlying_spawn_error_in_chain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    fs::write(
        &path,
        r#"{
            "mcpServers": {
                "broken": {
                    "command": "codewhale-tui-test-this-binary-does-not-exist-9f8e7d6c5b4a",
                    "args": []
                }
            }
        }"#,
    )
    .unwrap();

    let snapshot = discover_manager_snapshot(&path, None, false).await.unwrap();
    let server = snapshot
        .servers
        .iter()
        .find(|s| s.name == "broken")
        .expect("broken server should appear in snapshot");
    let err = server
        .error
        .as_deref()
        .expect("broken server should have an error");
    let lowered = err.to_lowercase();
    assert!(
        lowered.contains("os error")
            || lowered.contains("not found")
            || lowered.contains("no such"),
        "expected underlying spawn error in chain, got: {err}"
    );
}

#[test]
fn parse_sse_message_data_extracts_message_events() {
    let body = "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\r\n\r\n";
    let messages = parse_sse_message_data(body);
    assert_eq!(messages.len(), 1);
    let value: serde_json::Value = serde_json::from_slice(&messages[0]).unwrap();
    assert_eq!(value["id"], 1);
    assert!(value.get("result").is_some());
}

#[test]
fn response_id_matches_string_and_numeric_echoes() {
    assert!(response_id_matches(Some(&serde_json::json!("1")), "1"));
    assert!(response_id_matches(Some(&serde_json::json!(1)), "1"));
    assert!(!response_id_matches(Some(&serde_json::json!("2")), "1"));
}

#[test]
fn legacy_sse_transport_requires_explicit_config() {
    let mut server = test_server_config();
    server.url = Some("https://example.com/mcp/abc/sse".to_string());

    assert!(
        !is_legacy_sse_transport(&server),
        "/sse paths must not force legacy SSE without an explicit transport override"
    );

    server.transport = Some("sse".to_string());
    assert!(is_legacy_sse_transport(&server));

    server.transport = Some("SSE".to_string());
    assert!(is_legacy_sse_transport(&server));

    server.transport = Some("http".to_string());
    assert!(!is_legacy_sse_transport(&server));
}

#[test]
fn find_sse_event_separator_accepts_lf_and_crlf() {
    assert_eq!(
        find_sse_event_separator("event: endpoint\n\n"),
        Some((15, 2))
    );
    assert_eq!(
        find_sse_event_separator("event: endpoint\r\n\r\n"),
        Some((15, 4))
    );
}

#[tokio::test]
#[ignore = "flaky: requires a live TCP listener and is sensitive to port allocation races"]
async fn mcp_connection_supports_streamable_http_event_stream_responses() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_request(socket: &mut TcpStream) -> String {
        let mut request = Vec::new();
        let mut buf = [0; 1024];
        let header_end = loop {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0, "client closed before headers completed");
            request.extend_from_slice(&buf[..n]);
            if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break pos + 4;
            }
        };

        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        let total_len = header_end + content_length;
        while request.len() < total_len {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0, "client closed before body completed");
            request.extend_from_slice(&buf[..n]);
        }

        String::from_utf8(request).unwrap()
    }

    async fn write_json_sse(socket: &mut TcpStream, response: serde_json::Value) {
        let body = format!("event: message\ndata: {response}\n\n");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    }

    let _lock = lock_mcp_loopback_tests().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let request = read_http_request(&mut socket).await;
                assert!(request.starts_with("POST /mcp "));
                assert!(
                    request.contains("Accept: application/json, text/event-stream")
                        || request.contains("accept: application/json, text/event-stream")
                );
                let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
                let value: serde_json::Value = serde_json::from_str(body).unwrap();
                let method = value["method"].as_str().unwrap();

                if method == "notifications/initialized" {
                    socket
                        .write_all(b"HTTP/1.1 202 Accepted\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                        .await
                        .unwrap();
                    return;
                }

                let id = value["id"].clone();
                let result = match method {
                    "initialize" => serde_json::json!({
                        "protocolVersion": "2024-11-05",
                        "serverInfo": {"name": "mock-streamable", "version": "1.0.0"},
                        "capabilities": {"tools": {}, "resources": {}, "prompts": {}}
                    }),
                    "tools/list" => serde_json::json!({
                        "tools": [{
                            "name": "read_wiki_structure",
                            "description": "Read wiki structure",
                            "inputSchema": {"type": "object"}
                        }]
                    }),
                    "resources/list" => serde_json::json!({"resources": []}),
                    "resources/templates/list" => {
                        serde_json::json!({"resourceTemplates": []})
                    }
                    "prompts/list" => serde_json::json!({"prompts": []}),
                    other => panic!("unexpected method: {other}"),
                };
                write_json_sse(
                    &mut socket,
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result
                    }),
                )
                .await;
            });
        }
    });

    let config = McpServerConfig {
        command: None,
        args: vec![],
        env: HashMap::new(),
        cwd: None,
        url: Some(format!("http://{addr}/mcp")),
        transport: None,
        connect_timeout: Some(2),
        execute_timeout: None,
        read_timeout: None,
        disabled: false,
        enabled: true,
        required: false,
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        headers: HashMap::new(),
    };

    let conn = McpConnection::connect_with_policy(
        "deepwiki".to_string(),
        config,
        &McpTimeouts::default(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(conn.state(), ConnectionState::Ready);
    assert_eq!(conn.tools().len(), 1);
    assert_eq!(conn.tools()[0].name, "read_wiki_structure");

    server.abort();
}

#[test]
fn mask_url_secrets_strips_userinfo() {
    let masked = mask_url_secrets("https://user:s3cret@host.example/api?foo=bar");
    assert!(masked.contains("***"), "expected masked userinfo: {masked}");
    assert!(!masked.contains("s3cret"), "secret leaked: {masked}");
    assert!(masked.contains("host.example"), "host preserved: {masked}");
}

#[test]
fn mask_url_secrets_passes_through_clean_url() {
    assert_eq!(
        mask_url_secrets("https://api.example.com/mcp"),
        "https://api.example.com/mcp"
    );
}

#[test]
fn redact_body_preview_masks_bearer_token() {
    let redacted = redact_body_preview("Authorization: Bearer abc.def.ghi end");
    assert!(redacted.contains("Bearer ***"), "redacted: {redacted}");
    assert!(!redacted.contains("abc.def.ghi"), "leaked: {redacted}");
}

#[test]
fn redact_proxy_userinfo_strips_password() {
    // Corporate-style proxy URL with embedded creds — the
    // password must never reach the on-disk log file. URL strings
    // are assembled from placeholder constants via `format!` so the
    // literal source never contains a scheme-prefixed username +
    // password pair (colon-separated, `@`-terminated) that
    // GitGuardian's "Basic Auth String" detector would flag as a
    // committed credential.
    let (placeholder_user, placeholder_pass) = ("PLACEHOLDER_USER", "PLACEHOLDER_PASS");
    let with_creds = format!("http://{placeholder_user}:{placeholder_pass}@proxy.example/");
    let redacted = redact_proxy_userinfo(&with_creds);
    assert_eq!(redacted, "http://***@proxy.example/");
    assert!(!redacted.contains(placeholder_pass));
    assert!(!redacted.contains(placeholder_user));

    // User only (no password) — still redacted.
    let with_user_only = format!("https://{placeholder_user}@proxy.example:8080");
    let redacted = redact_proxy_userinfo(&with_user_only);
    assert_eq!(redacted, "https://***@proxy.example:8080");

    // No userinfo segment — pass through.
    let redacted = redact_proxy_userinfo("http://proxy.example:3128/");
    assert_eq!(redacted, "http://proxy.example:3128/");

    // `@` appears only in the path, not as userinfo separator —
    // must not be mistaken for credentials.
    let redacted = redact_proxy_userinfo("http://proxy.example/path@thing");
    assert_eq!(redacted, "http://proxy.example/path@thing");

    // Garbage input (no `://`) returned unchanged — the
    // surrounding warning log is the only caller and is already
    // handling the malformed-URL case.
    assert_eq!(redact_proxy_userinfo("not-a-url"), "not-a-url");
}

#[test]
fn redact_body_preview_masks_api_key_param() {
    let redacted = redact_body_preview("error message api_key=sk-12345&other=val");
    assert!(redacted.contains("api_key=***"), "redacted: {redacted}");
    assert!(!redacted.contains("sk-12345"), "leaked: {redacted}");
    assert!(
        redacted.contains("other=val"),
        "non-secret preserved: {redacted}"
    );
}

#[test]
fn invalid_json_preview_collapses_lines_and_redacts_secrets() {
    let preview = invalid_json_preview(
        b"Authorization: Bearer PLACEHOLDER_TOKEN\nAllow connection? api_key=PLACEHOLDER_KEY",
    );

    assert!(
        preview.contains("Authorization: Bearer *** Allow connection? api_key=***"),
        "preview: {preview}"
    );
    assert!(
        !preview.contains('\n'),
        "preview should be single-line: {preview}"
    );
    assert!(
        !preview.contains("PLACEHOLDER_TOKEN") && !preview.contains("PLACEHOLDER_KEY"),
        "secret leaked: {preview}"
    );
}

/// #420: `StdioTransport::shutdown` reaps the child process by sending
/// SIGTERM and giving it a brief grace period before drop fires SIGKILL.
/// The test spawns `cat` (which exits immediately on stdin EOF / SIGTERM)
/// and verifies the transport tears down cleanly. Unix-only because
/// SIGTERM doesn't exist on Windows; on Windows the test would just
/// duplicate the kill_on_drop path.
#[cfg(unix)]
#[tokio::test]
async fn stdio_transport_shutdown_terminates_child() {
    use tokio::process::Command as TokioCommand;
    let mut cmd = TokioCommand::new("cat");
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd.spawn().expect("spawn cat");
    let pid = child.id().expect("child pid");
    let stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let mut transport = StdioTransport {
        child,
        stdin,
        reader: tokio::io::BufReader::new(stdout),
        stderr_tail: StderrTail::new(),
    };

    // shutdown() should send SIGTERM and complete within the grace window.
    let start = std::time::Instant::now();
    transport.shutdown().await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < STDIO_SHUTDOWN_GRACE + Duration::from_millis(500),
        "shutdown blocked beyond grace window: {elapsed:?}"
    );

    // The child should be reaped — kill(pid, 0) returning ESRCH means
    // the pid is gone. If it's still alive, kill(0) returns 0, which
    // means our shutdown didn't terminate it.
    // SAFETY: pid was just collected from a tokio Child we spawned.
    // libc::kill with signal 0 only checks pid existence and is
    // async-signal-safe.
    let still_alive = unsafe { libc::kill(pid as i32, 0) } == 0;
    assert!(
        !still_alive,
        "child {pid} survived StdioTransport::shutdown — SIGTERM not delivered"
    );
}

/// Mid-run MCP server crash: the v0.8.x spawn path used `Stdio::null` for
/// stderr, so a server that died with a useful stderr message left the
/// caller with only "Stdio transport closed". Now stderr is piped into a
/// bounded ring buffer and surfaced when the read side fails.
#[cfg(unix)]
#[tokio::test]
async fn stdio_transport_recv_error_includes_stderr_tail() {
    use tokio::process::Command as TokioCommand;

    let mut cmd = TokioCommand::new("sh");
    cmd.arg("-c")
        .arg("echo 'mcp-server: failed to load plugin' 1>&2; exit 1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().expect("spawn sh");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");

    let stderr_tail = StderrTail::new();
    {
        let tail = Arc::clone(&stderr_tail);
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tail.push(line).await;
            }
        });
    }

    let mut transport = StdioTransport {
        child,
        stdin,
        reader: tokio::io::BufReader::new(stdout),
        stderr_tail,
    };

    // Give the subprocess time to write its stderr line and exit.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let err = transport
        .recv()
        .await
        .expect_err("expected transport closed error");
    let err_str = format!("{err}");
    assert!(
        err_str.contains("Stdio transport closed"),
        "missing closed marker in: {err_str}"
    );
    assert!(
        err_str.contains("mcp-server: failed to load plugin"),
        "stderr context missing from error: {err_str}"
    );
}

#[tokio::test]
async fn sse_connect_waits_for_endpoint_before_first_send() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _lock = lock_mcp_loopback_tests().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let post_seen = Arc::new(AtomicBool::new(false));
    let server_post_seen = Arc::clone(&post_seen);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let server_cancel = cancel_token.clone();

    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let post_seen = Arc::clone(&server_post_seen);
            let server_cancel = server_cancel.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                loop {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                if request.starts_with("GET /sse ") {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                        .await
                        .unwrap();
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    socket
                        .write_all(b"event: endpoint\ndata: /messages\n\n")
                        .await
                        .unwrap();
                    server_cancel.cancelled().await;
                } else if request.starts_with("POST /messages ") {
                    post_seen.store(true, AtomicOrdering::SeqCst);
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                        )
                        .await
                        .unwrap();
                }
            });
        }
    });

    let client = test_http_client();
    let url = format!("http://{addr}/sse");
    let mut transport = SseTransport::connect(
        client,
        url,
        HashMap::new(),
        cancel_token.clone(),
        Duration::from_secs(2),
    )
    .await
    .unwrap();

    transport
        .send(json_frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize"
        })))
        .await
        .unwrap();

    assert!(
        post_seen.load(AtomicOrdering::SeqCst),
        "first SSE send should POST to the discovered endpoint"
    );

    cancel_token.cancel();
    server.abort();
}

#[tokio::test]
async fn sse_connect_accepts_crlf_endpoint_events() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _lock = lock_mcp_loopback_tests().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let post_seen = Arc::new(AtomicBool::new(false));
    let server_post_seen = Arc::clone(&post_seen);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let server_cancel = cancel_token.clone();

    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let post_seen = Arc::clone(&server_post_seen);
            let server_cancel = server_cancel.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                loop {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                if request.starts_with("GET /sse ") {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                        .await
                        .unwrap();
                    socket
                        .write_all(b"event: endpoint\r\ndata: /messages\r\n\r\n")
                        .await
                        .unwrap();
                    server_cancel.cancelled().await;
                } else if request.starts_with("POST /messages ") {
                    post_seen.store(true, AtomicOrdering::SeqCst);
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                        )
                        .await
                        .unwrap();
                }
            });
        }
    });

    let client = test_http_client();
    let url = format!("http://{addr}/sse");
    let mut transport = SseTransport::connect(
        client,
        url,
        HashMap::new(),
        cancel_token.clone(),
        Duration::from_secs(2),
    )
    .await
    .unwrap();

    transport
        .send(json_frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize"
        })))
        .await
        .unwrap();

    assert!(
        post_seen.load(AtomicOrdering::SeqCst),
        "first SSE send should POST to the CRLF-discovered endpoint"
    );

    cancel_token.cancel();
    server.abort();
}

#[tokio::test]
async fn sse_transport_applies_custom_headers_to_get_and_post() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _lock = lock_mcp_loopback_tests().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let get_header_seen = Arc::new(AtomicBool::new(false));
    let post_header_seen = Arc::new(AtomicBool::new(false));
    let server_get_header_seen = Arc::clone(&get_header_seen);
    let server_post_header_seen = Arc::clone(&post_header_seen);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let server_cancel = cancel_token.clone();

    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let get_header_seen = Arc::clone(&server_get_header_seen);
            let post_header_seen = Arc::clone(&server_post_header_seen);
            let server_cancel = server_cancel.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                loop {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let request_lower = request.to_lowercase();
                if request.starts_with("GET /sse ") {
                    if request_lower.contains("x-custom-auth: my-test-token") {
                        get_header_seen.store(true, AtomicOrdering::SeqCst);
                    }
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                        .await
                        .unwrap();
                    socket
                        .write_all(b"event: endpoint\ndata: /messages\n\n")
                        .await
                        .unwrap();
                    server_cancel.cancelled().await;
                } else if request.starts_with("POST /messages ") {
                    if request_lower.contains("x-custom-auth: my-test-token") {
                        post_header_seen.store(true, AtomicOrdering::SeqCst);
                    }
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                        )
                        .await
                        .unwrap();
                }
            });
        }
    });

    let client = test_http_client();
    let url = format!("http://{addr}/sse");
    let mut headers = HashMap::new();
    headers.insert("X-Custom-Auth".to_string(), "my-test-token".to_string());
    let mut transport = SseTransport::connect(
        client,
        url,
        headers,
        cancel_token.clone(),
        Duration::from_secs(2),
    )
    .await
    .unwrap();

    transport
        .send(json_frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize"
        })))
        .await
        .unwrap();

    assert!(
        get_header_seen.load(AtomicOrdering::SeqCst),
        "legacy SSE GET must include user-configured custom headers"
    );
    assert!(
        post_header_seen.load(AtomicOrdering::SeqCst),
        "legacy SSE POST must include user-configured custom headers"
    );

    cancel_token.cancel();
    server.abort();
}

#[tokio::test]
async fn sse_post_error_includes_response_body_excerpt() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _lock = lock_mcp_loopback_tests().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let server_cancel = cancel_token.clone();

    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let server_cancel = server_cancel.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                loop {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                if request.starts_with("GET /sse ") {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                        .await
                        .unwrap();
                    socket
                        .write_all(b"event: endpoint\ndata: /messages\n\n")
                        .await
                        .unwrap();
                    server_cancel.cancelled().await;
                } else if request.starts_with("POST /messages ") {
                    socket
                        .write_all(
                            b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: 25\r\n\r\n{\"error\":\"missing query\"}",
                        )
                        .await
                        .unwrap();
                }
            });
        }
    });

    let client = test_http_client();
    let url = format!("http://{addr}/sse");
    let mut transport = SseTransport::connect(
        client,
        url,
        HashMap::new(),
        cancel_token.clone(),
        Duration::from_secs(2),
    )
    .await
    .unwrap();

    let err = transport
        .send(json_frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize"
        })))
        .await
        .expect_err("POST rejection should be returned");
    let err = format!("{err:#}");
    assert!(
        err.contains("400 Bad Request") && err.contains("missing query"),
        "SSE POST error should include status and body, got: {err}"
    );

    cancel_token.cancel();
    server.abort();
}

#[tokio::test]
async fn streamable_http_stale_session_reconnects_and_retries_tool_call() {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn write_response(socket: &mut tokio::net::TcpStream, response: &[u8]) {
        socket.write_all(response).await.unwrap();
        socket.flush().await.unwrap();
        socket.shutdown().await.unwrap();
    }

    let _lock = lock_mcp_loopback_tests().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let get_count = Arc::new(AtomicUsize::new(0));
    let stale_seen = Arc::new(AtomicBool::new(false));
    let success_seen = Arc::new(AtomicBool::new(false));
    let server_get_count = Arc::clone(&get_count);
    let server_stale_seen = Arc::clone(&stale_seen);
    let server_success_seen = Arc::clone(&success_seen);

    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let get_count = Arc::clone(&server_get_count);
            let stale_seen = Arc::clone(&server_stale_seen);
            let success_seen = Arc::clone(&server_success_seen);
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0; 4096];
                let header_end = loop {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if let Some(pos) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&request[..header_end]).to_string();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                while request.len() < header_end + content_length {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                let body = &request[header_end..header_end + content_length];
                let session_header = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("mcp-session-id")
                        .then(|| value.trim().to_string())
                });

                if headers.starts_with("GET /mcp ") {
                    let count = get_count.fetch_add(1, AtomicOrdering::SeqCst);
                    let session = if count == 0 { "sess-old" } else { "sess-new" };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nConnection: close\r\nMcp-Session-Id: {session}\r\nContent-Length: 0\r\n\r\n"
                    );
                    write_response(&mut socket, response.as_bytes()).await;
                    return;
                }

                let request_json: serde_json::Value = serde_json::from_slice(body).unwrap();
                let method = request_json
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let id = request_json
                    .get("id")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!("0"));

                if method == "tools/call" && session_header.as_deref() == Some("sess-old") {
                    stale_seen.store(true, AtomicOrdering::SeqCst);
                    write_response(
                        &mut socket,
                        b"HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: 27\r\n\r\n{\"error\":\"session expired\"}",
                    )
                    .await;
                    return;
                }

                let result = match method {
                    "initialize" => serde_json::json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {}
                    }),
                    "tools/list" => serde_json::json!({
                        "tools": [
                            { "name": "search", "inputSchema": {} }
                        ]
                    }),
                    "resources/list" => serde_json::json!({ "resources": [] }),
                    "resources/templates/list" => {
                        serde_json::json!({ "resourceTemplates": [] })
                    }
                    "prompts/list" => serde_json::json!({ "prompts": [] }),
                    "tools/call" => {
                        assert_eq!(session_header.as_deref(), Some("sess-new"));
                        success_seen.store(true, AtomicOrdering::SeqCst);
                        serde_json::json!({ "content": [{ "type": "text", "text": "ok" }] })
                    }
                    _ => {
                        write_response(
                            &mut socket,
                            b"HTTP/1.1 202 Accepted\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                        )
                        .await;
                        return;
                    }
                };
                let response_body = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                write_response(&mut socket, response.as_bytes()).await;
            });
        }
    });

    let mut cfg = McpConfig::default();
    cfg.servers.insert(
        "dephy".to_string(),
        McpServerConfig {
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            url: Some(format!("http://{addr}/mcp")),
            transport: None,
            connect_timeout: Some(10),
            execute_timeout: Some(10),
            read_timeout: None,
            disabled: false,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: HashMap::new(),
        },
    );
    let mut pool = McpPool::new(cfg);

    let result = pool
        .call_tool("mcp_dephy_search", serde_json::json!({ "query": "dephy" }))
        .await
        .unwrap();

    assert_eq!(
        result,
        serde_json::json!({ "content": [{ "type": "text", "text": "ok" }] })
    );
    assert!(stale_seen.load(AtomicOrdering::SeqCst));
    assert!(success_seen.load(AtomicOrdering::SeqCst));
    assert_eq!(get_count.load(AtomicOrdering::SeqCst), 2);

    server.abort();
}

#[tokio::test]
async fn legacy_sse_session_expiry_is_marked_stale() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    let _lock = lock_mcp_loopback_tests().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        let header_end = loop {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                return;
            }
            request.extend_from_slice(&buf[..n]);
            if let Some(pos) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        assert!(headers.starts_with("POST /messages "));
        socket
            .write_all(
                b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: 27\r\n\r\n{\"error\":\"session expired\"}",
            )
            .await
            .unwrap();
    });

    let (_sender, receiver) = mpsc::unbounded_channel();
    let sse_task = tokio::spawn(async {});
    let mut transport = SseTransport {
        client: test_http_client(),
        base_url: format!("http://{addr}/sse"),
        headers: HashMap::new(),
        endpoint_url: Some(format!("http://{addr}/messages")),
        receiver,
        pending_messages: VecDeque::new(),
        sse_task,
    };

    let err = transport
        .send(br#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#.to_vec())
        .await
        .expect_err("expired SSE session should fail");

    assert!(
        is_mcp_stale_session_error(&err),
        "SSE session expiry should be retryable, got: {err:#}"
    );

    server.abort();
}

#[tokio::test]
async fn legacy_sse_closed_stream_reconnects_and_retries_tool_call() {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;

    async fn read_http_request(socket: &mut TcpStream) -> (String, serde_json::Value) {
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        let header_end = loop {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                return (String::new(), serde_json::Value::Null);
            }
            request.extend_from_slice(&buf[..n]);
            if let Some(pos) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]).to_string();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                return (headers, serde_json::Value::Null);
            }
            request.extend_from_slice(&buf[..n]);
        }
        let body = &request[header_end..header_end + content_length];
        let json = if body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(body).unwrap()
        };
        (headers, json)
    }

    let _lock = lock_mcp_loopback_tests().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let active_sse = Arc::new(Mutex::new(None::<mpsc::UnboundedSender<Option<String>>>));
    let get_count = Arc::new(AtomicUsize::new(0));
    let tool_call_count = Arc::new(AtomicUsize::new(0));
    let success_seen = Arc::new(AtomicBool::new(false));
    let server_active_sse = Arc::clone(&active_sse);
    let server_get_count = Arc::clone(&get_count);
    let server_tool_call_count = Arc::clone(&tool_call_count);
    let server_success_seen = Arc::clone(&success_seen);

    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let active_sse = Arc::clone(&server_active_sse);
            let get_count = Arc::clone(&server_get_count);
            let tool_call_count = Arc::clone(&server_tool_call_count);
            let success_seen = Arc::clone(&server_success_seen);
            tokio::spawn(async move {
                let (headers, request_json) = read_http_request(&mut socket).await;
                if headers.starts_with("GET /sse ") {
                    get_count.fetch_add(1, AtomicOrdering::SeqCst);
                    let (tx, mut rx) = mpsc::unbounded_channel::<Option<String>>();
                    *active_sse.lock().unwrap() = Some(tx);
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                        .await
                        .unwrap();
                    socket
                        .write_all(b"event: endpoint\ndata: /messages\n\n")
                        .await
                        .unwrap();
                    while let Some(message) = rx.recv().await {
                        let Some(message) = message else {
                            return;
                        };
                        let event = format!("event: message\ndata: {message}\n\n");
                        socket.write_all(event.as_bytes()).await.unwrap();
                    }
                    return;
                }

                if !headers.starts_with("POST /messages ") {
                    return;
                }

                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                    .await
                    .unwrap();

                let method = request_json
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if method == "notifications/initialized" {
                    return;
                }

                let id = request_json
                    .get("id")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!("0"));

                if method == "tools/call" {
                    let count = tool_call_count.fetch_add(1, AtomicOrdering::SeqCst);
                    if count == 0 {
                        if let Some(tx) = active_sse.lock().unwrap().take() {
                            let _ = tx.send(None);
                        }
                        return;
                    }
                }

                let result = match method {
                    "initialize" => serde_json::json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {}
                    }),
                    "tools/list" => serde_json::json!({
                        "tools": [
                            { "name": "search", "inputSchema": {} }
                        ]
                    }),
                    "resources/list" => serde_json::json!({ "resources": [] }),
                    "resources/templates/list" => {
                        serde_json::json!({ "resourceTemplates": [] })
                    }
                    "prompts/list" => serde_json::json!({ "prompts": [] }),
                    "tools/call" => {
                        success_seen.store(true, AtomicOrdering::SeqCst);
                        serde_json::json!({ "content": [{ "type": "text", "text": "ok" }] })
                    }
                    other => panic!("unexpected method: {other}"),
                };
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result
                })
                .to_string();
                // Deliver the response over the *current* SSE channel. The
                // retry tool call can race ahead of the reconnecting GET
                // /sse that re-stores the sender; under parallel load those
                // two server tasks are scheduled in either order, so wait
                // briefly for the channel instead of dropping the response
                // (which left the client hanging until timeout) (#2597).
                let send_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let tx = loop {
                    if let Some(tx) = active_sse.lock().unwrap().as_ref().cloned() {
                        break Some(tx);
                    }
                    if std::time::Instant::now() >= send_deadline {
                        break None;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                };
                if let Some(tx) = tx {
                    let _ = tx.send(Some(response));
                }
            });
        }
    });

    let mut cfg = McpConfig::default();
    cfg.servers.insert(
        "dephy".to_string(),
        McpServerConfig {
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            url: Some(format!("http://{addr}/sse")),
            transport: Some("sse".to_string()),
            connect_timeout: Some(10),
            execute_timeout: Some(10),
            read_timeout: None,
            disabled: false,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: HashMap::new(),
        },
    );
    let mut pool = McpPool::new(cfg);

    let result = pool
        .call_tool("mcp_dephy_search", serde_json::json!({ "query": "dephy" }))
        .await
        .unwrap();

    assert_eq!(
        result,
        serde_json::json!({ "content": [{ "type": "text", "text": "ok" }] })
    );
    assert_eq!(tool_call_count.load(AtomicOrdering::SeqCst), 2);
    assert_eq!(get_count.load(AtomicOrdering::SeqCst), 2);
    assert!(success_seen.load(AtomicOrdering::SeqCst));

    server.abort();
}

#[test]
fn session_id_starts_none() {
    let transport = StreamableHttpTransport::new(
        test_http_client(),
        "https://example.invalid/mcp".to_string(),
        HashMap::new(),
    );
    assert!(transport.session_id.is_none());
}

/// Session ID captured from a POST response is replayed on the next POST.
#[tokio::test]
async fn session_id_captured_from_post_response_and_replayed() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _lock = lock_mcp_loopback_tests().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let n = socket.read(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]);
        assert!(req.starts_with("POST "), "expected POST, got: {req}");

        // First POST: return a session ID so the transport captures it.
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: sess-abc-123\r\nContent-Length: 2\r\n\r\n{}",
            )
            .await
            .unwrap();
        socket.flush().await.unwrap();

        // Read the second POST — should contain the session ID.
        let mut buf2 = [0u8; 4096];
        let n2 = socket.read(&mut buf2).await.unwrap();
        let req2 = String::from_utf8_lossy(&buf2[..n2]);
        // reqwest lower-cases header names.
        let req2_lower = req2.to_lowercase();
        assert!(
            req2_lower.contains("mcp-session-id: sess-abc-123"),
            "second POST must replay captured session ID, got:\n{req2}"
        );

        socket
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    });

    let client = test_http_client();
    let url = format!("http://{addr}/mcp");
    let mut transport = StreamableHttpTransport::new(client, url, HashMap::new());

    // First send: server returns Mcp-Session-Id.
    transport
        .send(json_frame(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "initialize",
            "params": {}
        })))
        .await
        .unwrap();
    assert_eq!(
        transport.session_id.as_deref(),
        Some("sess-abc-123"),
        "session ID should be captured from response"
    );

    // Second send: should replay the session ID.
    transport
        .send(json_frame(serde_json::json!({
            "jsonrpc": "2.0", "id": 2,
            "method": "tools/list",
            "params": {}
        })))
        .await
        .unwrap();

    server.abort();
}

/// Custom headers configured in McpServerConfig are applied to the GET
/// preflight so servers that require auth on session-establishment GET
/// (e.g. Hindsight, #1629) can authenticate it.
#[tokio::test]
async fn custom_headers_applied_to_get_preflight() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _lock = lock_mcp_loopback_tests().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // The test signals success by writing to this flag — the GET handler
    // sets it when it sees the expected header.
    let header_seen = Arc::new(AtomicBool::new(false));
    let header_seen_srv = Arc::clone(&header_seen);

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let n = socket.read(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]);

        // reqwest lower-cases header names.
        if req.starts_with("GET ") && req.to_lowercase().contains("x-custom-auth: my-test-token") {
            header_seen_srv.store(true, AtomicOrdering::SeqCst);
        }

        socket
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    });

    let client = test_http_client();
    let url = format!("http://{addr}/mcp");
    let mut headers = HashMap::new();
    headers.insert("X-Custom-Auth".to_string(), "my-test-token".to_string());

    let mut transport = HttpTransport::new(
        client,
        url,
        headers,
        tokio_util::sync::CancellationToken::new(),
        Duration::from_secs(10),
    );

    transport.try_establish_session().await.unwrap();

    server.abort();

    assert!(
        header_seen.load(AtomicOrdering::SeqCst),
        "GET preflight must include user-configured custom headers"
    );
}
