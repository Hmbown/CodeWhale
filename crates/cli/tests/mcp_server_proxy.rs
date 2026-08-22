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
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{},"resources":{}},"serverInfo":{"name":"fake-mcp","version":"0"}}}\n' "$id"
      ;;
    tools/list)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"whoami","description":"report the spawned process","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    tools/call)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"spawned-child"}]}}\n' "$id"
      ;;
    resources/list)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"resources":[{"uri":"file:///fake/readme.txt","name":"Fake readme","description":"resource from the spawned process","mimeType":"text/plain","size":16,"annotations":{"audience":["assistant"],"priority":0.75}}]}}\n' "$id"
      ;;
    resources/read)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"contents":[{"uri":"file:///fake/readme.txt","mimeType":"text/plain","text":"spawned-resource"}]}}\n' "$id"
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
fn mcp_server_enforces_jsonrpc_identity_and_initialize_lifecycle() {
    let fixture = Fixture::new();
    let (responses, stderr) = fixture.run_mcp_server(&[
        json!({"id": 1, "method": "ping"}),
        json!({"jsonrpc": "2.0", "id": null, "method": "ping"}),
        json!({"jsonrpc": "2", "id": 2, "method": "ping"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": {"name": "lifecycle-test", "version": "1"},
                "capabilities": {}
            }
        }),
        json!({"jsonrpc": "2.0", "id": 5, "method": "resources/list"}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "method": "ping"}),
        json!({"jsonrpc": "2.0", "id": 6, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 7, "method": "shutdown"}),
    ]);

    let null_id_responses: Vec<&Value> = responses
        .iter()
        .filter(|response| response["id"].is_null())
        .collect();
    assert_eq!(
        null_id_responses.len(),
        2,
        "missing id must be a notification while explicit null receives a response: {responses:?}"
    );
    assert_eq!(null_id_responses[0]["error"]["code"], -32600);
    assert!(null_id_responses[1]["result"].is_object());

    assert_eq!(response_for(&responses, 2)["error"]["code"], -32600);
    assert_eq!(response_for(&responses, 3)["error"]["code"], -32600);
    assert!(
        response_for(&responses, 3)["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("completed initialize"))
    );
    assert_eq!(
        response_for(&responses, 4)["result"]["protocolVersion"],
        "2024-11-05"
    );
    assert_eq!(response_for(&responses, 5)["error"]["code"], -32600);
    assert_eq!(response_for(&responses, 6)["result"]["tools"], json!([]));
    assert!(
        stderr.contains("codewhale mcp-server: stdio server exited"),
        "missing clean shutdown receipt:\n{stderr}"
    );
}

#[test]
fn mcp_server_proxies_tools_and_resources_from_the_configured_child_process() {
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
        json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": {"name": "proxy-test", "version": "1"},
                "capabilities": {}
            }
        }),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "mcp__fake__whoami", "arguments": {}}
        }),
        json!({"jsonrpc": "2.0", "id": 3, "method": "resources/list"}),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "resources/read",
            "params": {"uri": "file:///fake/readme.txt"}
        }),
        json!({"jsonrpc": "2.0", "id": 5, "method": "shutdown"}),
    ]);

    let initialize = response_for(&responses, 0);
    assert_eq!(initialize["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(
        initialize["result"]["serverInfo"]["name"],
        "codewhale-mcp-server"
    );
    assert!(initialize["result"]["capabilities"]["tools"].is_object());
    assert!(initialize["result"]["capabilities"]["resources"].is_object());

    let tools = response_for(&responses, 1)["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returned no array; stderr:\n{stderr}"))
        .clone();
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["mcp__fake__whoami"],
        "only the child's real tools may be exposed; the stub's fabricated \
         `health`/`capabilities` must be gone. stderr:\n{stderr}"
    );
    assert_eq!(tools[0]["tool_name"], "whoami");
    assert!(tools[0]["inputSchema"].is_object());

    let call = response_for(&responses, 2);
    assert_eq!(
        call["result"]["content"][0]["text"], "spawned-child",
        "the standard MCP result must come from the spawned process: {call}"
    );
    assert_eq!(
        call["result"]["result"]["content"][0]["text"], "spawned-child",
        "the legacy nested result must remain available: {call}"
    );

    let resources = response_for(&responses, 3)["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("resources/list returned no array; stderr:\n{stderr}"));
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["uri"], "file:///fake/readme.txt");
    assert_eq!(resources[0]["name"], "Fake readme");
    assert_eq!(resources[0]["mimeType"], "text/plain");
    assert_eq!(resources[0]["size"], 16);
    assert_eq!(
        resources[0]["annotations"]["audience"],
        json!(["assistant"])
    );
    assert_eq!(resources[0]["annotations"]["priority"], 0.75);
    assert_eq!(resources[0]["server_name"], "fake");

    let read = response_for(&responses, 4);
    assert_eq!(read["result"]["contents"][0]["text"], "spawned-resource");
    assert_eq!(
        read["result"]["resource"]["contents"][0]["text"], "spawned-resource",
        "the legacy nested resource must remain available: {read}"
    );
    assert!(
        !stderr.contains("deepseek-mcp"),
        "stale identity in stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("codewhale mcp-server: stdio server exited"),
        "missing Codewhale shutdown identity in stderr:\n{stderr}"
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
            "id": 10,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": {"name": "failure-test", "version": "1"},
                "capabilities": {}
            }
        }),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
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
