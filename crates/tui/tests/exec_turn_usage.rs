//! End-to-end shape lock for the per-model-call `turn_usage` event on the
//! `codewhale exec --output-format stream-json` stream (#52 / FINISH-0.9.4).
//!
//! A `wiremock` OpenAI-compatible endpoint stands in for the provider. Two
//! cases pin the contract:
//!
//! - usage reported by the provider -> exactly one `turn_usage` event per
//!   model call, carrying the reported input/output/reasoning/cache fields,
//!   and the pre-existing event sequence (`content` … `metadata` → `done`)
//!   is unchanged for existing consumers;
//! - usage absent from the provider stream -> no `turn_usage` event at all
//!   (honest absence, never fabricated zeros-as-data).

#![cfg(unix)]

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use wait_timeout::ChildExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_MODEL: &str = "turn-usage-model";
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

fn sse_chunk(value: Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::to_string(&value).expect("SSE JSON")
    )
}

/// Final-answer SSE whose closing chunk reports usage with reasoning and
/// DeepSeek-style prompt-cache fields.
fn answer_sse_with_usage(answer: &str) -> String {
    [
        sse_chunk(json!({
            "id": "chatcmpl-usage",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {"content": answer}, "finish_reason": null}]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-usage",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 8,
                "total_tokens": 28,
                "completion_tokens_details": {"reasoning_tokens": 5},
                "prompt_cache_hit_tokens": 12,
                "prompt_cache_miss_tokens": 8
            }
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

/// Final-answer SSE whose provider never reports usage.
fn answer_sse_without_usage(answer: &str) -> String {
    [
        sse_chunk(json!({
            "id": "chatcmpl-no-usage",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {"content": answer}, "finish_reason": null}]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-no-usage",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

fn sse_response(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .insert_header("cache-control", "no-cache")
        .set_body_string(body)
}

fn json_response(value: Value) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "application/json")
        .set_body_json(value)
}

async fn start_mock_llm(answer_sse: String) -> MockServer {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(json_response(json!({
            "object": "list",
            "data": [{ "id": TEST_MODEL, "object": "model" }]
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(answer_sse))
        .mount(&server)
        .await;

    server
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
        "COLORTERM",
        "LANG",
        "LC_ALL",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}

fn run_exec_stream_json(server: &MockServer) -> Vec<Value> {
    let workspace = TempDir::new().expect("workspace tempdir");
    let home = TempDir::new().expect("home tempdir");

    let mut command = Command::new(codewhale_tui_binary());
    preserve_host_env(&mut command);
    command
        .current_dir(workspace.path())
        .arg("--workspace")
        .arg(workspace.path())
        .arg("--no-project-config")
        .arg("exec")
        .arg("--auto")
        .arg("--model")
        .arg(TEST_MODEL)
        .arg("--output-format")
        .arg("stream-json")
        .arg("answer briefly")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("XDG_DATA_HOME", home.path().join(".local").join("share"))
        .env("XDG_CACHE_HOME", home.path().join(".cache"))
        .env(
            "CODEWHALE_CONFIG_PATH",
            home.path().join(".codewhale").join("config.toml"),
        )
        .env(
            "DEEPSEEK_CONFIG_PATH",
            home.path().join(".deepseek").join("config.toml"),
        )
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("CODEWHALE_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", TEST_MODEL)
        .env("CODEWHALE_MODEL", TEST_MODEL)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    std::fs::create_dir_all(home.path().join(".codewhale")).expect("create codewhale config dir");
    std::fs::create_dir_all(home.path().join(".deepseek")).expect("create deepseek config dir");

    let mut child = command.spawn().expect("spawn codewhale-tui exec");
    let stdout_reader = read_pipe_in_background(child.stdout.take().expect("stdout pipe"));
    let stderr_reader = read_pipe_in_background(child.stderr.take().expect("stderr pipe"));

    let status = match child
        .wait_timeout(RUN_TIMEOUT)
        .expect("wait for codewhale-tui")
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = join_pipe_reader(stdout_reader, "stdout");
            let stderr = join_pipe_reader(stderr_reader, "stderr");
            panic!(
                "codewhale-tui exec timed out after {RUN_TIMEOUT:?}\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }
    };

    let stdout = join_pipe_reader(stdout_reader, "stdout");
    let stderr = join_pipe_reader(stderr_reader, "stderr");
    assert!(
        status.success(),
        "codewhale-tui exec failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );

    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|err| {
                panic!("stream-json line should parse: {err}\nline: {line}\nstdout:\n{stdout}")
            })
        })
        .collect()
}

fn read_pipe_in_background<R>(mut reader: R) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut output = Vec::new();
        reader.read_to_end(&mut output).map(|_| output)
    })
}

fn join_pipe_reader(
    handle: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
    stream_name: &str,
) -> Vec<u8> {
    handle
        .join()
        .unwrap_or_else(|_| panic!("{stream_name} reader thread panicked"))
        .unwrap_or_else(|err| panic!("failed to read {stream_name}: {err}"))
}

fn codewhale_tui_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_codewhale-tui") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_codewhale-tui") {
        return PathBuf::from(path);
    }

    let mut path = std::env::current_exe().expect("current test executable path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX));
    path
}

fn events_of_type<'a>(events: &'a [Value], event_type: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(event_type))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn turn_usage_event_is_emitted_with_reported_fields_and_stream_contract_holds() {
    let server = start_mock_llm(answer_sse_with_usage("done in one step")).await;
    let events = run_exec_stream_json(&server);

    // Every event carries the stream schema envelope.
    for event in &events {
        assert_eq!(event["schema"], "codewhale.exec-stream");
        assert_eq!(event["schema_version"], 1);
    }

    // Exactly one per-call usage receipt, numbered from 1.
    let usage_events = events_of_type(&events, "turn_usage");
    assert_eq!(
        usage_events.len(),
        1,
        "expected one turn_usage event: {events:#?}"
    );
    let usage = usage_events[0];
    assert_eq!(usage["turn"], 1);
    assert_eq!(usage["input_tokens"], 20);
    assert_eq!(usage["output_tokens"], 8);
    assert_eq!(usage["reasoning_tokens"], 5);
    assert_eq!(usage["prompt_cache_hit_tokens"], 12);
    assert_eq!(usage["prompt_cache_miss_tokens"], 8);
    assert!(
        usage["duration_ms"].as_u64().is_some(),
        "duration_ms must be a non-negative integer: {usage}"
    );
    // Fields the provider did not report are omitted, not zero-filled.
    let usage_object = usage.as_object().expect("turn_usage object");
    for absent in ["prompt_cache_write_tokens", "reasoning_replay_tokens"] {
        assert!(
            !usage_object.contains_key(absent),
            "{absent} must be omitted when unreported: {usage}"
        );
    }

    // The usage receipt lands after the model output it accounts for and
    // before the terminal receipts.
    let types: Vec<&str> = events
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .collect();
    let content_pos = types.iter().position(|t| *t == "content");
    let usage_pos = types.iter().position(|t| *t == "turn_usage");
    assert!(
        content_pos.is_some_and(|c| usage_pos.is_some_and(|u| c < u)),
        "turn_usage must follow the content it accounts for: {types:?}"
    );

    // Existing consumers' terminal contract is unchanged: `metadata`
    // immediately precedes exactly one trailing `done`.
    assert_eq!(types.last(), Some(&"done"), "stream must end with done");
    assert_eq!(
        types.get(types.len() - 2),
        Some(&"metadata"),
        "metadata must immediately precede done: {types:?}"
    );
    assert_eq!(
        events_of_type(&events, "done").len(),
        1,
        "exactly one done event"
    );
    let metadata = events_of_type(&events, "metadata");
    assert_eq!(metadata.len(), 1, "exactly one metadata event");
    // The terminal receipt still carries the cumulative usage.
    assert_eq!(metadata[0]["meta"]["input_tokens"], 20);
    assert_eq!(metadata[0]["meta"]["output_tokens"], 8);
    assert_eq!(metadata[0]["meta"]["reasoning_tokens"], 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn turn_usage_event_is_skipped_when_provider_reports_no_usage() {
    let server = start_mock_llm(answer_sse_without_usage("quiet answer")).await;
    let events = run_exec_stream_json(&server);

    assert!(
        events_of_type(&events, "turn_usage").is_empty(),
        "no turn_usage event without provider-reported usage: {events:#?}"
    );

    // The rest of the stream contract still holds.
    let types: Vec<&str> = events
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .collect();
    assert!(types.contains(&"content"), "content missing: {types:?}");
    assert_eq!(types.last(), Some(&"done"), "stream must end with done");
    assert_eq!(types.get(types.len() - 2), Some(&"metadata"));
}
