//! `codewhale mcp-server` must proxy to the user's configured servers.
//!
//! Regression coverage for #4727, where every configured server was wired to
//! an in-process stub: `command`/`args`/`env` were never executed, `health`
//! and `capabilities` answered `{"status": "ok"}` from a hardcoded literal,
//! and every real tool came back "not found". A client had no way to tell a
//! working integration from a fabricated one, which is why these tests assert
//! on the *origin* of the answer, not merely that an answer arrived.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

/// A minimal MCP server in POSIX sh, so the test depends on nothing beyond the
/// shell already present on every unix runner.
const FAKE_SERVER: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  if [ -z "$id" ]; then
    continue
  fi
  case "$method" in
    initialize)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake-mcp","version":"0"}}}\n' "$id"
      ;;
    tools/list)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"whoami","description":"report the spawned process"}]}}\n' "$id"
      ;;
    tools/call)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"spawned-child"}]}}\n' "$id"
      ;;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"unsupported method"}}\n' "$id"
      ;;
  esac
done
"#;

struct Fixture {
    _root: TempDir,
    home: PathBuf,
}

impl Fixture {
    /// Seal HOME before anything writes config. The suite has written to the
    /// real `~/.codewhale/config.toml` before (#4831); this test must never be
    /// the one that does it again.
    fn new() -> Self {
        let root = TempDir::new().expect("fixture root");
        let home = root.path().join("sealed-home");
        fs::create_dir_all(home.join(".codewhale")).expect("sealed config dir");
        fs::write(home.join(".codewhale").join("config.toml"), "").expect("seed config");
        Self { _root: root, home }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(codewhale_binary());
        command
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEWHALE_HOME", self.home.join(".codewhale"))
            .env("CODEWHALE_SECRET_BACKEND", "file");
        command
    }

    fn write_fake_server(&self) -> PathBuf {
        let script = self.home.join("fake-mcp-server.sh");
        fs::write(&script, FAKE_SERVER).expect("write fake MCP server");
        script
    }

    fn configure_servers(&self, definitions: Value) {
        let output = self
            .command()
            .args(["config", "set", "mcp.server_definitions"])
            .arg(definitions.to_string())
            .output()
            .expect("run config set");
        assert!(
            output.status.success(),
            "config set failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Drive `codewhale mcp-server` over stdio with `requests`, returning the
    /// parsed JSON-RPC responses plus stderr.
    fn run_mcp_server(&self, requests: &[Value]) -> (Vec<Value>, String) {
        let mut child = self
            .command()
            .arg("mcp-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn codewhale mcp-server");

        {
            let stdin = child.stdin.as_mut().expect("mcp-server stdin");
            for request in requests {
                writeln!(stdin, "{request}").expect("write request");
            }
        }

        let output = child.wait_with_output().expect("mcp-server output");
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let responses = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect();
        (responses, stderr)
    }
}

fn codewhale_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_codewhale") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_codewhale") {
        return PathBuf::from(path);
    }
    let mut path = std::env::current_exe().expect("current test executable path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("codewhale")
}

fn response_for(responses: &[Value], id: i64) -> &Value {
    responses
        .iter()
        .find(|response| response["id"] == json!(id))
        .unwrap_or_else(|| panic!("no response with id {id} in {responses:?}"))
}

#[test]
fn mcp_server_proxies_tools_from_the_configured_child_process() {
    let fixture = Fixture::new();
    let script = fixture.write_fake_server();
    fixture.configure_servers(json!([{
        "config": {
            "name": "fake",
            "command": "/bin/sh",
            "args": [script.to_str().expect("utf-8 script path")],
        }
    }]));

    let (responses, stderr) = fixture.run_mcp_server(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "mcp__fake__whoami", "arguments": {}}
        }),
        json!({"jsonrpc": "2.0", "id": 3, "method": "shutdown"}),
    ]);

    let tools = response_for(&responses, 1)["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returned no array; stderr:\n{stderr}"))
        .clone();
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["tool_name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["whoami"],
        "only the child's real tools may be exposed; the stub's fabricated \
         `health`/`capabilities` must be gone. stderr:\n{stderr}"
    );

    let call = response_for(&responses, 2);
    assert_eq!(
        call["result"]["result"]["content"][0]["text"], "spawned-child",
        "the tool result must come from the spawned process: {call}"
    );
}

#[test]
fn mcp_server_reports_a_server_it_could_not_spawn() {
    let fixture = Fixture::new();
    fixture.configure_servers(json!([{
        "config": {
            "name": "broken",
            "command": "codewhale-nonexistent-mcp-server-binary",
        }
    }]));

    let (responses, stderr) = fixture.run_mcp_server(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "server/list"}),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "mcp__broken__health", "arguments": {}}
        }),
        json!({"jsonrpc": "2.0", "id": 3, "method": "shutdown"}),
    ]);

    let server = response_for(&responses, 1)["result"]["lifecycle"]["servers"][0].clone();
    assert_eq!(
        server["running"],
        json!(false),
        "an unspawnable server must not report as running: {server}"
    );
    assert!(
        server["error"]
            .as_str()
            .is_some_and(|error| error.contains("failed to spawn command")),
        "the lifecycle must carry the spawn failure: {server}"
    );
    assert!(
        stderr.contains("is not available"),
        "the failure must also be loud on stderr, got:\n{stderr}"
    );

    let call = response_for(&responses, 2);
    assert!(
        call["error"].is_object(),
        "a dead server must return an error, never a fabricated success: {call}"
    );
}
