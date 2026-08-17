//! Agent focus journey against the real binary in a real PTY.
//!
//! Two detached workers are spawned by a loopback provider and held open, so
//! they stay `running`. The probe then drives the keyboard contract exactly as
//! a user would: `←` on an empty composer enters the agent list, `Enter`
//! focuses a worker (its transcript owns the conversation area and the
//! composer chip names it), a typed follow-up is delivered to that worker and
//! the rail counts it as queued until the child's next round, `Esc` returns
//! to the main conversation which keeps a one-line receipt, and `↓` opens the
//! manage register.

#![cfg(unix)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::qa_harness::harness::{Harness, SealedWorkspace, make_sealed_workspace};
use crate::qa_harness::keys;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const BOOT_TIMEOUT: Duration = Duration::from_secs(20);
const INTERACTION_TIMEOUT: Duration = Duration::from_secs(20);
const PASTE_GUARD_SETTLE: Duration = Duration::from_millis(180);
const COMPOSER_READY_TEXT: &str = "Write a task";
const MODEL: &str = "deepseek-v4-pro";
const PARENT_PROMPT: &str = "spawn the focus probe workers now";
const CHILD_MARKER: &str = "focusprobe";
/// Empty-composer hint while a worker is focused.
const FOCUS_MARKER: &str = "Esc returns to main";
const FOLLOW_UP: &str = "focus-follow-up-please-continue";

static AGENT_FOCUS_PTY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn sse_chunk(value: Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::to_string(&value).expect("SSE JSON")
    )
}

fn text_sse(text: &str) -> String {
    [
        sse_chunk(json!({
            "id": "chatcmpl-focus",
            "object": "chat.completion.chunk",
            "model": MODEL,
            "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-focus",
            "object": "chat.completion.chunk",
            "model": MODEL,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 4, "total_tokens": 16}
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

fn agent_tool_call_sse(count: usize) -> String {
    let tool_calls = (1..=count)
        .map(|worker| {
            json!({
                "index": worker - 1,
                "id": format!("call_focus_{worker}"),
                "type": "function",
                "function": {
                    "name": "agent",
                    "arguments": serde_json::to_string(&json!({
                        "action": "start",
                        "detached": true,
                        "prompt": format!("{CHILD_MARKER}{worker} keep working"),
                        "type": "explorer",
                        "fork_context": false,
                        "session_name": format!("focus-{worker}")
                    }))
                    .expect("agent arguments")
                }
            })
        })
        .collect::<Vec<_>>();
    [
        sse_chunk(json!({
            "id": "chatcmpl-focus-fanout",
            "object": "chat.completion.chunk",
            "model": MODEL,
            "choices": [{"index": 0, "delta": {"tool_calls": tool_calls}, "finish_reason": null}]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-focus-fanout",
            "object": "chat.completion.chunk",
            "model": MODEL,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 20, "completion_tokens": 12, "total_tokens": 32}
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

fn chat_message_response(text: &str) -> ResponseTemplate {
    json_response(json!({
        "id": "chatcmpl-focus-child",
        "object": "chat.completion",
        "model": MODEL,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 12, "completion_tokens": 4, "total_tokens": 16}
    }))
}

async fn mount_models(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(json_response(json!({
            "object": "list",
            "data": [{"id": MODEL, "object": "model"}]
        })))
        .mount(server)
        .await;
}

struct ProbeResponder {
    child_requests: Arc<AtomicUsize>,
    parent_turns: Arc<AtomicUsize>,
    workers: usize,
    child_hold: Duration,
}

impl Respond for ProbeResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let raw = request
            .body_json::<Value>()
            .unwrap_or(Value::Null)
            .to_string();
        if raw.contains(CHILD_MARKER) && !raw.contains(PARENT_PROMPT) {
            self.child_requests.fetch_add(1, Ordering::SeqCst);
            return chat_message_response("focus child receipt").set_delay(self.child_hold);
        }
        if raw.contains(PARENT_PROMPT) {
            if self.parent_turns.fetch_add(1, Ordering::SeqCst) == 0 {
                return sse_response(agent_tool_call_sse(self.workers));
            }
            return sse_response(text_sse("focus parent wrapped up"));
        }
        sse_response(text_sse("unexpected-request"))
    }
}

fn tui_builder(
    ws: &SealedWorkspace,
    server_uri: &str,
) -> crate::qa_harness::harness::HarnessBuilder {
    tui_builder_with_posture(ws, server_uri, true)
}

fn tui_builder_with_posture(
    ws: &SealedWorkspace,
    server_uri: &str,
    yolo: bool,
) -> crate::qa_harness::harness::HarnessBuilder {
    let mut args = vec![
        "--workspace".to_string(),
        ws.workspace()
            .to_str()
            .expect("utf-8 workspace path")
            .to_string(),
        "--no-project-config".to_string(),
        "--skip-onboarding".to_string(),
    ];
    if yolo {
        args.push("--yolo".to_string());
    }
    args.extend(["--max-subagents".to_string(), "2".to_string()]);
    Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("RUST_LOG", "warn")
        .env("NO_ANIMATIONS", "1")
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", server_uri.to_string())
        .env("DEEPSEEK_MODEL", MODEL)
        .args(args)
        .size(pty_rows(), pty_cols())
}

/// Optional capture/size overrides so the same journey can be run at other
/// terminal sizes and leave real frames behind as evidence:
/// `CODEWHALE_FOCUS_PTY_SIZE=16x60 CODEWHALE_FOCUS_CAPTURE_DIR=/tmp/caps`.
fn pty_size() -> (u16, u16) {
    std::env::var("CODEWHALE_FOCUS_PTY_SIZE")
        .ok()
        .and_then(|raw| {
            let (rows, cols) = raw.split_once('x')?;
            Some((rows.trim().parse().ok()?, cols.trim().parse().ok()?))
        })
        .unwrap_or((42, 150))
}

fn pty_rows() -> u16 {
    pty_size().0
}

fn pty_cols() -> u16 {
    pty_size().1
}

fn capture(harness: &mut Harness, name: &str) {
    let Ok(dir) = std::env::var("CODEWHALE_FOCUS_CAPTURE_DIR") else {
        return;
    };
    let (rows, cols) = pty_size();
    let dir = std::path::Path::new(&dir).join(format!("{rows}x{cols}"));
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("{name}.txt")), harness.debug_dump());
}

fn wait_for_counter(
    harness: &mut Harness,
    counter: &AtomicUsize,
    expected: usize,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        harness.pump();
        if counter.load(Ordering::SeqCst) >= expected {
            return Ok(());
        }
        if let Some(code) = harness.wait_for_exit(Duration::from_millis(0)) {
            return Err(anyhow!(
                "codewhale-tui exited with {code} before the counter reached {expected}\n{}",
                harness.debug_dump()
            ));
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "counter did not reach {expected} within {timeout:?}; observed {}\n{}",
                counter.load(Ordering::SeqCst),
                harness.debug_dump()
            ));
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn type_and_submit(harness: &mut Harness, text: &str) -> Result<()> {
    harness.send(keys::key::text(text))?;
    harness.wait_for_text(text, Duration::from_secs(5))?;
    std::thread::sleep(PASTE_GUARD_SETTLE);
    harness.pump();
    harness.send(keys::key::enter())?;
    Ok(())
}

/// `←` then Enter after the parent turn has settled on a still-running row.
///
/// A 200ms sleep between those keys is a race: completion redraws drop rail
/// focus and Enter becomes a composer no-op. Wait for the live row, then
/// send the keys back-to-back.
fn focus_listed_worker(harness: &mut Harness, worker: &str) -> Result<()> {
    harness.wait_for_text("for agents", INTERACTION_TIMEOUT)?;
    harness.wait_for_text("to manage", INTERACTION_TIMEOUT)?;
    harness.wait_for_text(COMPOSER_READY_TEXT, INTERACTION_TIMEOUT)?;
    harness.wait_for_text("✓ done", INTERACTION_TIMEOUT)?;
    harness.wait_for(
        |frame| {
            let text = frame.text();
            text.contains(worker) && !text.contains("completed")
        },
        INTERACTION_TIMEOUT,
    )?;
    let mut chord = keys::key::left();
    chord.extend(keys::key::enter());
    harness.send(chord)?;
    harness
        .wait_for_text(FOCUS_MARKER, INTERACTION_TIMEOUT)
        .map_err(|_| {
            anyhow!(
                "← then Enter did not focus the worker\n{}",
                harness.debug_dump()
            )
        })?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn focus_a_worker_send_a_follow_up_and_return_to_main() -> Result<()> {
    let _guard = AGENT_FOCUS_PTY_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server).await;
    let child_requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ProbeResponder {
            child_requests: Arc::clone(&child_requests),
            parent_turns: Arc::new(AtomicUsize::new(0)),
            workers: 2,
            child_hold: Duration::from_secs(40),
        })
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        "[subagents]\nmax_concurrent = 2\nlaunch_concurrency = 2\nmax_admitted = 2\n",
    )?;
    let mut tui = tui_builder(&ws, &server.uri()).spawn()?;
    tui.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    type_and_submit(&mut tui, PARENT_PROMPT)?;
    wait_for_counter(&mut tui, &child_requests, 2, INTERACTION_TIMEOUT)?;
    // Wait until the parent turn has wrapped so the composer is idle and the
    // footer advertises the agent keys.
    tui.wait_for_text("focus parent wrapped up", INTERACTION_TIMEOUT)?;
    tui.wait_for_text("for agents", INTERACTION_TIMEOUT)
        .map_err(|_| {
            anyhow!(
                "footer never advertised `← for agents` while workers exist\n{}",
                tui.debug_dump()
            )
        })?;
    tui.wait_for_text("to manage", Duration::from_secs(2))?;
    capture(&mut tui, "01-main-with-workers");

    // ← enters the agent list; Enter focuses the selected worker.
    tui.send(b"\x1b[D")?; // Left
    std::thread::sleep(Duration::from_millis(200));
    tui.pump();
    capture(&mut tui, "02-agent-list-entered");
    tui.send(keys::key::enter())?;
    tui.wait_for_text(FOCUS_MARKER, Duration::from_secs(5))
        .map_err(|_| anyhow!("← then Enter did not focus a worker\n{}", tui.debug_dump()))?;
    // The composer chip names the addressed fork and the rail marks the row.
    let focused = tui.debug_dump();
    assert!(
        focused.contains("→ ") && focused.contains("❯ "),
        "focused frame lacks the composer chip or the rail marker:\n{focused}"
    );
    capture(&mut tui, "03-worker-focused");

    // A follow-up goes to that worker: echoed in the focused view, counted as
    // queued on its rail row (the child is mid-round, held by the provider),
    // and the main transcript keeps a one-line receipt.
    type_and_submit(&mut tui, FOLLOW_UP)?;
    tui.wait_for_text("1 queued", INTERACTION_TIMEOUT)
        .map_err(|_| {
            anyhow!(
                "rail row never counted the queued follow-up\n{}",
                tui.debug_dump()
            )
        })?;
    // The echoed message and its receipt sit at the tail of the focused view;
    // on a 16-row terminal only the receipt may still be on screen.
    tui.wait_for(
        |frame| {
            let text = frame.text();
            text.contains(FOLLOW_UP) || text.contains("Queued for")
        },
        Duration::from_secs(5),
    )?;
    capture(&mut tui, "04-follow-up-queued");

    // Esc on the empty composer returns to main; the receipt line stays.
    tui.send(keys::key::esc())?;
    tui.wait_for(
        |frame| !frame.text().contains(FOCUS_MARKER),
        Duration::from_secs(5),
    )
    .map_err(|_| anyhow!("Esc did not leave focus\n{}", tui.debug_dump()))?;
    tui.wait_for_text("Queued for", Duration::from_secs(5))
        .map_err(|_| {
            anyhow!(
                "main transcript lacks the queued receipt line\n{}",
                tui.debug_dump()
            )
        })?;

    capture(&mut tui, "05-back-to-main");

    // ↓ opens the manage register.
    tui.send(b"\x1b[B")?; // Down
    tui.wait_for_text("stop", Duration::from_secs(5))
        .map_err(|_| {
            anyhow!(
                "↓ did not open the agents register with its stop action\n{}",
                tui.debug_dump()
            )
        })?;
    capture(&mut tui, "06-manage-register");
    tui.send(keys::key::esc())?;

    let _ = tui.shutdown();
    Ok(())
}

// ---- Auto-Review inside a worker: the guardian decides, the receipt lands in the child's transcript ----

const GATE_PARENT_PROMPT: &str = "spawn the gate probe worker now";
const GATE_CHILD_MARKER: &str = "gateprobe";
const GATE_ECHO: &str = "gate-probe-output";

/// Serves the parent (streaming), the child (non-streaming: one `bash` call,
/// then a held wrap-up so the worker stays `running` while we focus), and the
/// Auto-Review guardian (non-streaming verdict) from one loopback endpoint,
/// telling them apart by body shape.
struct GateResponder {
    child_rounds: Arc<AtomicUsize>,
    guardian_requests: Arc<AtomicUsize>,
    parent_turns: Arc<AtomicUsize>,
    child_hold: Duration,
}

impl Respond for GateResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let raw = request
            .body_json::<Value>()
            .unwrap_or(Value::Null)
            .to_string();
        if raw.contains("proposed_tool_call") {
            self.guardian_requests.fetch_add(1, Ordering::SeqCst);
            return chat_message_response(
                r#"{"risk_level":"low","decision":"allow","reason":"echo through cat stays inside the workspace"}"#,
            );
        }
        if raw.contains(GATE_CHILD_MARKER) && !raw.contains(GATE_PARENT_PROMPT) {
            let round = self.child_rounds.fetch_add(1, Ordering::SeqCst);
            if round == 0 {
                return json_response(json!({
                    "id": "chatcmpl-gate-child",
                    "object": "chat.completion",
                    "model": MODEL,
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "call_gate_child_bash",
                                "type": "function",
                                "function": {
                                    "name": "bash",
                                    "arguments": serde_json::to_string(&json!({
                                        "command": format!("echo {GATE_ECHO} | cat")
                                    })).expect("bash arguments")
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {"prompt_tokens": 12, "completion_tokens": 6, "total_tokens": 18}
                }));
            }
            return chat_message_response("gate child wrapped up").set_delay(self.child_hold);
        }
        if raw.contains(GATE_PARENT_PROMPT) {
            if self.parent_turns.fetch_add(1, Ordering::SeqCst) == 0 {
                let tool_calls = vec![json!({
                    "index": 0,
                    "id": "call_gate_worker",
                    "type": "function",
                    "function": {
                        "name": "agent",
                        "arguments": serde_json::to_string(&json!({
                            "action": "start",
                            "detached": true,
                            "prompt": format!("{GATE_CHILD_MARKER} run the probe command"),
                            "type": "worker",
                            "fork_context": false,
                            "session_name": "gate-worker"
                        }))
                        .expect("agent arguments")
                    }
                })];
                return sse_response(
                    [
                        sse_chunk(json!({
                            "id": "chatcmpl-gate-fanout",
                            "object": "chat.completion.chunk",
                            "model": MODEL,
                            "choices": [{"index": 0, "delta": {"tool_calls": tool_calls}, "finish_reason": null}]
                        })),
                        sse_chunk(json!({
                            "id": "chatcmpl-gate-fanout",
                            "object": "chat.completion.chunk",
                            "model": MODEL,
                            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                            "usage": {"prompt_tokens": 20, "completion_tokens": 12, "total_tokens": 32}
                        })),
                        "data: [DONE]\n\n".to_string(),
                    ]
                    .join(""),
                );
            }
            return sse_response(text_sse("gate parent wrapped up"));
        }
        sse_response(text_sse("unexpected-request"))
    }
}

/// Auto-Review parity for a worker: the child's held `bash` call goes to the
/// same one-shot guardian the parent uses (one guardian request, no prompt),
/// runs after the allow, and the one-line receipt is visible when the worker
/// is focused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_review_gates_a_workers_call_and_the_receipt_shows_in_focus() -> Result<()> {
    let _guard = AGENT_FOCUS_PTY_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server).await;
    let child_rounds = Arc::new(AtomicUsize::new(0));
    let guardian_requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(GateResponder {
            child_rounds: Arc::clone(&child_rounds),
            guardian_requests: Arc::clone(&guardian_requests),
            parent_turns: Arc::new(AtomicUsize::new(0)),
            // Hold the wrap-up, not the bash call: the guardian receipt is
            // written when bash is allowed, and ← focuses a *running* row
            // the same way the sibling journey does.
            child_hold: Duration::from_secs(40),
        })
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        "[subagents]\nmax_concurrent = 2\nlaunch_concurrency = 2\nmax_admitted = 2\n",
    )?;
    std::fs::write(
        ws.home().join(".codewhale").join("settings.toml"),
        "locale = \"en\"\ndefault_mode = \"agent\"\npermission_posture = \"auto-review\"\nlow_motion = true\nfancy_animations = false\n",
    )?;
    let mut tui = tui_builder_with_posture(&ws, &server.uri(), false).spawn()?;
    tui.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    type_and_submit(&mut tui, GATE_PARENT_PROMPT)?;
    // Child bash → guardian → the call runs → child wrap-up request (held).
    wait_for_counter(&mut tui, &guardian_requests, 1, INTERACTION_TIMEOUT)?;
    wait_for_counter(&mut tui, &child_rounds, 2, INTERACTION_TIMEOUT)?;
    tui.wait_for_text("gate parent wrapped up", INTERACTION_TIMEOUT)?;
    // No approval prompt was ever opened for the child.
    assert!(
        !tui.debug_dump().contains("Allow once"),
        "Auto-Review must not prompt for a worker's call:\n{}",
        tui.debug_dump()
    );
    capture(&mut tui, "10-gate-main");

    // Focus the still-running worker: its transcript carries the receipt.
    focus_listed_worker(&mut tui, "gate-worker")?;
    tui.wait_for(
        |frame| {
            let text = frame.text();
            text.contains("Auto-Review allowed 'bash'") && text.contains("model guardian")
        },
        INTERACTION_TIMEOUT,
    )
    .map_err(|_| {
        anyhow!(
            "focused worker transcript lacks the guardian receipt\n{}",
            tui.debug_dump()
        )
    })?;
    capture(&mut tui, "11-gate-focused-receipt");
    tui.send(keys::key::esc())?;
    let _ = tui.shutdown();
    Ok(())
}
