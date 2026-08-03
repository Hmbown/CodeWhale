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
    for (receipt, sentinel) in [
        (&success_receipt, SUCCESS_SENTINEL),
        (&failure_receipt, FAILURE_SENTINEL),
    ] {
        assert!(
            receipt.contains("of output omitted"),
            "model-facing truncation must state how much was omitted"
        );
        assert!(
            receipt.contains("full output at"),
            "model-facing truncation must name the recovery path"
        );
        assert!(
            receipt.contains("/artifacts/"),
            "the footer deliberately names the on-disk artifact so the model can read the omitted range back"
        );
        assert!(!receipt.contains("retrieve_tool_result"));
        assert!(!receipt.contains("[Exact evidence retained"));
        assert!(!receipt.contains(sentinel));
        assert!(
            receipt.len() <= 42_000,
            "bounded preview must stay within the hybrid 32 KiB head + 8 KiB tail receipt budget, got {} bytes",
            receipt.len()
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
    let (sentinel, prefix) = if success {
        (SUCCESS_SENTINEL, "BASH-SUCCESS")
    } else {
        (FAILURE_SENTINEL, "BASH-FAILURE")
    };
    let command = probe_command(sentinel, prefix, success);
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

/// Shell fixture that emits enough bytes to force exact-evidence routing under
/// the 32_768-token default threshold. The Bash adapter self-bounds each
/// stream to ~30 KB, so a single stream would now fit inside the hybrid
/// 32 KiB + 8 KiB preview budget; the probe therefore fills stdout AND stderr
/// (~60 KB combined) so the envelope still omits a middle range. The sentinel
/// rides stderr at filler line 100: deep enough to survive the shell tool's
/// own 22 KB head bound (so the artifact retains it) yet beyond the preview's
/// 32 KiB head (so the model receipt omits it). The probe executes through
/// the platform shell — bash on Unix, `cmd /C` on Windows
/// (#1691) — so each platform needs native syntax to exercise the same
/// routing path.
#[cfg(not(windows))]
fn probe_command(sentinel: &str, prefix: &str, success: bool) -> String {
    let trailer = if success { "" } else { "; exit 7" };
    let stdout_loop = format!(
        "i=0; while [ \"$i\" -lt 2800 ]; do printf '{prefix}-%04d-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n' \"$i\"; i=$((i + 1)); done"
    );
    let stderr_loop = format!(
        "j=0; while [ \"$j\" -lt 2800 ]; do if [ \"$j\" -eq 100 ]; then printf '%s\\n' '{sentinel}'; fi; printf '{prefix}-ERR-%04d-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n' \"$j\"; j=$((j + 1)); done"
    );
    format!("{stdout_loop}; {{ {stderr_loop}; }} >&2{trailer}")
}

/// PowerShell syntax: on Windows the shell dispatcher prefers `pwsh.exe`,
/// then in-box `powershell.exe`, only falling back to `cmd.exe` when no
/// PowerShell exists at all. Single quotes only — the payload is passed to
/// `-Command` as one argv string, and four or more double quotes would push
/// it onto the temp-`-File` path for no benefit. The failure variant mirrors
/// the Unix `{ ...; } >&2; exit 7` shape by writing every line to the OS
/// stderr handle and exiting 7 after the loop.
#[cfg(windows)]
fn probe_command(sentinel: &str, prefix: &str, success: bool) -> String {
    let stdout_line = format!(
        "'{prefix}-{{0}}-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'"
    );
    let stderr_line = format!(
        "'{prefix}-ERR-{{0}}-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'"
    );
    let stdout_loop = format!("0..2799 | ForEach-Object {{ Write-Output ({stdout_line} -f $_) }}");
    let stderr_loop = format!(
        "0..2799 | ForEach-Object {{ if ($_ -eq 100) {{ [Console]::Error.WriteLine('{sentinel}') }}; [Console]::Error.WriteLine(({stderr_line} -f $_)) }}"
    );
    let trailer = if success { "" } else { "; exit 7" };
    format!("{stdout_loop}; {stderr_loop}{trailer}")
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
