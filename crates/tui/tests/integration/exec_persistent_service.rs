//! Real-process acceptance for `persist:true` background services on the
//! headless `codewhale exec` host.
//!
//! Three black-box contracts against the actual binary and real child
//! processes, with a `wiremock` OpenAI-compatible provider:
//!
//! - a successful exec releases the explicitly persisted service: the exec
//!   process exits 0, emits a `service_released` receipt, and the service
//!   process is still alive afterwards;
//! - a failed exec (incomplete non-limit stop) kills the pending service and
//!   exits nonzero;
//! - a terminating signal mid-turn kills the pending service and exits
//!   nonzero.

#![cfg(unix)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

async fn serialize_persistent_service_tests() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

use serde_json::{Value, json};
use tempfile::TempDir;
use wait_timeout::ChildExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TEST_MODEL: &str = "persist-service-model";
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

fn sse_chunk(value: Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::to_string(&value).expect("SSE JSON")
    )
}

/// First model turn: one Bash tool call staging the persistent service.
fn stage_service_sse(command: &str) -> String {
    let arguments = serde_json::to_string(&json!({
        "command": command,
        "background": true,
        "persist": true,
    }))
    .expect("tool arguments JSON");
    [
        sse_chunk(json!({
            "id": "chatcmpl-stage",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_persist", "type": "function", "function": {"name": "Bash", "arguments": arguments}}]}, "finish_reason": null}]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-stage",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 4, "total_tokens": 16}
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

/// Second model turn: an ordinary completed final answer.
fn final_answer_sse() -> String {
    [
        sse_chunk(json!({
            "id": "chatcmpl-final",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {"content": "service is up"}, "finish_reason": null}]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-final",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 30, "completion_tokens": 3, "total_tokens": 33}
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

/// Second model turn: incomplete non-limit stop. `length` now degrades and
/// continues the headless loop (no default max-turns), so this fixture uses
/// `content_filter` to force a failed exec and prove pending services die.
fn incomplete_answer_sse() -> String {
    [
        sse_chunk(json!({
            "id": "chatcmpl-incomplete",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {"content": "partial"}, "finish_reason": null}]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-incomplete",
            "object": "chat.completion.chunk",
            "model": TEST_MODEL,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "content_filter"}],
            "usage": {"prompt_tokens": 30, "completion_tokens": 2, "total_tokens": 32}
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

/// Sequential provider: first POST stages the service; later POSTs get the
/// scenario's second turn. The failure scenario waits for explicit service
/// readiness; an optional delay holds the exec mid-turn for the signal case.
struct SequentialTurns {
    requests: Arc<AtomicUsize>,
    stage_command: String,
    second_turn: String,
    second_turn_delay: Option<Duration>,
    second_turn_ready: Option<Mutex<mpsc::Receiver<()>>>,
}

impl Respond for SequentialTurns {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let call = self.requests.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            sse_response(stage_service_sse(&self.stage_command))
        } else {
            if call == 1
                && let Some(ready) = &self.second_turn_ready
                && ready
                    .lock()
                    .expect("service readiness receiver")
                    .recv_timeout(RUN_TIMEOUT)
                    .is_err()
            {
                return ResponseTemplate::new(504)
                    .set_body_string("test did not confirm service readiness");
            }
            let response = sse_response(self.second_turn.clone());
            match self.second_turn_delay {
                Some(delay) => response.set_delay(delay),
                None => response,
            }
        }
    }
}

async fn start_mock_llm(
    stage_command: &str,
    second_turn: String,
    second_turn_delay: Option<Duration>,
    second_turn_ready: Option<mpsc::Receiver<()>>,
) -> MockServer {
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
        .respond_with(SequentialTurns {
            requests: Arc::new(AtomicUsize::new(0)),
            stage_command: stage_command.to_string(),
            second_turn,
            second_turn_delay,
            second_turn_ready: second_turn_ready.map(Mutex::new),
        })
        .mount(&server)
        .await;

    server
}

fn preserve_host_env(command: &mut Command) {
    command.env_clear();
    for key in [
        "PATH",
        "SHELL",
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

fn exec_command(server: &MockServer, workspace: &Path, home: &Path) -> Command {
    let mut command = Command::new(codewhale_tui_binary());
    preserve_host_env(&mut command);
    command
        .current_dir(workspace)
        .arg("--workspace")
        .arg(workspace)
        .arg("--no-project-config")
        .arg("exec")
        .arg("--auto")
        .arg("--sandbox")
        .arg("danger-full-access")
        .arg("--model")
        .arg(TEST_MODEL)
        .arg("--output-format")
        .arg("stream-json")
        .arg("start the service, then confirm")
        .env("HOME", home)
        .env("USERPROFILE", home)
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
    std::fs::create_dir_all(home.join(".codewhale")).expect("create codewhale config dir");
    std::fs::create_dir_all(home.join(".deepseek")).expect("create deepseek config dir");
    command
}

fn read_pipe_in_background<R>(mut reader: R) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_pipe(handle: std::thread::JoinHandle<std::io::Result<Vec<u8>>>, label: &str) -> String {
    let bytes = handle
        .join()
        .unwrap_or_else(|_| panic!("{label} reader thread panicked"))
        .unwrap_or_else(|error| panic!("{label} read failed: {error}"));
    String::from_utf8_lossy(&bytes).into_owned()
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

fn stream_events(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect()
}

fn pid_is_alive(pid: i32) -> bool {
    // SAFETY: signal 0 performs only an existence/permission check.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn kill_process_group(pid: i32) {
    // SAFETY: the pid came from this test's own staged service; the negative
    // pid targets its process group only.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

fn wait_for_pid_file(path: &Path) -> Result<i32, String> {
    let deadline = Instant::now() + RUN_TIMEOUT;
    loop {
        if let Ok(contents) = std::fs::read_to_string(path)
            && let Ok(pid) = contents.trim().parse::<i32>()
        {
            return Ok(pid);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "service pid file never appeared at {}",
                path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_pid_death(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while pid_is_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "pending persistent service (pid {pid}) must be killed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The staged service records its own pid, then stays alive.
const SERVICE_COMMAND: &str = "echo $$ > service.pid; exec sleep 600";

#[tokio::test(flavor = "multi_thread")]
async fn successful_exec_releases_persisted_service() {
    let _serial = serialize_persistent_service_tests().await;
    let server = start_mock_llm(SERVICE_COMMAND, final_answer_sse(), None, None).await;
    let workspace = TempDir::new().expect("workspace tempdir");
    let home = TempDir::new().expect("home tempdir");

    let mut child = exec_command(&server, workspace.path(), home.path())
        .spawn()
        .expect("spawn codewhale-tui exec");
    let stdout_reader = read_pipe_in_background(child.stdout.take().expect("stdout pipe"));
    let stderr_reader = read_pipe_in_background(child.stderr.take().expect("stderr pipe"));
    let status = child
        .wait_timeout(RUN_TIMEOUT)
        .expect("wait for exec")
        .unwrap_or_else(|| {
            let _ = child.kill();
            let _ = child.wait();
            panic!("exec timed out");
        });
    let stdout = join_pipe(stdout_reader, "stdout");
    let stderr = join_pipe(stderr_reader, "stderr");

    let service_pid = wait_for_pid_file(&workspace.path().join("service.pid"))
        .unwrap_or_else(|error| panic!("{error}\nstdout:\n{stdout}\nstderr:\n{stderr}"));
    let events = stream_events(&stdout);
    let released = events
        .iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some("service_released"))
        .unwrap_or_else(|| {
            panic!("missing service_released event\nstdout:\n{stdout}\nstderr:\n{stderr}")
        });

    assert!(
        status.success(),
        "successful exec must exit 0 (got {status:?})\nstderr:\n{stderr}"
    );
    assert_eq!(
        released.get("pid").and_then(Value::as_u64),
        Some(u64::try_from(service_pid).expect("pid fits u64")),
        "release receipt must carry the real service pid"
    );
    assert_eq!(
        released.get("ownership").and_then(Value::as_str),
        Some("external")
    );
    assert!(
        pid_is_alive(service_pid),
        "explicitly persisted service must survive successful headless exit"
    );

    kill_process_group(service_pid);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_exec_kills_pending_service_and_exits_nonzero() {
    let _serial = serialize_persistent_service_tests().await;
    let (service_ready, service_ready_receiver) = mpsc::channel();
    let server = start_mock_llm(
        SERVICE_COMMAND,
        incomplete_answer_sse(),
        None,
        Some(service_ready_receiver),
    )
    .await;
    let workspace = TempDir::new().expect("workspace tempdir");
    let home = TempDir::new().expect("home tempdir");

    let mut child = exec_command(&server, workspace.path(), home.path())
        .spawn()
        .expect("spawn codewhale-tui exec");
    let stdout_reader = read_pipe_in_background(child.stdout.take().expect("stdout pipe"));
    let stderr_reader = read_pipe_in_background(child.stderr.take().expect("stderr pipe"));

    // Spawning a background process does not mean its first instruction ran.
    // Hold the deliberate model failure until the real service is ready, so
    // cancellation cannot kill it before it writes the PID we need to check.
    let service_pid = match wait_for_pid_file(&workspace.path().join("service.pid")) {
        Ok(pid) => pid,
        Err(error) => {
            drop(service_ready);
            // Let the host clean up its managed services before forcing exit.
            // SAFETY: direct child of this test.
            unsafe {
                libc::kill(
                    i32::try_from(child.id()).expect("child pid fits i32"),
                    libc::SIGTERM,
                );
            }
            if child
                .wait_timeout(Duration::from_secs(5))
                .ok()
                .flatten()
                .is_none()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
            let stdout = join_pipe(stdout_reader, "stdout");
            let stderr = join_pipe(stderr_reader, "stderr");
            panic!("{error}\nstdout:\n{stdout}\nstderr:\n{stderr}");
        }
    };
    assert!(
        pid_is_alive(service_pid),
        "service must be alive before the model fails"
    );
    service_ready
        .send(())
        .expect("release the deliberate model failure");

    let status = match child.wait_timeout(RUN_TIMEOUT).expect("wait for exec") {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = join_pipe(stdout_reader, "stdout");
            let stderr = join_pipe(stderr_reader, "stderr");
            panic!("exec timed out\nstdout:\n{stdout}\nstderr:\n{stderr}");
        }
    };
    let stdout = join_pipe(stdout_reader, "stdout");
    let stderr = join_pipe(stderr_reader, "stderr");
    assert!(
        !status.success(),
        "provider incomplete stop must fail the exec\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stream_events(&stdout)
            .iter()
            .any(|event| event.get("type").and_then(Value::as_str) == Some("service_released")),
        "a failed exec must never release a pending service"
    );
    wait_for_pid_death(service_pid);
}

#[tokio::test(flavor = "multi_thread")]
async fn terminating_signal_kills_pending_service_and_exits_nonzero() {
    let _serial = serialize_persistent_service_tests().await;
    // Hold the second model turn open long past the signal.
    let server = start_mock_llm(
        SERVICE_COMMAND,
        final_answer_sse(),
        Some(Duration::from_secs(300)),
        None,
    )
    .await;
    let workspace = TempDir::new().expect("workspace tempdir");
    let home = TempDir::new().expect("home tempdir");

    let mut child = exec_command(&server, workspace.path(), home.path())
        .spawn()
        .expect("spawn codewhale-tui exec");
    let stdout_reader = read_pipe_in_background(child.stdout.take().expect("stdout pipe"));
    let stderr_reader = read_pipe_in_background(child.stderr.take().expect("stderr pipe"));

    // The pid file proves the service was staged before the signal.
    let service_pid = wait_for_pid_file(&workspace.path().join("service.pid"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(pid_is_alive(service_pid));

    // SAFETY: direct child of this test.
    unsafe {
        libc::kill(
            i32::try_from(child.id()).expect("child pid fits i32"),
            libc::SIGTERM,
        );
    }
    let status = child
        .wait_timeout(Duration::from_secs(30))
        .expect("wait for signalled exec")
        .unwrap_or_else(|| {
            let _ = child.kill();
            let _ = child.wait();
            panic!("signalled exec did not exit");
        });
    let _ = join_pipe(stdout_reader, "stdout");
    let _ = join_pipe(stderr_reader, "stderr");

    assert!(
        !status.success(),
        "a signalled exec must exit nonzero (got {status:?})"
    );
    wait_for_pid_death(service_pid);
}
