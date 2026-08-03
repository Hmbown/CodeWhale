//! Process-level acceptance for the v0.9.4 Terminal-Bench P0 exit-path fix.
//!
//! Benchmark evidence (Terminal-Bench 2.1, codewhale 0.9.4): five tasks were
//! forfeited when the DeepSeek stream dropped mid-response ("error decoding
//! response body" after partial content). The engine surfaced the warning
//! and `codewhale exec` exited 1, and Harbor raised
//! `NonZeroAgentExitCodeError`. The fix: headless turns re-issue the request
//! after a mid-stream network drop (bounded by MAX_STREAM_RETRIES), and a
//! turn that still fails exits `EX_TEMPFAIL` (75) — a retryable
//! infrastructure failure the harness can distinguish from a genuine task
//! failure (exit 1).
//!
//! These tests drive the real binary against a raw TCP server that sends a
//! partial SSE body and then closes mid-`content-length`, reproducing the
//! exact reqwest decode failure from the bench artifacts.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use wait_timeout::ChildExt;

const MODEL: &str = "stream-drop-test";
/// The server claims a body far larger than it delivers on a "drop" response,
/// so hyper raises `error decoding response body` after the first SSE chunk —
/// the exact failure string in the Terminal-Bench crash artifacts.
const CLAIMED_DROP_BODY_LEN: usize = 1_048_576;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_exec_recovers_from_mid_stream_drop() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    let (base_url, chat_posts, _server) = start_flaky_server(1).await;

    let output = run_exec(workspace.path(), home.path(), &base_url);

    assert!(
        output.status.success(),
        "a recovered stream drop must exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        chat_posts.load(Ordering::SeqCst),
        2,
        "the dropped attempt must be re-issued exactly once"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("recovered after retry"),
        "the retried turn's content must stream: {stdout}"
    );
    assert!(
        !stdout.contains(r#""type":"error""#),
        "a transient drop that the retry recovers must not surface an error event: {stdout}"
    );
    let meta = terminal_metadata(&stdout);
    assert_eq!(
        meta["meta"]["status"].as_str(),
        Some("completed"),
        "recovered run must record a completed terminal receipt: {meta}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_exec_exits_ex_tempfail_after_drop_budget_exhausted() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    // More drops than the engine can consume: initial attempt +
    // MAX_STREAM_RETRIES (3) resumes, then the turn must fail.
    let (base_url, chat_posts, _server) = start_flaky_server(usize::MAX).await;

    let output = run_exec(workspace.path(), home.path(), &base_url);

    assert_eq!(
        output.status.code(),
        Some(75),
        "retry budget exhausted on a network-class failure must exit EX_TEMPFAIL (75), \
         not the generic task-failure 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        chat_posts.load(Ordering::SeqCst),
        4,
        "initial attempt plus the bounded resume budget (MAX_STREAM_RETRIES = 3)"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let error_events = stdout
        .lines()
        .filter(|line| line.contains(r#""type":"error""#))
        .count();
    assert_eq!(
        error_events, 1,
        "only the final, budget-exhausted attempt may emit an error event: {stdout}"
    );
    let error_line = stdout
        .lines()
        .find(|line| line.contains(r#""type":"error""#))
        .expect("terminal error event");
    assert!(
        error_line.contains("Provider stream connection dropped"),
        "the error channel must carry the real failure: {error_line}"
    );
    let meta = terminal_metadata(&stdout);
    assert_eq!(meta["meta"]["status"].as_str(), Some("failed"), "{meta}");
    assert_eq!(
        meta["meta"]["error_category"].as_str(),
        Some("network"),
        "the terminal receipt must classify the failure as retryable infra: {meta}"
    );
}

/// Extract the terminal `metadata` event from the stream-json stdout.
fn terminal_metadata(stdout: &str) -> Value {
    let line = stdout
        .lines()
        .rev()
        .find(|line| line.contains(r#""type":"metadata""#))
        .unwrap_or_else(|| panic!("stream-json metadata event missing: {stdout}"));
    serde_json::from_str(line).expect("metadata event is valid JSON")
}

/// Spawn a raw HTTP server that answers `GET /v1/models` and, for every POST
/// to the chat endpoint, either truncates the SSE body mid-stream (the first
/// `drops` requests) or completes a normal text turn. Returns the base URL
/// and a counter of chat-completion POSTs.
async fn start_flaky_server(
    drops: usize,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind flaky server");
    let addr = listener.local_addr().expect("flaky server addr");
    let chat_posts = Arc::new(AtomicUsize::new(0));
    let server_posts = Arc::clone(&chat_posts);
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let request = match read_http_request(&mut socket).await {
                Some(request) => request,
                None => continue,
            };
            if request.starts_with("GET ") {
                write_all(
                    &mut socket,
                    &http_response(
                        "application/json",
                        &json!({"object":"list","data":[{"id":MODEL,"object":"model"}]})
                            .to_string(),
                    ),
                )
                .await;
                continue;
            }
            let call = server_posts.fetch_add(1, Ordering::SeqCst) + 1;
            if call <= drops {
                // Partial SSE (one real content chunk), then the socket
                // closes with the declared content-length unmet — the
                // production "Provider stream connection dropped" failure.
                let partial = sse_chunk(
                    json!({"id":"drop","object":"chat.completion.chunk","model":MODEL,"choices":[{"index":0,"delta":{"content":"partial answer that must be discarded"},"finish_reason":null}]}),
                );
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {CLAIMED_DROP_BODY_LEN}\r\nconnection: close\r\n\r\n"
                );
                write_all(&mut socket, &format!("{head}{partial}")).await;
            } else {
                let body = [
                    sse_chunk(json!({"id":"final","object":"chat.completion.chunk","model":MODEL,"choices":[{"index":0,"delta":{"content":"recovered after retry"},"finish_reason":null}]})),
                    sse_chunk(json!({"id":"final","object":"chat.completion.chunk","model":MODEL,"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":20,"completion_tokens":2,"total_tokens":22}})),
                    "data: [DONE]\n\n".to_string(),
                ]
                .join("");
                write_all(&mut socket, &http_response("text/event-stream", &body)).await;
            }
        }
    });
    (format!("http://{addr}/v1"), chat_posts, task)
}

/// Read one HTTP request (headers plus the content-length body, when any).
async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Option<String> {
    use tokio::io::AsyncReadExt;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_subslice(&buffer, b"\r\n\r\n") {
            break pos + 4;
        }
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > 1 << 20 {
            return None;
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Some(headers)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn write_all(socket: &mut tokio::net::TcpStream, bytes: &str) {
    use tokio::io::AsyncWriteExt;
    let _ = socket.write_all(bytes.as_bytes()).await;
    let _ = socket.shutdown().await;
}

fn http_response(content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn sse_chunk(value: Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::to_string(&value).expect("SSE JSON")
    )
}

fn run_exec(workspace: &Path, home: &Path, base_url: &str) -> std::process::Output {
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
        .arg("answer briefly")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("CODEWHALE_CONFIG_PATH", home.join(".codewhale/config.toml"))
        .env("DEEPSEEK_CONFIG_PATH", home.join(".deepseek/config.toml"))
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", base_url)
        .env("CODEWHALE_BASE_URL", base_url)
        .env("DEEPSEEK_MODEL", MODEL)
        .env("CODEWHALE_MODEL", MODEL)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_with_timeout(command, Duration::from_secs(45))
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
