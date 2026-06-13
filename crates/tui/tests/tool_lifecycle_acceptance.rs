//! Cucumber acceptance test for the public LLM/tool lifecycle.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use cucumber::{World as _, gherkin::Step, given, then, when, writer::Stats as _};
use serde_json::{Value, json};
use tempfile::TempDir;
use wait_timeout::ChildExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const FEATURE_NAME: &str = "Tool call lifecycle";
const FEATURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/features/tool_lifecycle_happy_path.feature"
);
const HAPPY_PATH_SCENARIO: &str = "Happy path lists the current directory through a tool";
const TOOL_CALL_ID: &str = "call_list_dir";
const TEST_MODEL: &str = "acceptance-model";

#[derive(Debug, Default, cucumber::World)]
struct ToolLifecycleWorld {
    workspace: Option<TempDir>,
    home: Option<TempDir>,
    llm_server: Option<MockServer>,
    tool_name: Option<String>,
    tool_input: Option<Value>,
    final_answer: Option<String>,
    stdout: String,
    stderr: String,
    events: Vec<Value>,
    requests: Vec<Value>,
}

#[given("an offline CodeWhale workspace containing:")]
fn offline_codewhale_workspace_containing(world: &mut ToolLifecycleWorld, step: &Step) {
    let workspace = TempDir::new().expect("workspace tempdir");
    let home = TempDir::new().expect("home tempdir");

    for row in data_table_rows(step) {
        let relative_path = row_value(&row, "path");
        let kind = row_value(&row, "kind");
        let path = workspace.path().join(relative_path);
        match kind.as_str() {
            "file" => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).expect("create workspace file parent");
                }
                std::fs::write(&path, "").expect("write workspace file");
            }
            "folder" => std::fs::create_dir_all(&path).expect("create workspace folder"),
            other => panic!("unsupported workspace entry kind: {other}"),
        }
    }

    world.workspace = Some(workspace);
    world.home = Some(home);
}

#[given(regex = r#"^the mocked LLM will request the "([^"]+)" tool with:$"#)]
fn mocked_llm_will_request_tool(world: &mut ToolLifecycleWorld, tool_name: String, step: &Step) {
    let rows = data_table_rows(step);
    assert_eq!(rows.len(), 1, "tool input table should contain one row");
    let input = Value::Object(
        rows[0]
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    );

    world.tool_name = Some(tool_name);
    world.tool_input = Some(input);
}

#[given("the mocked LLM will answer after the tool result:")]
fn mocked_llm_will_answer_after_tool_result(world: &mut ToolLifecycleWorld, step: &Step) {
    let rows = data_table_rows(step);
    assert_eq!(rows.len(), 1, "final answer table should contain one row");
    world.final_answer = Some(row_value(&rows[0], "content"));
}

#[when(regex = r#"^the user asks "([^"]+)"$"#)]
async fn user_asks(world: &mut ToolLifecycleWorld, prompt: String) {
    let server = start_mock_llm(world).await;
    let output = run_codewhale_exec(world, &server, &prompt);

    world.stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    world.stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "codewhale-tui exec failed\nstdout:\n{}\nstderr:\n{}",
        world.stdout,
        world.stderr
    );

    world.events = parse_stream_events(&world.stdout);
    world.requests = server
        .received_requests()
        .await
        .expect("mock server should record requests")
        .into_iter()
        .filter(|request| request.url.path().ends_with("/chat/completions"))
        .map(|request| {
            request
                .body_json()
                .expect("chat request body should be JSON")
        })
        .collect();
    world.llm_server = Some(server);
}

#[then("CodeWhale should send the user request to the mocked LLM")]
fn codewhale_should_send_user_request_to_mocked_llm(world: &mut ToolLifecycleWorld) {
    let first_request = world
        .requests
        .first()
        .expect("expected an initial chat request");

    assert!(
        request_contains_user_text(first_request, "list the current directory"),
        "initial request should include the user prompt:\n{first_request:#}"
    );
    assert!(
        !request_contains_tool_result(first_request),
        "initial request should not include a tool result:\n{first_request:#}"
    );
}

#[then("the public tool lifecycle should show a running tool:")]
fn public_tool_lifecycle_should_show_running_tool(world: &mut ToolLifecycleWorld, step: &Step) {
    let expected = one_table_row(step);
    assert_eq!(row_value(&expected, "status"), "running");
    assert_eq!(row_value(&expected, "marker"), "[~]");

    let event = tool_use_event(world, &row_value(&expected, "tool"));
    assert_eq!(
        event.get("input").and_then(|input| input.get("path")),
        Some(&json!(row_value(&expected, "input")))
    );
}

#[then("the public tool result should return directory entries:")]
fn public_tool_result_should_return_directory_entries(world: &mut ToolLifecycleWorld, step: &Step) {
    let output = tool_result_output(world);
    let entries: Vec<Value> =
        serde_json::from_str(output).expect("list_dir result should be JSON entries");

    for row in data_table_rows(step) {
        let expected_name = row_value(&row, "entry");
        let expected_is_dir = match row_value(&row, "kind").as_str() {
            "file" => false,
            "folder" => true,
            other => panic!("unsupported expected entry kind: {other}"),
        };
        assert!(
            entries.iter().any(|entry| {
                entry.get("name").and_then(Value::as_str) == Some(expected_name.as_str())
                    && entry.get("is_dir").and_then(Value::as_bool) == Some(expected_is_dir)
            }),
            "missing {expected_name} in list_dir result:\n{output}"
        );
    }
}

#[then("CodeWhale should send the tool result back to the mocked LLM")]
fn codewhale_should_send_tool_result_back_to_mocked_llm(world: &mut ToolLifecycleWorld) {
    let request = world
        .requests
        .iter()
        .find(|request| request_contains_tool_result(request))
        .expect("expected a follow-up chat request containing the tool result");
    let tool_result = tool_result_message(request).expect("tool result message");
    assert_eq!(
        tool_result
            .get("tool_call_id")
            .and_then(serde_json::Value::as_str),
        Some(TOOL_CALL_ID)
    );

    let content = tool_result
        .get("content")
        .and_then(serde_json::Value::as_str)
        .expect("tool result content");
    for entry in ["README.md", "notes.txt", "src"] {
        assert!(
            content.contains(entry),
            "tool result sent to LLM should include {entry}:\n{content}"
        );
    }
}

#[then("the public tool lifecycle should show a completed tool:")]
fn public_tool_lifecycle_should_show_completed_tool(world: &mut ToolLifecycleWorld, step: &Step) {
    let expected = one_table_row(step);
    assert_eq!(row_value(&expected, "status"), "completed");
    assert_eq!(row_value(&expected, "marker"), "✓");

    let event = tool_result_event(world);
    assert_eq!(event.get("status").and_then(Value::as_str), Some("success"));

    let tool_use = tool_use_event(world, &row_value(&expected, "tool"));
    assert_eq!(
        tool_use.get("input").and_then(|input| input.get("path")),
        Some(&json!(row_value(&expected, "input")))
    );
}

#[then(regex = r#"^the public output should include "([^"]+)"$"#)]
fn public_output_should_include(world: &mut ToolLifecycleWorld, expected: String) {
    let content = world
        .events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("content"))
        .filter_map(|event| event.get("content").and_then(Value::as_str))
        .collect::<String>();
    assert!(
        content.contains(&expected),
        "public content output should include {expected:?}:\nstdout:\n{}\nstderr:\n{}",
        world.stdout,
        world.stderr
    );
}

#[tokio::test(flavor = "current_thread")]
async fn happy_path_lists_current_directory_through_tool() {
    run_scenario(HAPPY_PATH_SCENARIO).await;
}

async fn run_scenario(name: &'static str) {
    let writer = ToolLifecycleWorld::cucumber()
        .fail_on_skipped()
        .with_default_cli()
        .filter_run(FEATURE_PATH, move |feature, _, scenario| {
            feature.name == FEATURE_NAME && scenario.name == name
        })
        .await;
    assert_eq!(writer.failed_steps(), 0, "scenario failed: {name}");
    assert_eq!(writer.skipped_steps(), 0, "scenario skipped steps: {name}");
    assert_eq!(writer.passed_steps(), 10, "scenario did not run: {name}");
}

async fn start_mock_llm(world: &ToolLifecycleWorld) -> MockServer {
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
        .and(request_has_tool_result)
        .respond_with(sse_response(&final_answer_sse(
            world.final_answer.as_ref().expect("final LLM answer"),
        )))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(request_has_no_tool_result)
        .respond_with(sse_response(&tool_call_sse(
            world.tool_name.as_ref().expect("tool name"),
            world.tool_input.as_ref().expect("tool input"),
        )))
        .mount(&server)
        .await;

    server
}

fn run_codewhale_exec(
    world: &ToolLifecycleWorld,
    server: &MockServer,
    prompt: &str,
) -> std::process::Output {
    let workspace = world
        .workspace
        .as_ref()
        .expect("workspace")
        .path()
        .to_path_buf();
    let home = world.home.as_ref().expect("home").path().to_path_buf();

    let mut command = Command::new(codewhale_tui_binary());
    preserve_host_env(&mut command);
    command
        .current_dir(&workspace)
        .arg("--workspace")
        .arg(&workspace)
        .arg("--no-project-config")
        .arg("exec")
        .arg("--auto")
        .arg("--model")
        .arg(TEST_MODEL)
        .arg("--output-format")
        .arg("stream-json")
        .arg(prompt)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env(
            "CODEWHALE_CONFIG_PATH",
            home.join(".codewhale").join("config.toml"),
        )
        .env(
            "DEEPSEEK_CONFIG_PATH",
            home.join(".deepseek").join("config.toml"),
        )
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("CODEWHALE_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", TEST_MODEL)
        .env("CODEWHALE_MODEL", TEST_MODEL)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    std::fs::create_dir_all(home.join(".codewhale")).expect("create codewhale home config dir");
    std::fs::create_dir_all(home.join(".deepseek")).expect("create deepseek home config dir");

    run_with_timeout(command, Duration::from_secs(45))
}

fn run_with_timeout(mut command: Command, timeout: Duration) -> std::process::Output {
    let mut child = command.spawn().expect("spawn codewhale-tui exec");
    let stdout_reader = read_pipe_in_background(child.stdout.take().expect("stdout pipe"));
    let stderr_reader = read_pipe_in_background(child.stderr.take().expect("stderr pipe"));

    let status = match child.wait_timeout(timeout).expect("wait for codewhale-tui") {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = join_pipe_reader(stdout_reader, "stdout");
            let stderr = join_pipe_reader(stderr_reader, "stderr");
            panic!(
                "codewhale-tui exec timed out after {timeout:?}\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }
    };

    let stdout = join_pipe_reader(stdout_reader, "stdout");
    let stderr = join_pipe_reader(stderr_reader, "stderr");

    std::process::Output {
        status,
        stdout,
        stderr,
    }
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
        .unwrap_or_else(|err| panic!("read {stream_name}: {err}"))
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

fn tool_call_sse(tool_name: &str, tool_input: &Value) -> String {
    let arguments = serde_json::to_string(tool_input).expect("tool input arguments");
    [
        sse_chunk(json!({
            "id": "chatcmpl-tool",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": TOOL_CALL_ID,
                        "type": "function",
                        "function": {
                            "name": tool_name,
                            "arguments": arguments
                        }
                    }]
                },
                "finish_reason": null
            }]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-tool",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 2,
                "total_tokens": 12
            }
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

fn final_answer_sse(answer: &str) -> String {
    [
        sse_chunk(json!({
            "id": "chatcmpl-final",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "delta": { "content": answer },
                "finish_reason": null
            }]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-final",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 8,
                "total_tokens": 28
            }
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

fn sse_chunk(value: Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::to_string(&value).expect("SSE JSON")
    )
}

fn sse_response(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .insert_header("cache-control", "no-cache")
        .set_body_string(body.to_string())
}

fn json_response(value: Value) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "application/json")
        .set_body_json(value)
}

fn request_has_tool_result(request: &Request) -> bool {
    request
        .body_json::<Value>()
        .is_ok_and(|body| request_contains_tool_result(&body))
}

fn request_has_no_tool_result(request: &Request) -> bool {
    !request_has_tool_result(request)
}

fn request_contains_tool_result(request: &Value) -> bool {
    tool_result_message(request).is_some()
}

fn tool_result_message(request: &Value) -> Option<&Value> {
    request
        .get("messages")
        .and_then(Value::as_array)?
        .iter()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
}

fn request_contains_user_text(request: &Value, expected: &str) -> bool {
    request
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|message| {
            message.get("role").and_then(Value::as_str) == Some("user")
                && message
                    .get("content")
                    .is_some_and(|content| value_contains_text(content, expected))
        })
}

fn value_contains_text(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(text) => text.contains(expected),
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_text(value, expected)),
        Value::Object(values) => values
            .values()
            .any(|value| value_contains_text(value, expected)),
        _ => false,
    }
}

fn parse_stream_events(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let json_start = line.find('{')?;
            let json_line = &line[json_start..];
            Some(serde_json::from_str(json_line).unwrap_or_else(|err| {
                panic!(
                    "stream-json line should parse: {err}\nline: {line}\njson: {json_line}\nstdout:\n{stdout}"
                )
            }))
        })
        .collect()
}

fn tool_use_event<'a>(world: &'a ToolLifecycleWorld, expected_tool: &str) -> &'a Value {
    world
        .events
        .iter()
        .find(|event| {
            event.get("type").and_then(Value::as_str) == Some("tool_use")
                && event.get("name").and_then(Value::as_str) == Some(expected_tool)
        })
        .unwrap_or_else(|| {
            panic!(
                "expected tool_use event for {expected_tool}\nstdout:\n{}\nstderr:\n{}",
                world.stdout, world.stderr
            )
        })
}

fn tool_result_event(world: &ToolLifecycleWorld) -> &Value {
    world
        .events
        .iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some("tool_result"))
        .unwrap_or_else(|| {
            panic!(
                "expected tool_result event\nstdout:\n{}\nstderr:\n{}",
                world.stdout, world.stderr
            )
        })
}

fn tool_result_output(world: &ToolLifecycleWorld) -> &str {
    tool_result_event(world)
        .get("output")
        .and_then(Value::as_str)
        .expect("tool_result output")
}

fn one_table_row(step: &Step) -> Vec<(String, String)> {
    let rows = data_table_rows(step);
    assert_eq!(rows.len(), 1, "expected exactly one data table row");
    rows.into_iter().next().expect("one row")
}

fn data_table_rows(step: &Step) -> Vec<Vec<(String, String)>> {
    let table = step
        .table
        .as_ref()
        .expect("step should include a data table");
    let mut rows = table.rows.iter();
    let headers = rows
        .next()
        .expect("data table should include a header")
        .clone();

    let values: Vec<Vec<(String, String)>> = rows
        .map(|row| {
            headers
                .iter()
                .zip(row.iter())
                .map(|(header, value)| (header.clone(), value.clone()))
                .collect()
        })
        .collect();
    assert!(
        !values.is_empty(),
        "data table should include at least one row"
    );
    values
}

fn row_value(row: &[(String, String)], header: &str) -> String {
    row.iter()
        .find_map(|(key, value)| (key == header).then(|| value.clone()))
        .unwrap_or_else(|| panic!("data table row missing {header} value"))
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
