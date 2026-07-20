//! Provider-free acceptance lock for the public headless launch contract that
//! makes CodeWhale embeddable as a future Verifiers v1 harness (#4641).
//!
//! Everything here is loopback and sealed: a `wiremock` OpenAI-compatible
//! fixture stands in for the interception endpoint, `CODEWHALE_HOME` is a fresh
//! per-run directory, the credential is delivered only through the route's
//! `api_key_env`, and a sentinel secret must never escape the child process.
//! No provider credential, network egress, or installed CodeWhale is required.

#![cfg(unix)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use wait_timeout::ChildExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_MODEL: &str = "verifiers-contract-model";
const API_KEY_ENV: &str = "VF_CODEWHALE_API_KEY";
/// A value that appears nowhere except the child environment; any sighting in
/// argv, stdout, stderr, the stream-json stream, or a written file is a leak.
const SENTINEL_SECRET: &str = "vf-sentinel-do-not-leak-8f3a2b1c9d7e";
const APPEND_MARKER: &str = "VF-APPENDED-SYSTEM-PROMPT-MARKER";
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

fn sse_chunk(value: Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::to_string(&value).expect("SSE JSON")
    )
}

fn text_sse(model: &str, text: &str) -> String {
    [
        sse_chunk(json!({
            "id": "chatcmpl-vf",
            "object": "chat.completion.chunk",
            "model": model,
            "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-vf",
            "object": "chat.completion.chunk",
            "model": model,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 11, "completion_tokens": 3, "total_tokens": 14}
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

async fn mount_models(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(json_response(json!({
            "object": "list",
            "data": [{"id": TEST_MODEL, "object": "model"}]
        })))
        .mount(server)
        .await;
}

struct Fixture {
    _root: TempDir,
    home: PathBuf,
    codewhale_home: PathBuf,
    workspace: PathBuf,
    config_path: PathBuf,
    mcp_config_path: PathBuf,
}

impl Fixture {
    /// Build a sealed launch environment whose only route lives in an explicit
    /// `--config` file that names the credential env var but never the secret.
    fn new(base_url: &str) -> Self {
        let root = TempDir::new().expect("fixture root");
        let home = root.path().join("home");
        let codewhale_home = root.path().join("codewhale-home");
        let workspace = root.path().join("workspace");
        for dir in [&home, &codewhale_home, &workspace] {
            std::fs::create_dir_all(dir).expect("create fixture dir");
        }

        // Route config: only the env-var NAME is stored, never the secret.
        let config_path = root.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                "provider = \"openai\"\n\n[providers.openai]\nbase_url = \"{base_url}/v1\"\nmodel = \"{TEST_MODEL}\"\napi_key_env = \"{API_KEY_ENV}\"\n"
            ),
        )
        .expect("write route config");

        // Generated MCP file with no task servers: proves the URL-based MCP
        // config surface loads cleanly from a fresh, generated file.
        let mcp_config_path = root.path().join("vf-mcp.json");
        std::fs::write(&mcp_config_path, json!({"mcpServers": {}}).to_string())
            .expect("write mcp config");

        Fixture {
            _root: root,
            home,
            codewhale_home,
            workspace,
            config_path,
            mcp_config_path,
        }
    }

    /// The exact `#4641` launch shape, minus the dispatcher hop (this drives the
    /// `codewhale-tui` runtime directly). `--no-project-config` precedes the
    /// subcommand; the prompt follows `--`.
    fn exec_argv(&self, prompt: &str) -> Vec<String> {
        vec![
            "--config".into(),
            self.config_path.to_string_lossy().into_owned(),
            "--workspace".into(),
            self.workspace.to_string_lossy().into_owned(),
            "--no-project-config".into(),
            "--skip-onboarding".into(),
            "exec".into(),
            "--auto".into(),
            "--sandbox".into(),
            "danger-full-access".into(),
            "--append-system-prompt".into(),
            APPEND_MARKER.into(),
            "--disallowed-tools".into(),
            "web_search".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--".into(),
            prompt.into(),
        ]
    }

    fn run(&self, prompt: &str) -> std::process::Output {
        let argv = self.exec_argv(prompt);
        // The secret must never be an argument; only its env-var name is.
        assert!(
            !argv.iter().any(|arg| arg.contains(SENTINEL_SECRET)),
            "sentinel secret must never appear in argv"
        );

        let mut command = Command::new(codewhale_tui_binary());
        preserve_host_env(&mut command);
        command
            .current_dir(&self.workspace)
            .args(&argv)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local").join("share"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("CODEWHALE_HOME", &self.codewhale_home)
            .env("CODEWHALE_SECRET_BACKEND", "file")
            .env("CODEWHALE_MEMORY", "false")
            .env("CODEWHALE_TELEMETRY", "false")
            .env("CODEWHALE_MCP_CONFIG", &self.mcp_config_path)
            // The interception secret lives ONLY in the child environment,
            // reached through the route's api_key_env.
            .env(API_KEY_ENV, SENTINEL_SECRET)
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        run_with_timeout(command, RUN_TIMEOUT)
    }

    /// Every regular file under the sealed roots, for secret-leak scanning.
    fn written_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for base in [&self.home, &self.codewhale_home, &self.workspace] {
            collect_files(base, &mut out);
        }
        out.push(self.config_path.clone());
        out.push(self.mcp_config_path.clone());
        out
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => collect_files(&path, out),
            Ok(ft) if ft.is_file() => out.push(path),
            _ => {}
        }
    }
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

/// The public headless launch reaches exactly the configured route/model, the
/// interception secret stays confined to the child environment, the appended
/// system prompt is delivered, the generated MCP config loads, and the run
/// exits cleanly — all provider-free.
#[tokio::test(flavor = "current_thread")]
async fn headless_launch_confines_secret_and_reaches_configured_route() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(text_sse(TEST_MODEL, "contract acknowledged")))
        .mount(&server)
        .await;

    let fixture = Fixture::new(&server.uri());
    let output = fixture.run("Reply with a short acknowledgement.");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "headless exec should exit 0\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let requests = server
        .received_requests()
        .await
        .expect("recorded fixture requests");
    let chat: Vec<&wiremock::Request> = requests
        .iter()
        .filter(|req| req.url.path() == "/v1/chat/completions")
        .collect();
    assert!(
        !chat.is_empty(),
        "expected at least one chat/completions request to the configured endpoint"
    );

    // Reaches the exact configured model, carrying the secret only as the
    // Authorization bearer resolved from api_key_env.
    let first = &chat[0];
    let body: Value = serde_json::from_slice(&first.body).expect("chat request body JSON");
    assert_eq!(
        body["model"], TEST_MODEL,
        "request must target the configured model"
    );
    let auth = first
        .headers
        .get("authorization")
        .map(|value| value.to_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    assert_eq!(
        auth,
        format!("Bearer {SENTINEL_SECRET}"),
        "the route must present the api_key_env secret to the model endpoint"
    );

    // The appended system prompt is delivered to the model.
    let body_text = String::from_utf8_lossy(&first.body);
    assert!(
        body_text.contains(APPEND_MARKER),
        "appended system prompt must reach the model request"
    );

    // Secret hygiene: the sentinel must not appear in argv, stdout (the
    // stream-json stream), stderr, or any written file.
    assert!(
        !stdout.contains(SENTINEL_SECRET),
        "secret leaked into stdout/stream-json"
    );
    assert!(
        !stderr.contains(SENTINEL_SECRET),
        "secret leaked into stderr"
    );
    for file in fixture.written_files() {
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SENTINEL_SECRET),
            "secret leaked into written file: {}",
            file.display()
        );
    }

    // Stdout is a valid stream-json stream (every non-empty line parses).
    let mut saw_line = false;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        saw_line = true;
        serde_json::from_str::<Value>(line)
            .unwrap_or_else(|err| panic!("stream-json line must parse: {err}\nline: {line}"));
    }
    assert!(saw_line, "expected a stream-json event stream on stdout");
}

/// A fixture model failure surfaces as a nonzero exit, so a harness can treat
/// the launch as failed rather than silently succeeding.
#[tokio::test(flavor = "current_thread")]
async fn fixture_model_failure_exits_nonzero() {
    let server = MockServer::start().await;
    mount_models(&server).await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream model failure"))
        .mount(&server)
        .await;

    let fixture = Fixture::new(&server.uri());
    let output = fixture.run("This request should fail at the model.");
    assert!(
        !output.status.success(),
        "a fixture model failure must exit nonzero\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
