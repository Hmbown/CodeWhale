//! Process-level acceptance for adaptive exact-evidence routing (#4619).
//!
//! "Exact" begins at the common engine routing seam: tool adapters such as
//! Bash intentionally bound their own operating-system stream and annotate
//! that truncation before returning a `ToolResult`. Adaptive evidence binds
//! every byte of that returned result. Root streaming, sequential/deferred
//! completion, and MCP all converge on the same engine seam; sub-agents have a
//! separate call site covered by `tools::subagent::tests`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use wait_timeout::ChildExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const MODEL: &str = "adaptive-evidence-test";
const SUCCESS_CALL_ID: &str = "call_bash_success";
const FAILURE_CALL_ID: &str = "call_bash_failure";
const SUCCESS_SENTINEL: &str = "DEEP_SUCCESS_EVIDENCE_SENTINEL_4619";
const FAILURE_SENTINEL: &str = "DEEP_FAILURE_EVIDENCE_SENTINEL_4619";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_bash_success_and_failure_are_distinct_bounded_exact_evidence() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");

    let server = mock_llm().await;
    let output = run_exec(workspace.path(), home.path(), &server);
    assert!(
        output.status.success(),
        "exec failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = server.received_requests().await.expect("recorded requests");
    let success_receipt = requests
        .iter()
        .filter_map(|request| request.body_json::<Value>().ok())
        .find_map(|body| tool_result_content_for(&body, SUCCESS_CALL_ID).map(str::to_owned))
        .expect("model-visible Bash success receipt");
    let failure_receipt = requests
        .iter()
        .filter_map(|request| request.body_json::<Value>().ok())
        .find_map(|body| tool_result_content_for(&body, FAILURE_CALL_ID).map(str::to_owned))
        .expect("model-visible Bash failure receipt");
    for (receipt, call_id, sentinel) in [
        (&success_receipt, SUCCESS_CALL_ID, SUCCESS_SENTINEL),
        (&failure_receipt, FAILURE_CALL_ID, FAILURE_SENTINEL),
    ] {
        assert!(receipt.starts_with("[Exact evidence retained"));
        assert!(receipt.contains(&format!("retrieve_tool_result ref=art_{call_id}")));
        assert!(!receipt.contains(sentinel));
        assert!(!receipt.contains("/artifacts/"));
        assert!(
            receipt.len() <= 3_200,
            "handle-only receipt must stay bounded"
        );
    }
    assert_ne!(success_receipt, failure_receipt);

    let artifact_dir = find_artifact_dir(home.path()).expect("origin-session artifacts");
    let payloads = std::fs::read_dir(&artifact_dir)
        .expect("artifact directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("txt"))
        .count();
    assert_eq!(payloads, 2, "exactly one evidence payload per result");

    let success = assert_exact_artifact(&artifact_dir, SUCCESS_CALL_ID, SUCCESS_SENTINEL, "Bash");
    let failure = assert_exact_artifact(&artifact_dir, FAILURE_CALL_ID, FAILURE_SENTINEL, "Bash");
    assert_ne!(
        success, failure,
        "success and failure bytes must stay distinct"
    );
}

fn assert_exact_artifact(
    artifact_dir: &Path,
    call_id: &str,
    sentinel: &str,
    tool_name: &str,
) -> Vec<u8> {
    let handle = format!("art_{call_id}");
    let exact =
        std::fs::read(artifact_dir.join(format!("{handle}.txt"))).expect("exact evidence bytes");
    assert!(
        String::from_utf8_lossy(&exact).contains(sentinel),
        "deep content omitted from context must remain retrievable"
    );
    let metadata: Value = serde_json::from_slice(
        &std::fs::read(artifact_dir.join(format!("{handle}.evidence.json")))
            .expect("evidence metadata"),
    )
    .expect("valid evidence metadata");
    let digest = Sha256::digest(&exact)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(metadata["handle"], handle);
    assert_eq!(metadata["call_id"], call_id);
    assert_eq!(metadata["tool_name"], tool_name);
    assert_eq!(metadata["digest"], digest);
    assert_eq!(metadata["size_bytes"], exact.len() as u64);
    assert_eq!(metadata["generation"], 1);
    assert_eq!(metadata["redacted"], false);
    assert_eq!(metadata["encoding"], "utf-8");
    assert_eq!(metadata["retention_state"], "live");
    assert!(
        metadata["origin_session"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    exact
}

async fn mock_llm() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(json_response(json!({
            "object": "list",
            "data": [{"id": MODEL, "object": "model"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(EvidenceScenario {
            requests: Arc::new(AtomicUsize::new(0)),
        })
        .mount(&server)
        .await;
    server
}

#[derive(Clone)]
struct EvidenceScenario {
    requests: Arc<AtomicUsize>,
}

impl Respond for EvidenceScenario {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let sequence = self.requests.fetch_add(1, Ordering::SeqCst);
        let body = request.body_json::<Value>().unwrap_or(Value::Null);
        let raw = body.to_string();
        let response = if raw.contains(FAILURE_CALL_ID) {
            final_sse()
        } else if raw.contains(SUCCESS_CALL_ID) {
            bash_tool_sse(FAILURE_CALL_ID, false)
        } else {
            bash_tool_sse(SUCCESS_CALL_ID, true)
        };
        assert!(
            sequence < 3,
            "unexpected extra model request #{sequence}: {raw}"
        );
        sse_response(response)
    }
}

fn run_exec(workspace: &Path, home: &Path, server: &MockServer) -> std::process::Output {
    std::fs::create_dir_all(home.join(".codewhale")).expect("config directory");
    std::fs::create_dir_all(home.join(".deepseek")).expect("legacy config directory");
    std::fs::write(
        home.join(".codewhale/config.toml"),
        "allow_shell = true\n\n[retry]\nenabled = false\n",
    )
    .expect("headless test config");
    let mut command = Command::new(binary());
    preserve_host_env(&mut command);
    command
        .current_dir(workspace)
        .args(["--workspace", workspace.to_str().expect("workspace utf8")])
        .arg("--no-project-config")
        .args([
            "exec",
            "--auto",
            "--model",
            MODEL,
            "--output-format",
            "stream-json",
        ])
        .arg("run both provider-fixtured Bash evidence probes")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("CODEWHALE_CONFIG_PATH", home.join(".codewhale/config.toml"))
        .env("DEEPSEEK_CONFIG_PATH", home.join(".deepseek/config.toml"))
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("CODEWHALE_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", MODEL)
        .env("CODEWHALE_MODEL", MODEL)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_with_timeout(command, Duration::from_secs(45))
}

fn find_artifact_dir(home: &Path) -> Option<PathBuf> {
    let sessions = home.join(".codewhale/sessions");
    std::fs::read_dir(sessions)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let path = entry.path().join("artifacts");
            path.is_dir().then_some(path)
        })
}

fn tool_result_content_for<'a>(body: &'a Value, call_id: &str) -> Option<&'a str> {
    body.get("messages")?
        .as_array()?
        .iter()
        .find(|message| {
            message.get("role").and_then(Value::as_str) == Some("tool")
                && message.get("tool_call_id").and_then(Value::as_str) == Some(call_id)
        })?
        .get("content")?
        .as_str()
}

fn bash_tool_sse(call_id: &str, success: bool) -> String {
    let (sentinel, prefix, trailer) = if success {
        (SUCCESS_SENTINEL, "BASH-SUCCESS", "")
    } else {
        (FAILURE_SENTINEL, "BASH-FAILURE", "; exit 7")
    };
    let body = format!(
        "i=0; while [ \"$i\" -lt 2800 ]; do if [ \"$i\" -eq 120 ]; then printf '%s\\n' '{sentinel}'; fi; printf '{prefix}-%04d-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n' \"$i\"; i=$((i + 1)); done{trailer}"
    );
    let command = if success {
        body
    } else {
        format!("{{ {body}; }} >&2")
    };
    let arguments = serde_json::to_string(&json!({
        "action": "run",
        "command": command,
        "timeout_ms": 30_000
    }))
    .expect("tool arguments");
    [
        chunk(json!({"id":"tool","object":"chat.completion.chunk","model":MODEL,"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":call_id,"type":"function","function":{"name":"Bash","arguments":arguments}}]},"finish_reason":null}]})),
        chunk(json!({"id":"tool","object":"chat.completion.chunk","model":MODEL,"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}})),
        "data: [DONE]\n\n".to_string(),
    ].join("")
}

fn final_sse() -> String {
    [
        chunk(json!({"id":"final","object":"chat.completion.chunk","model":MODEL,"choices":[{"index":0,"delta":{"content":"evidence retained"},"finish_reason":null}]})),
        chunk(json!({"id":"final","object":"chat.completion.chunk","model":MODEL,"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":20,"completion_tokens":2,"total_tokens":22}})),
        "data: [DONE]\n\n".to_string(),
    ].join("")
}

fn chunk(value: Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::to_string(&value).expect("SSE JSON")
    )
}

fn sse_response(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

fn json_response(value: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(value)
}

fn binary() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_codewhale-tui")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/codewhale-tui")
        })
}

fn preserve_host_env(command: &mut Command) {
    command.env_clear();
    for key in [
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "SystemDrive",
        "WINDIR",
        "COMSPEC",
        "TEMP",
        "TMP",
        "TERM",
        "LANG",
        "LC_ALL",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}

fn run_with_timeout(mut command: Command, timeout: Duration) -> std::process::Output {
    let mut child = command.spawn().expect("spawn codewhale exec");
    let stdout = read_in_background(child.stdout.take().expect("stdout"));
    let stderr = read_in_background(child.stderr.take().expect("stderr"));
    let status = child
        .wait_timeout(timeout)
        .expect("wait")
        .unwrap_or_else(|| {
            let _ = child.kill();
            let _ = child.wait();
            panic!("codewhale exec timed out")
        });
    std::process::Output {
        status,
        stdout: stdout.join().expect("stdout thread").expect("read stdout"),
        stderr: stderr.join().expect("stderr thread").expect("read stderr"),
    }
}

fn read_in_background<R: Read + Send + 'static>(
    mut reader: R,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).map(|_| bytes)
    })
}
