//! Local-only release runtime QA through real pseudo-terminals.
//!
//! These scenarios cover the live TUI checks that unit tests cannot prove:
//! six-worker fanout liveness/cancellation, multi-terminal route isolation,
//! and the explicit Enter-queue / Ctrl+Enter-steer contract. Every provider is a loopback wiremock
//! server and every process receives a sealed HOME.

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
const INTERACTION_TIMEOUT: Duration = Duration::from_secs(15);
const PASTE_GUARD_SETTLE: Duration = Duration::from_millis(180);
const COMPOSER_READY_TEXT: &str = "Write a task";
const MUSE_MODEL: &str = "muse-spark-1.1";
const GPT_MODEL: &str = "gpt-5.6-terra";
const DEEPSEEK_TEST_MODEL: &str = "deepseek-v4-pro";
static RELEASE_RUNTIME_QA_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn sse_chunk(value: Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::to_string(&value).expect("SSE JSON")
    )
}

fn text_sse(model: &str, text: &str) -> String {
    [
        sse_chunk(json!({
            "id": "chatcmpl-local-qa",
            "object": "chat.completion.chunk",
            "model": model,
            "choices": [{
                "index": 0,
                "delta": { "content": text },
                "finish_reason": null
            }]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-local-qa",
            "object": "chat.completion.chunk",
            "model": model,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 12,
                "completion_tokens": 4,
                "total_tokens": 16
            }
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

fn fanout_tool_call_sse() -> String {
    fanout_tool_call_sse_n(6)
}

fn fanout_tool_call_sse_n(count: usize) -> String {
    let tool_calls = (1..=count)
        .map(|worker| {
            json!({
                "index": worker - 1,
                "id": format!("call_agent_{worker}"),
                "type": "function",
                "function": {
                    "name": "agent",
                    "arguments": serde_json::to_string(&json!({
                        "message": format!("stay busy worker {worker} until the parent QA turn is cancelled"),
                        "agent_type": "explorer",
                        // Explicit fresh context: this harness dispatches mock
                        // responses on request content, and an auto-forked
                        // child would carry the parent conversation (including
                        // the parent prompt) in its requests. Explicit false
                        // always wins over the auto-fork policy.
                        "fork_context": false,
                        "session_name": format!("qa-worker-{worker}")
                    }))
                    .expect("agent arguments")
                }
            })
        })
        .collect::<Vec<_>>();

    [
        sse_chunk(json!({
            "id": "chatcmpl-fanout",
            "object": "chat.completion.chunk",
            "model": DEEPSEEK_TEST_MODEL,
            "choices": [{
                "index": 0,
                "delta": { "tool_calls": tool_calls },
                "finish_reason": null
            }]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-fanout",
            "object": "chat.completion.chunk",
            "model": DEEPSEEK_TEST_MODEL,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 12,
                "total_tokens": 32
            }
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

fn fleet_role_tool_call_sse() -> String {
    let roles = ["worker", "scout", "reviewer", "verifier"];
    let tool_calls = roles
        .iter()
        .enumerate()
        .map(|(index, role)| {
            json!({
                "index": index,
                "id": format!("call_role_{role}"),
                "type": "function",
                "function": {
                    "name": "agent",
                    "arguments": serde_json::to_string(&json!({
                        "action": "start",
                        "prompt": format!("role-probe-{role}"),
                        "type": role,
                        "fork_context": false,
                        "session_name": format!("qa-{role}"),
                        "workspace_policy": "shared",
                        "write_authority": "read_only",
                        "expected_artifact": "one role launch receipt",
                        "deliberate": true
                    }))
                    .expect("Fleet role arguments")
                }
            })
        })
        .collect::<Vec<_>>();

    [
        sse_chunk(json!({
            "id": "chatcmpl-fleet-roles",
            "object": "chat.completion.chunk",
            "model": DEEPSEEK_TEST_MODEL,
            "choices": [{
                "index": 0,
                "delta": { "tool_calls": tool_calls },
                "finish_reason": null
            }]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-fleet-roles",
            "object": "chat.completion.chunk",
            "model": DEEPSEEK_TEST_MODEL,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 12,
                "total_tokens": 32
            }
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

async fn mount_models(server: &MockServer, models: &[&str]) {
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(json_response(json!({
            "object": "list",
            "data": models
                .iter()
                .map(|model| json!({ "id": model, "object": "model" }))
                .collect::<Vec<_>>()
        })))
        .mount(server)
        .await;
}

async fn mount_text_model(server: &MockServer, model: &str, answer: &str) {
    mount_models(server, &[model]).await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(text_sse(model, answer)))
        .mount(server)
        .await;
}

fn common_tui_builder(ws: &SealedWorkspace) -> crate::qa_harness::harness::HarnessBuilder {
    Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
        ])
        .size(42, 150)
}

/// Release scenarios exercise the direct-session runtime. The optional launch
/// screen is not enabled in these sealed homes.
fn enter_launch_session(harness: &mut Harness) -> Result<()> {
    harness.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    Ok(())
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
        // A dead child renders as a hang: `pump()` only feeds *new* bytes into a
        // retained frame, so `debug_dump()` keeps painting the last frame the TUI
        // drew before it died. Without this probe a SIGABRT (stack overflow aborts
        // as 134) burns the whole timeout and then reports a live-looking screen.
        if let Some(code) = harness.wait_for_exit(Duration::from_millis(0)) {
            return Err(anyhow!(
                "TUI exited with {code} before the counter reached {expected}; observed {}\n{}",
                counter.load(Ordering::SeqCst),
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
    // Rapid PTY writes intentionally exercise paste-burst detection. Wait
    // beyond its 120 ms trailing-Enter suppression window before submitting.
    // Ambient ocean life keeps repainting even when the runtime is idle, so
    // visual frame stability is not a valid readiness signal.
    harness.wait_for_text(text, Duration::from_secs(3))?;
    std::thread::sleep(PASTE_GUARD_SETTLE);
    harness.pump();
    harness.send(keys::key::enter())?;
    Ok(())
}

const COMPACTION_SUMMARY: &str = "1. Primary request and intent — preserve the exact active task across compaction.\n\
2. Key technical concepts — deterministic PTY lifecycle coverage and bounded engine mailboxes.\n\
3. Files and code sections — the release runtime QA harness owns this loopback proof.\n\
4. Errors and fixes — None.\n\
5. Problem solving — keep provider responses local and hold them long enough to outlive toast expiry.\n\
6. User messages — continue the active release verification without losing state.\n\
7. Pending tasks — finish the focused release gates.\n\
8. Current work — proving serialized compaction and persistent lifecycle labels.\n\
9. Next step — verify the rebuilt binary.";

#[derive(Clone)]
struct CompactionLifecycleResponder {
    stream_requests: Arc<AtomicUsize>,
    compaction_requests: Arc<AtomicUsize>,
    /// Compaction requests that carried the retired system-prompt bridge.
    /// Checkpoints now live once in ordinary history, Codex-style.
    legacy_system_bridge_requests: Arc<AtomicUsize>,
    successor_stream_requests: Arc<AtomicUsize>,
    order: Arc<std::sync::Mutex<Vec<&'static str>>>,
    stream_delay: Duration,
    compaction_delay: Duration,
}

impl CompactionLifecycleResponder {
    fn new(stream_delay: Duration, compaction_delay: Duration) -> Self {
        Self {
            stream_requests: Arc::new(AtomicUsize::new(0)),
            compaction_requests: Arc::new(AtomicUsize::new(0)),
            legacy_system_bridge_requests: Arc::new(AtomicUsize::new(0)),
            successor_stream_requests: Arc::new(AtomicUsize::new(0)),
            order: Arc::new(std::sync::Mutex::new(Vec::new())),
            stream_delay,
            compaction_delay,
        }
    }

    fn request_order(&self) -> Vec<&'static str> {
        self.order
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    fn record(&self, kind: &'static str) {
        self.order
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(kind);
    }
}

impl Respond for CompactionLifecycleResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body = request.body_json::<Value>().unwrap_or(Value::Null);
        let raw = body.to_string();
        // Prepared non-streaming requests may omit `stream: false` on the
        // OpenAI wire. The compact-prompt instruction is the compactor's
        // stable semantic discriminator and cannot occur in these test turns.
        let is_compaction = body.get("stream").and_then(Value::as_bool) == Some(false)
            || raw.contains("You are performing a context checkpoint compaction");
        if is_compaction {
            self.compaction_requests.fetch_add(1, Ordering::SeqCst);
            if raw.contains("A previous context checkpoint produced the summary below") {
                self.legacy_system_bridge_requests
                    .fetch_add(1, Ordering::SeqCst);
            }
            self.record("compact");
            return json_response(json!({
                "id": "chatcmpl-compact-pty",
                "object": "chat.completion",
                "created": 0,
                "model": DEEPSEEK_TEST_MODEL,
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": COMPACTION_SUMMARY},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 128,
                    "completion_tokens": 64,
                    "total_tokens": 192
                }
            }))
            .set_delay(self.compaction_delay);
        }

        self.stream_requests.fetch_add(1, Ordering::SeqCst);
        if raw.contains("preserve the exact active task across compaction") {
            self.successor_stream_requests
                .fetch_add(1, Ordering::SeqCst);
        }
        self.record("stream");
        sse_response(text_sse(DEEPSEEK_TEST_MODEL, "loopback turn completed"))
            .set_delay(self.stream_delay)
    }
}

async fn mount_compaction_responder(server: &MockServer, responder: CompactionLifecycleResponder) {
    mount_models(server, &[DEEPSEEK_TEST_MODEL]).await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(responder)
        .mount(server)
        .await;
}

fn write_auto_compaction_settings(
    ws: &SealedWorkspace,
    enabled: bool,
    threshold_percent: u8,
) -> Result<()> {
    std::fs::write(
        ws.home().join(".codewhale").join("settings.toml"),
        format!("auto_compact = {enabled}\nauto_compact_threshold_percent = {threshold_percent}\n"),
    )?;
    Ok(())
}

fn write_compaction_session(
    ws: &SealedWorkspace,
    id: &str,
    message_count: usize,
    message_chars: usize,
) -> Result<std::path::PathBuf> {
    let messages = (0..message_count)
        .map(|index| {
            let mut text = format!("ordinary archive dialogue item {index:02}: ");
            text.push_str(
                &"bounded context pressure evidence "
                    .repeat(message_chars.saturating_div(34).saturating_add(1)),
            );
            text.truncate(message_chars.max(48));
            json!({
                "role": if index % 2 == 0 { "user" } else { "assistant" },
                "content": [{"type": "text", "text": text, "cache_control": null}]
            })
        })
        .collect::<Vec<_>>();
    let session_path = ws.workspace().join(format!("{id}.json"));
    std::fs::write(
        &session_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "metadata": {
                "id": id,
                "title": format!("Compaction PTY {id}"),
                "created_at": "2026-08-08T00:00:00Z",
                "updated_at": "2026-08-08T00:00:00Z",
                "message_count": messages.len(),
                "total_tokens": message_count.saturating_mul(message_chars).saturating_div(4),
                "model": DEEPSEEK_TEST_MODEL,
                "model_provider": "deepseek",
                "workspace": ws.workspace(),
                "mode": "agent",
                "cost": {},
                "cumulative_turn_secs": 0
            },
            "messages": messages,
            "system_prompt": null,
            "work_state": null
        }))?,
    )?;
    Ok(session_path)
}

fn load_session(harness: &mut Harness, path: &std::path::Path, message_count: usize) -> Result<()> {
    type_and_submit(harness, &format!("/load {}", path.to_string_lossy()))?;
    // The loaded-session note wraps when the temp path is long, splitting the
    // needle across rows, a gutter prefix, and the scrollbar edge glyph. Match
    // on frame text with chrome glyphs removed and whitespace normalized so
    // the wait survives any wrap point.
    let needle = format!("{message_count} messages");
    harness.wait_for(
        move |frame| {
            let normalized = frame
                .text()
                .replace(['▏', '▎', '▌', '│', '┃', '●'], " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            normalized.contains(&needle)
        },
        INTERACTION_TIMEOUT,
    )?;
    Ok(())
}

fn pump_for(harness: &mut Harness, duration: Duration) -> Result<()> {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        harness.pump();
        if let Some(code) = harness.wait_for_exit(Duration::from_millis(0)) {
            return Err(anyhow!(
                "TUI exited with {code} during a bounded lifecycle hold\n{}",
                harness.debug_dump()
            ));
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    harness.pump();
    Ok(())
}

fn compaction_tui_builder(
    ws: &SealedWorkspace,
    server: &MockServer,
) -> crate::qa_harness::harness::HarnessBuilder {
    common_tui_builder(ws)
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", DEEPSEEK_TEST_MODEL)
        .env("NO_ANIMATIONS", "1")
}

/// Regression for the v0.9.6 `/compact` freeze, upgraded to the v0.9.7
/// queue-behind-pressure contract: a live turn stops draining the bounded
/// engine op channel and `/subagents` refresh fills all 32 slots. The manual
/// compaction shortcut must still return immediately — and now queues behind
/// the saturated mailbox instead of refusing with "engine is busy". The final
/// marker proves keyboard input and rendering remain live afterward.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_compaction_full_mailbox_queues_and_never_freezes_tui() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    let responder =
        CompactionLifecycleResponder::new(Duration::from_secs(30), Duration::from_millis(0));
    mount_compaction_responder(&server, responder.clone()).await;

    let ws = make_sealed_workspace()?;
    write_auto_compaction_settings(&ws, false, 80)?;
    let mut tui = compaction_tui_builder(&ws, &server).spawn()?;
    enter_launch_session(&mut tui)?;

    type_and_submit(&mut tui, "hold the engine turn for mailbox saturation")?;
    wait_for_counter(&mut tui, &responder.stream_requests, 1, INTERACTION_TIMEOUT)?;
    type_and_submit(&mut tui, "/subagents")?;
    tui.wait_for_text("No Fleet workers running.", Duration::from_secs(5))?;

    // One ListSubAgents op came from opening the view. Forty-eight more refresh
    // keys leave ample headroom over the engine's 32-slot bounded channel even
    // if a terminal or scheduler coalesces a few writes.
    for _ in 0..48 {
        tui.send(keys::key::ch('r'))?;
        tui.pump();
        std::thread::sleep(Duration::from_millis(12));
    }
    pump_for(&mut tui, Duration::from_millis(250))?;
    tui.send(keys::key::esc())?;
    tui.wait_for(
        |frame| !frame.contains("No Fleet workers running."),
        Duration::from_secs(3),
    )?;

    let compact_started = Instant::now();
    tui.send(keys::key::ctrl('l'))?;
    tui.wait_for_text("Context compaction queued", Duration::from_secs(3))?;
    assert!(
        compact_started.elapsed() < Duration::from_secs(3),
        "full-mailbox compaction did not return within the liveness budget"
    );
    // The queued request may not start while the saturating turn holds the
    // mailbox; the deferred send waits for a free slot instead of racing it.
    assert_eq!(
        responder.compaction_requests.load(Ordering::SeqCst),
        0,
        "a queued manual request must wait behind the saturating turn"
    );
    // A repeat during deferral is one queued pass, not a second receipt path.
    tui.send(keys::key::ctrl('l'))?;
    tui.wait_for_text(
        "Context compaction is already in progress.",
        Duration::from_secs(3),
    )?;

    tui.send(keys::key::text("post-compact-full-mailbox-live"))?;
    tui.wait_for_text("post-compact-full-mailbox-live", Duration::from_secs(3))?;

    let _ = tui.shutdown();
    Ok(())
}

/// A manual request accepted during a live turn is serialized behind that
/// turn, starts exactly one provider compaction request, and owns its typed
/// phase label beyond the five-second toast lifetime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_manual_compaction_serializes_and_label_persists() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    let responder =
        CompactionLifecycleResponder::new(Duration::from_secs(3), Duration::from_secs(9));
    mount_compaction_responder(&server, responder.clone()).await;

    let ws = make_sealed_workspace()?;
    write_auto_compaction_settings(&ws, false, 80)?;
    let session_path = write_compaction_session(&ws, "manual-compaction-pty", 12, 600)?;
    let mut tui = compaction_tui_builder(&ws, &server).spawn()?;
    enter_launch_session(&mut tui)?;
    load_session(&mut tui, &session_path, 12)?;

    type_and_submit(
        &mut tui,
        "keep this turn active while manual compaction queues",
    )?;
    wait_for_counter(&mut tui, &responder.stream_requests, 1, INTERACTION_TIMEOUT)?;
    tui.send(keys::key::ctrl('l'))?;
    tui.wait_for_text(
        "Context compaction queued; it will run after the active turn.",
        Duration::from_secs(3),
    )?;
    tui.send(keys::key::ctrl('l'))?;
    tui.wait_for_text(
        "Context compaction is already in progress.",
        Duration::from_secs(3),
    )?;

    wait_for_counter(
        &mut tui,
        &responder.compaction_requests,
        1,
        INTERACTION_TIMEOUT,
    )?;
    tui.wait_for_text("Compacting context…", Duration::from_secs(5))?;
    pump_for(&mut tui, Duration::from_millis(5_500))?;
    assert!(
        tui.frame().contains("Compacting context…"),
        "manual lifecycle label expired with its five-second start toast:\n{}",
        tui.debug_dump()
    );
    tui.wait_for(
        |frame| !frame.contains("Compacting context…"),
        INTERACTION_TIMEOUT,
    )?;
    // The one-shot completion toast may be superseded by the preceding turn's
    // done phase before the next PTY draw. Prove the stronger state transition:
    // a subsequent provider request must carry the committed successor summary.
    type_and_submit(&mut tui, "post-compaction successor probe")?;
    wait_for_counter(&mut tui, &responder.stream_requests, 2, INTERACTION_TIMEOUT)?;
    wait_for_counter(
        &mut tui,
        &responder.successor_stream_requests,
        1,
        INTERACTION_TIMEOUT,
    )?;
    assert_eq!(
        responder.compaction_requests.load(Ordering::SeqCst),
        1,
        "repeated manual requests must serialize into one compaction pass"
    );
    assert_eq!(
        responder.request_order(),
        vec!["stream", "compact", "stream"],
        "manual compaction must wait behind the active turn and commit before the successor turn"
    );

    let _ = tui.shutdown();
    Ok(())
}

/// Auto-compaction uses full conservative request pressure on a configured
/// 272K route. Its typed auto label must remain visible beyond toast expiry,
/// and the ordinary streamed turn may start only after compaction completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_auto_compaction_label_persists() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    let responder = CompactionLifecycleResponder::new(Duration::ZERO, Duration::from_secs(9));
    mount_compaction_responder(&server, responder.clone()).await;

    let ws = make_sealed_workspace()?;
    write_auto_compaction_settings(&ws, true, 25)?;
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        "provider = \"deepseek\"\n[providers.deepseek]\ncontext_window = 272000\n",
    )?;
    // 24 × 14K chars ≈ 84K plain-estimate tokens, over the 68K trigger
    // (25% of the configured 272K window).
    let session_path = write_compaction_session(&ws, "auto-compaction-pty", 24, 14_000)?;
    let mut tui = compaction_tui_builder(&ws, &server).spawn()?;
    enter_launch_session(&mut tui)?;
    load_session(&mut tui, &session_path, 24)?;

    type_and_submit(&mut tui, "trigger the pressure boundary")?;
    wait_for_counter(
        &mut tui,
        &responder.compaction_requests,
        1,
        INTERACTION_TIMEOUT,
    )?;
    assert_eq!(
        responder.stream_requests.load(Ordering::SeqCst),
        0,
        "the provider turn must not start before automatic compaction"
    );
    tui.wait_for_text("Context automatically compacting…", Duration::from_secs(5))?;
    pump_for(&mut tui, Duration::from_millis(5_500))?;
    assert!(
        tui.frame().contains("Context automatically compacting…"),
        "auto lifecycle label expired with its five-second start toast:\n{}",
        tui.debug_dump()
    );
    wait_for_counter(&mut tui, &responder.stream_requests, 1, INTERACTION_TIMEOUT)?;
    wait_for_counter(
        &mut tui,
        &responder.successor_stream_requests,
        1,
        INTERACTION_TIMEOUT,
    )?;
    tui.wait_for(
        |frame| !frame.contains("Context automatically compacting…"),
        Duration::from_secs(5),
    )?;
    assert!(
        !tui.frame().contains("Context automatically compacting…"),
        "auto lifecycle label must clear after its matching completion:\n{}",
        tui.debug_dump()
    );
    assert_eq!(
        responder.request_order(),
        vec!["compact", "stream"],
        "automatic compaction must finish before the ordinary provider turn"
    );

    let _ = tui.shutdown();
    Ok(())
}

/// Regression for the v0.9.6 release blocker: an idle `/compact` ran and
/// committed engine-side, but its only feedback was a status toast that the
/// engine's immediately-following turn-complete status replaced within the
/// same event drain — the user saw nothing and reported `/compact` as dead.
/// The outcome must land in the transcript, where it survives later frames.
/// A second `/compact` must replace the checkpoint in ordinary history and
/// must not revive the retired system-prompt bridge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_idle_compaction_reports_outcome_in_transcript() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    let responder = CompactionLifecycleResponder::new(Duration::ZERO, Duration::ZERO);
    mount_compaction_responder(&server, responder.clone()).await;

    let ws = make_sealed_workspace()?;
    write_auto_compaction_settings(&ws, false, 80)?;
    let session_path = write_compaction_session(&ws, "idle-compaction-pty", 12, 600)?;
    let mut tui = compaction_tui_builder(&ws, &server).spawn()?;
    enter_launch_session(&mut tui)?;
    load_session(&mut tui, &session_path, 12)?;

    tui.send(keys::key::ctrl('l'))?;
    wait_for_counter(
        &mut tui,
        &responder.compaction_requests,
        1,
        INTERACTION_TIMEOUT,
    )?;
    tui.wait_for_text("Compaction complete:", INTERACTION_TIMEOUT)?;
    // Transcript receipt, not a toast: it must outlive the five-second toast
    // lifetime and the turn-complete footer transition.
    pump_for(&mut tui, Duration::from_millis(5_500))?;
    assert!(
        tui.frame().contains("Compaction complete:"),
        "compaction outcome must survive as a transcript receipt:\n{}",
        tui.debug_dump()
    );

    tui.send(keys::key::ctrl('l'))?;
    wait_for_counter(
        &mut tui,
        &responder.compaction_requests,
        2,
        INTERACTION_TIMEOUT,
    )?;
    tui.wait_for_text("0 removed", INTERACTION_TIMEOUT)?;
    assert_eq!(
        responder
            .legacy_system_bridge_requests
            .load(Ordering::SeqCst),
        0,
        "repeat compaction must not reintroduce a system-prompt bridge"
    );

    let _ = tui.shutdown();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn underwater_footer_moves_from_working_through_one_shot_completion() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server, &[DEEPSEEK_TEST_MODEL]).await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            sse_response(text_sse(DEEPSEEK_TEST_MODEL, "local phase proof"))
                .set_delay(Duration::from_millis(850)),
        )
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    let mut tui = common_tui_builder(&ws)
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", DEEPSEEK_TEST_MODEL)
        .spawn()?;
    match tui.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("[dogfood] launch wait error debug: {e:?}");
            return Err(anyhow::anyhow!("launch wait failed: {e:#}"));
        }
    }

    type_and_submit(&mut tui, "show the underwater phase transition")?;
    // TUI-DOG-008: live phases (working/finishing/done) render on the phase
    // strip ABOVE the composer, so the bottom row is no longer the phase
    // owner. Assert the phase words anywhere in the frame — the mock reply
    // ("local phase proof") and the prompt contain none of them.
    tui.wait_for(|frame| frame.contains("working"), INTERACTION_TIMEOUT)?;
    tui.wait_for(
        |frame| frame.contains("finishing") || frame.contains("✓ done"),
        INTERACTION_TIMEOUT,
    )?;
    tui.wait_for(|frame| frame.contains("✓ done"), INTERACTION_TIMEOUT)?;

    let _ = tui.shutdown();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn underwater_theme_picker_emits_each_live_palette_to_the_terminal() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let ws = make_sealed_workspace()?;
    let mut tui = common_tui_builder(&ws)
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("DEEPSEEK_MODEL", DEEPSEEK_TEST_MODEL)
        .env("COLORTERM", "truecolor")
        .env("RUST_BACKTRACE", "1")
        .spawn()?;
    enter_launch_session(&mut tui)?;
    // A bracketed paste plus trailing space makes this an explicit command
    // invocation, outside both autocomplete and unbracketed burst handling.
    tui.paste("/theme ")?;
    tui.wait_for_text("/theme", Duration::from_secs(3))?;
    std::thread::sleep(PASTE_GUARD_SETTLE);
    tui.pump();
    tui.send(keys::key::enter())?;
    std::thread::sleep(Duration::from_millis(300));
    tui.pump();
    if let Some(status) = tui.wait_for_exit(Duration::from_millis(1)) {
        let logs = std::fs::read_dir(ws.home().join(".codewhale/logs"))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(anyhow!(
            "theme picker process exited with {status}:\n{}\nlogs:\n{logs}",
            tui.debug_dump(),
        ));
    }
    if tui
        .wait_for_text("live preview", Duration::from_secs(1))
        .is_err()
    {
        // A PTY can deliver the first Enter inside the paste guard's trailing
        // suppression window. Once that window expires, the next deliberate
        // Enter must execute the retained draft.
        std::thread::sleep(PASTE_GUARD_SETTLE);
        tui.pump();
        tui.send(keys::key::enter())?;
        tui.wait_for_text("live preview", INTERACTION_TIMEOUT)?;
    }

    let labels = [
        "System",
        "Terminal",
        "Blue Stage",
        "Blue Stage Light",
        "Grayscale",
        "Catppuccin Mocha",
        "Tokyo Night",
        "Dracula",
        "Gruvbox Dark",
        "Claude",
        "Matrix",
        "Solarized Light",
    ];
    let mut previous_signature = None;
    for (index, label) in labels.iter().enumerate() {
        let selected = format!("▸ {}.", index + 1);
        tui.wait_for(
            |frame| frame.text().contains(&selected),
            INTERACTION_TIMEOUT,
        )?;
        let frame = tui.frame();
        let signature = (
            frame.colors_at(0, 0).expect("theme surface cell"),
            frame
                .first_symbol_colors("▸")
                .expect("selected theme pointer cell"),
        );
        assert!(
            frame.text().contains(label),
            "missing theme row {label}:\n{}",
            frame.debug_dump()
        );
        if let Some(previous) = previous_signature {
            assert_ne!(
                signature,
                previous,
                "live ANSI palette did not change from {} to {label}",
                labels[index - 1]
            );
        }
        previous_signature = Some(signature);
        if index + 1 < labels.len() {
            tui.send(b"\x1b[B")?;
            std::thread::sleep(Duration::from_millis(250));
            tui.pump();
            if let Some(status) = tui.wait_for_exit(Duration::from_millis(1)) {
                let logs = std::fs::read_dir(ws.home().join(".codewhale/logs"))
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
                    .collect::<Vec<_>>()
                    .join("\n");
                return Err(anyhow!(
                    "theme preview exited with {status}:\n{}\nlogs:\n{logs}",
                    tui.debug_dump()
                ));
            }
        }
    }

    tui.send(b"\x1b")?;
    let _ = tui.shutdown();
    Ok(())
}

fn chat_requests(requests: &[Request]) -> Vec<Value> {
    requests
        .iter()
        .filter(|request| request.url.path().ends_with("/chat/completions"))
        .map(|request| request.body_json().expect("chat body JSON"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_multi_terminal_muse_and_gpt_routes_stay_isolated() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let meta_server = MockServer::start().await;
    let openai_server = MockServer::start().await;
    mount_text_model(&meta_server, MUSE_MODEL, "meta-route-ok").await;
    mount_models(&openai_server, &["gpt-5.6-luna", GPT_MODEL]).await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(text_sse(GPT_MODEL, "openai-route-ok")))
        .mount(&openai_server)
        .await;

    let ws = make_sealed_workspace()?;
    let openai_base_url = openai_server.uri();
    let meta_base_url = meta_server.uri();
    let shared_openai_env = [
        ("OPENAI_API_KEY", "openai-local-test-key"),
        ("OPENAI_BASE_URL", openai_base_url.as_str()),
        ("OPENAI_MODEL", "gpt-5.6-luna"),
    ];
    let shared_meta_env = [
        ("META_MODEL_API_KEY", "meta-local-test-key"),
        ("MODEL_API_KEY", "meta-local-test-key"),
        ("META_MODEL_API_BASE_URL", meta_base_url.as_str()),
        ("META_MODEL_API_MODEL", MUSE_MODEL),
    ];

    let mut meta_builder = common_tui_builder(&ws).env("CODEWHALE_PROVIDER", "meta");
    let mut openai_builder = common_tui_builder(&ws).env("CODEWHALE_PROVIDER", "openai");
    for (key, value) in shared_openai_env.into_iter().chain(shared_meta_env) {
        meta_builder = meta_builder.env(key, value);
        openai_builder = openai_builder.env(key, value);
    }

    let mut meta_tui = meta_builder.spawn()?;
    let mut openai_tui = openai_builder.spawn()?;
    enter_launch_session(&mut meta_tui)?;
    enter_launch_session(&mut openai_tui)?;

    // Change terminal B's model through the live command path while terminal A
    // remains open on Meta. Both processes share one sealed settings file.
    type_and_submit(&mut openai_tui, "/model gpt-5.6-terra")?;
    openai_tui.wait_for(
        |frame| frame.row(0).contains(GPT_MODEL),
        INTERACTION_TIMEOUT,
    )?;
    assert!(
        meta_tui.frame().contains(MUSE_MODEL),
        "terminal A route changed when terminal B selected a model:\n{}",
        meta_tui.debug_dump()
    );

    type_and_submit(&mut meta_tui, "route probe from meta terminal")?;
    type_and_submit(&mut openai_tui, "route probe from openai terminal")?;
    meta_tui.wait_for_text("meta-route-ok", INTERACTION_TIMEOUT)?;
    openai_tui.wait_for_text("openai-route-ok", INTERACTION_TIMEOUT)?;

    let meta_requests = meta_server.received_requests().await.unwrap_or_default();
    let openai_requests = openai_server.received_requests().await.unwrap_or_default();
    let meta_chat = chat_requests(&meta_requests);
    let openai_chat = chat_requests(&openai_requests);
    assert_eq!(
        meta_chat.len(),
        1,
        "unexpected Meta chat requests: {meta_chat:#?}"
    );
    assert_eq!(
        openai_chat.len(),
        1,
        "unexpected OpenAI chat requests: {openai_chat:#?}"
    );
    assert_eq!(meta_chat[0]["model"], MUSE_MODEL);
    assert_eq!(openai_chat[0]["model"], GPT_MODEL);
    assert!(
        meta_chat[0]
            .to_string()
            .contains("route probe from meta terminal")
    );
    assert!(!meta_chat[0].to_string().contains("openai terminal"));
    assert!(
        openai_chat[0]
            .to_string()
            .contains("route probe from openai terminal")
    );
    assert!(!openai_chat[0].to_string().contains("meta terminal"));

    let _ = meta_tui.shutdown();
    let _ = openai_tui.shutdown();
    Ok(())
}

#[derive(Clone)]
struct FanoutResponder {
    child_requests: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct FleetRoleResponder {
    launched: Arc<AtomicUsize>,
    canonical_prompts: Arc<AtomicUsize>,
    worker: Arc<AtomicUsize>,
    scout: Arc<AtomicUsize>,
    reviewer: Arc<AtomicUsize>,
    verifier: Arc<AtomicUsize>,
}

impl Respond for FleetRoleResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body = request.body_json::<Value>().unwrap_or(Value::Null);
        let raw = body.to_string();
        let role_markers = [
            ("role-probe-worker", "Fleet worker", &self.worker),
            ("role-probe-scout", "Fleet scout", &self.scout),
            ("role-probe-reviewer", "Fleet reviewer", &self.reviewer),
            ("role-probe-verifier", "Fleet verifier", &self.verifier),
        ];
        let matched = role_markers
            .iter()
            .filter(|(marker, _, _)| raw.contains(marker))
            .collect::<Vec<_>>();
        if matched.len() == 1 {
            let (_, expected_prompt, counter) = matched[0];
            self.launched.fetch_add(1, Ordering::SeqCst);
            counter.fetch_add(1, Ordering::SeqCst);
            if raw.contains(expected_prompt) {
                self.canonical_prompts.fetch_add(1, Ordering::SeqCst);
            }
            return sse_response(text_sse(DEEPSEEK_TEST_MODEL, "role-launch-complete"));
        }

        if raw.contains("launch four canonical read-only Fleet roles") {
            return sse_response(fleet_role_tool_call_sse());
        }

        sse_response(text_sse(
            DEEPSEEK_TEST_MODEL,
            "fleet-role-receipts-complete",
        ))
    }
}

impl Respond for FanoutResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body = request.body_json::<Value>().unwrap_or(Value::Null);
        let raw = body.to_string();

        if raw.contains("stay busy worker") && !raw.contains("launch six QA workers") {
            self.child_requests.fetch_add(1, Ordering::SeqCst);
            return sse_response(text_sse(DEEPSEEK_TEST_MODEL, "child-finished-too-soon"))
                .set_delay(Duration::from_secs(20));
        }

        if raw.contains("launch six QA workers") {
            return sse_response(fanout_tool_call_sse());
        }

        sse_response(text_sse(DEEPSEEK_TEST_MODEL, "unexpected-request"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_six_worker_fanout_keeps_typing_render_and_esc_cancel_live() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server, &[DEEPSEEK_TEST_MODEL]).await;
    let child_requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(FanoutResponder {
            child_requests: Arc::clone(&child_requests),
        })
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        "[subagents]\nmax_concurrent = 6\nlaunch_concurrency = 6\nmax_admitted = 6\n",
    )?;
    let mut tui = common_tui_builder(&ws)
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", DEEPSEEK_TEST_MODEL)
        .args(["--yolo", "--max-subagents", "6"])
        .spawn()?;
    enter_launch_session(&mut tui)?;

    type_and_submit(
        &mut tui,
        "launch six QA workers and keep the parent turn open",
    )?;
    wait_for_counter(&mut tui, &child_requests, 6, INTERACTION_TIMEOUT)?;
    tui.wait_for(
        |frame| {
            let text = frame.text();
            text.matches("Agent ").count() >= 6
                || text.matches("delegate scout [running]").count() >= 6
        },
        Duration::from_secs(5),
    )?;

    let fanout_frame = tui.debug_dump();
    assert!(
        fanout_frame.matches("Agent ").count() >= 6
            || fanout_frame.matches("delegate scout [running]").count() >= 6,
        "all six workers were not visible in the live runtime projection:\n{fanout_frame}"
    );

    // The provider is deliberately holding every child open. Prove keyboard
    // input and rendering remain live during the storm, then interrupt the
    // still-live orchestration turn directly with Esc.
    tui.send(keys::key::text("fanout-live-marker"))?;
    tui.wait_for_text("fanout-live-marker", Duration::from_secs(3))?;
    let before_cancel = tui.debug_dump();
    assert!(
        before_cancel.contains("Agent") || before_cancel.contains("agent"),
        "fanout UI did not expose agent activity:\n{before_cancel}"
    );

    let cancel_started = Instant::now();
    tui.send(b"\x1b")?;
    tui.wait_for(
        |frame| {
            let text = frame.text().to_ascii_lowercase();
            text.contains("cancelled") || text.contains("interrupted")
        },
        Duration::from_secs(5),
    )?;
    assert!(
        cancel_started.elapsed() < Duration::from_secs(5),
        "Esc cancellation exceeded the five-second liveness budget"
    );

    // Let the raw-key paste-burst window from the pre-cancel marker expire.
    // Without this guard, the first character of the next marker can remain
    // retained while cancellation repaints, making this a paste-heuristic
    // race instead of the intended post-cancel composer-liveness assertion.
    std::thread::sleep(PASTE_GUARD_SETTLE);
    tui.pump();
    tui.send(keys::key::text("post-cancel-live"))?;
    tui.wait_for_text("post-cancel-live", Duration::from_secs(3))?;
    assert_eq!(child_requests.load(Ordering::SeqCst), 6);

    let _ = tui.shutdown();
    Ok(())
}

struct SingleDispatchResponder {
    child_requests: Arc<AtomicUsize>,
}

impl Respond for SingleDispatchResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body = request.body_json::<Value>().unwrap_or(Value::Null);
        let raw = body.to_string();

        if raw.contains("stay busy worker") && !raw.contains("dispatch one QA worker") {
            self.child_requests.fetch_add(1, Ordering::SeqCst);
            return sse_response(text_sse(DEEPSEEK_TEST_MODEL, "child-acknowledged"));
        }

        if raw.contains("dispatch one QA worker") {
            return sse_response(fanout_tool_call_sse_n(1));
        }

        sse_response(text_sse(DEEPSEEK_TEST_MODEL, "unexpected-request"))
    }
}

/// Dispatching `agent` must not kill the process.
///
/// Regression guard for the 0.9.4 release blocker: the Tokio runtime was built
/// by `#[tokio::main]`, so every worker thread carried tokio's 2 MiB default
/// while only the `codewhale-main` owner thread got `CODEWHALE_MAIN_STACK_BYTES`.
/// The engine runs on a worker, and a debug-build `agent` dispatch measured a
/// stack high-water mark between 2.25 and 2.5 MiB — it overflowed the guard page
/// and aborted the process with 134, mid-dispatch, before any child request was
/// ever issued.
///
/// This asserts the invariant that the default violated (the process survives an
/// `agent` dispatch) rather than re-asserting a child counter, which a dead
/// process also fails — but fails slowly and for the wrong stated reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_agent_dispatch_never_aborts_the_runtime() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server, &[DEEPSEEK_TEST_MODEL]).await;
    let child_requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(SingleDispatchResponder {
            child_requests: Arc::clone(&child_requests),
        })
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        "[subagents]\nmax_concurrent = 1\nlaunch_concurrency = 1\nmax_admitted = 1\n",
    )?;
    let mut tui = common_tui_builder(&ws)
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", DEEPSEEK_TEST_MODEL)
        .args(["--yolo", "--max-subagents", "1"])
        .spawn()?;
    enter_launch_session(&mut tui)?;

    type_and_submit(&mut tui, "dispatch one QA worker for the stack guard check")?;

    // The abort lands inside the first `agent` dispatch, so the process is
    // already reaped by the time the child request would have been issued.
    // Probe liveness first: it names the mechanism in the failure text instead
    // of leaving a 15s timeout over a retained frame of a dead TUI.
    assert!(
        tui.wait_for_exit(Duration::from_millis(250)).is_none(),
        "codewhale-tui exited during `agent` dispatch (a stack overflow aborts as 134); \
         the Tokio runtime must carry CODEWHALE_MAIN_STACK_BYTES — see main.rs build_runtime()"
    );

    wait_for_counter(&mut tui, &child_requests, 1, INTERACTION_TIMEOUT)?;

    let _ = tui.shutdown();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_four_read_only_fleet_roles_launch_with_canonical_prompts() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server, &[DEEPSEEK_TEST_MODEL]).await;
    let launched = Arc::new(AtomicUsize::new(0));
    let canonical_prompts = Arc::new(AtomicUsize::new(0));
    let worker = Arc::new(AtomicUsize::new(0));
    let scout = Arc::new(AtomicUsize::new(0));
    let reviewer = Arc::new(AtomicUsize::new(0));
    let verifier = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(FleetRoleResponder {
            launched: Arc::clone(&launched),
            canonical_prompts: Arc::clone(&canonical_prompts),
            worker: Arc::clone(&worker),
            scout: Arc::clone(&scout),
            reviewer: Arc::clone(&reviewer),
            verifier: Arc::clone(&verifier),
        })
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        "[subagents]\nmax_concurrent = 4\nlaunch_concurrency = 4\nmax_admitted = 4\n",
    )?;
    let mut tui = common_tui_builder(&ws)
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", DEEPSEEK_TEST_MODEL)
        .args(["--yolo", "--max-subagents", "4"])
        .spawn()?;
    enter_launch_session(&mut tui)?;

    type_and_submit(&mut tui, "launch four canonical read-only Fleet roles")?;
    wait_for_counter(&mut tui, &launched, 4, INTERACTION_TIMEOUT)?;

    assert_eq!(
        worker.load(Ordering::SeqCst),
        1,
        "worker did not launch once"
    );
    assert_eq!(scout.load(Ordering::SeqCst), 1, "scout did not launch once");
    assert_eq!(
        reviewer.load(Ordering::SeqCst),
        1,
        "reviewer did not launch once"
    );
    assert_eq!(
        verifier.load(Ordering::SeqCst),
        1,
        "verifier did not launch once"
    );
    assert_eq!(
        canonical_prompts.load(Ordering::SeqCst),
        4,
        "each live child request must contain its canonical Fleet role prompt"
    );

    let _ = tui.shutdown();
    Ok(())
}

#[derive(Clone)]
struct SteeringResponder {
    initial_requests: Arc<AtomicUsize>,
    steer_requests: Arc<AtomicUsize>,
}

impl Respond for SteeringResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body = request.body_json::<Value>().unwrap_or(Value::Null);
        let raw = body.to_string();
        if raw.contains("queued steering from enter") {
            self.steer_requests.fetch_add(1, Ordering::SeqCst);
            return sse_response(text_sse(DEEPSEEK_TEST_MODEL, "steering-applied"));
        }
        if raw.contains("portable steering from enter") {
            self.steer_requests.fetch_add(1, Ordering::SeqCst);
            return sse_response(text_sse(DEEPSEEK_TEST_MODEL, "portable-steering-applied"));
        }
        if raw.contains("initial slow turn") {
            self.initial_requests.fetch_add(1, Ordering::SeqCst);
            return sse_response(text_sse(DEEPSEEK_TEST_MODEL, "initial-turn-output"))
                // Leave enough room for the real launch transition plus the
                // queued-preview assertion on slower release-gate machines.
                .set_delay(Duration::from_secs(8));
        }
        sse_response(text_sse(DEEPSEEK_TEST_MODEL, "unexpected-request"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_empty_enter_promotes_queued_follow_up() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server, &[DEEPSEEK_TEST_MODEL]).await;
    let initial_requests = Arc::new(AtomicUsize::new(0));
    let steer_requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(SteeringResponder {
            initial_requests: Arc::clone(&initial_requests),
            steer_requests: Arc::clone(&steer_requests),
        })
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    let mut tui = common_tui_builder(&ws)
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", DEEPSEEK_TEST_MODEL)
        .spawn()?;
    enter_launch_session(&mut tui)?;

    type_and_submit(&mut tui, "initial slow turn")?;
    // Use the same bounded interaction budget as the rest of this PTY gate.
    // Cold debug binaries can take more than three seconds to reach the
    // loopback server while release builds and workspace tests run in parallel.
    // A dead engine still fails closed because the counter never advances.
    wait_for_counter(&mut tui, &initial_requests, 1, INTERACTION_TIMEOUT)?;

    tui.send(keys::key::text("queued steering from enter"))?;
    tui.wait_for_text("queued steering from enter", Duration::from_secs(3))?;
    tui.send(b"\t")?;
    std::thread::sleep(PASTE_GUARD_SETTLE);
    tui.pump();
    assert!(
        tui.frame().contains("queued steering from enter"),
        "Tab must leave a busy-turn draft in the composer:\n{}",
        tui.debug_dump()
    );
    tui.send(keys::key::enter())?;
    tui.wait_for_text("Enter send now", Duration::from_secs(5))?;
    assert!(
        tui.frame().contains("queued steering from enter"),
        "queued steering preview was not readable:\n{}",
        tui.debug_dump()
    );

    tui.send(keys::key::text("stash this draft, do not steer"))?;
    tui.wait_for_text("stash this draft, do not steer", Duration::from_secs(3))?;
    tui.send(keys::key::ctrl_g())?;
    tui.wait_for_text("Draft stashed", Duration::from_secs(3))?;
    assert_eq!(
        steer_requests.load(Ordering::SeqCst),
        0,
        "Ctrl+G must not send a queued follow-up"
    );
    tui.wait_for_text("Enter send now", Duration::from_secs(3))?;

    let steer_started = Instant::now();
    tui.send(keys::key::enter())?;
    wait_for_counter(&mut tui, &steer_requests, 1, INTERACTION_TIMEOUT)?;
    tui.wait_for_text("steering-applied", INTERACTION_TIMEOUT)?;
    assert!(
        steer_started.elapsed() < Duration::from_secs(10),
        "empty Enter queue promotion was not incorporated promptly"
    );

    let _ = tui.shutdown();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_enter_queue_then_enter_steers_running_turn() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server, &[DEEPSEEK_TEST_MODEL]).await;
    let initial_requests = Arc::new(AtomicUsize::new(0));
    let steer_requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(SteeringResponder {
            initial_requests: Arc::clone(&initial_requests),
            steer_requests: Arc::clone(&steer_requests),
        })
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    let mut tui = common_tui_builder(&ws)
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", DEEPSEEK_TEST_MODEL)
        .spawn()?;
    enter_launch_session(&mut tui)?;

    type_and_submit(&mut tui, "initial slow turn")?;
    wait_for_counter(&mut tui, &initial_requests, 1, INTERACTION_TIMEOUT)?;

    tui.send(keys::key::text("busy-shift-line"))?;
    tui.send(keys::key::shift_enter())?;
    tui.send(keys::key::text("busy-alt-line"))?;
    tui.send(keys::key::alt_enter())?;
    tui.send(keys::key::text("busy-ctrl-j-line"))?;
    tui.send(keys::key::ctrl_j())?;
    tui.send(keys::key::text("portable steering from enter"))?;
    tui.wait_for_text("portable steering from enter", Duration::from_secs(3))?;
    let frame = tui.frame();
    let rows = [
        "busy-shift-line",
        "busy-alt-line",
        "busy-ctrl-j-line",
        "portable steering from enter",
    ]
    .map(|line| {
        frame
            .find_text(line)
            .expect("busy multiline draft stays visible")
            .0
    });
    assert!(
        rows.windows(2).all(|pair| pair[0] < pair[1]),
        "newline chords must stay newlines during a running turn:\n{}",
        frame.debug_dump()
    );
    tui.wait_for_text("then ↵ steer", Duration::from_secs(3))?;
    let steer_started = Instant::now();
    // The first portable Enter queues the completed draft. The queued preview
    // uses the already-visible "then Enter" contract; the second Enter
    // promotes it into the active turn even when a multiline preview consumes
    // the compact control row.
    tui.send(keys::key::enter())?;
    std::thread::sleep(PASTE_GUARD_SETTLE);
    tui.pump();
    tui.send(keys::key::enter())?;
    wait_for_counter(&mut tui, &steer_requests, 1, INTERACTION_TIMEOUT)?;
    tui.wait_for_text("portable-steering-applied", INTERACTION_TIMEOUT)?;
    assert!(
        steer_started.elapsed() < Duration::from_secs(10),
        "two-Enter steering was not incorporated promptly"
    );

    let _ = tui.shutdown();
    Ok(())
}

/// Records, for every chat request, the highest-numbered follow-up marker
/// present in the serialized body. Because history accumulates, request `k`
/// contains markers `1..=k`, so the sequence of maxima is an exact record of
/// which follow-up each request carried — which makes a dropped message and a
/// double-sent message both visible, and distinguishable from each other.
#[derive(Clone)]
struct QueueOrderResponder {
    markers: Vec<String>,
    observed: Arc<std::sync::Mutex<Vec<usize>>>,
    initial_delay: Duration,
}

impl QueueOrderResponder {
    fn observed(&self) -> Vec<usize> {
        self.observed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }
}

impl Respond for QueueOrderResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let raw = request
            .body_json::<Value>()
            .unwrap_or(Value::Null)
            .to_string();
        let highest = self
            .markers
            .iter()
            .enumerate()
            .filter(|(_, marker)| raw.contains(marker.as_str()))
            .map(|(index, _)| index + 1)
            .max()
            .unwrap_or(0);
        self.observed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(highest);
        if highest == 0 {
            return sse_response(text_sse(DEEPSEEK_TEST_MODEL, "initial-turn-output"))
                .set_delay(self.initial_delay);
        }
        sse_response(text_sse(
            DEEPSEEK_TEST_MODEL,
            &format!("follow-up-{highest}-done"),
        ))
    }
}

/// The running-turn contract, end to end: while a turn is in flight, bare
/// Enter queues rather than steering, the composer says so, and every queued
/// follow-up dispatches exactly once, in order, after the turn completes.
///
/// This is the mailbox-backpressure row of #3758. Queueing six follow-ups
/// against a busy engine puts several ops in flight behind the
/// `dispatch_in_flight` guard (#4605); the failure modes it rules out are a
/// silently dropped follow-up and a follow-up sent twice, which look identical
/// on the transcript but are opposite bugs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_queued_follow_ups_dispatch_exactly_once_and_in_order() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server, &[DEEPSEEK_TEST_MODEL]).await;

    // Six markers, none a prefix of another, so "contains" cannot confuse
    // marker 1 with marker 10.
    let markers: Vec<String> = (1..=6).map(|n| format!("queue-marker-{n}-end")).collect();
    let responder = QueueOrderResponder {
        markers: markers.clone(),
        observed: Arc::new(std::sync::Mutex::new(Vec::new())),
        initial_delay: Duration::from_secs(14),
    };
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    let mut tui = common_tui_builder(&ws)
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", DEEPSEEK_TEST_MODEL)
        .spawn()?;
    enter_launch_session(&mut tui)?;

    type_and_submit(&mut tui, "queue backpressure initial turn")?;
    // Start the busy-state clock when the loopback server has actually
    // received the request. Cold debug launches may spend most of the generic
    // interaction budget before the request reaches wiremock; the responder's
    // 14-second delay begins only after this signal.
    let request_deadline = Instant::now() + INTERACTION_TIMEOUT;
    while responder.observed().is_empty() {
        tui.pump();
        if Instant::now() >= request_deadline {
            return Err(anyhow!(
                "initial queue-order request never reached the mock server\n{}",
                tui.debug_dump()
            ));
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    let first_marker = markers.first().expect("queue matrix has a marker");
    tui.send(keys::key::text(first_marker))?;
    tui.wait_for_text(first_marker, Duration::from_secs(3))?;
    tui.wait_for_text("then ↵ steer", Duration::from_secs(3))?;

    // While the turn is running the composer must advertise queueing, and it
    // must not advertise the stash chords as a way to send (#440 / #3758).
    let busy_frame = tui.frame();
    let busy_dump = busy_frame.debug_dump();
    assert!(
        busy_frame.contains("↵ queue"),
        "busy composer must say Enter queues:\n{busy_dump}"
    );
    for line in busy_frame.text().lines() {
        if !(line.contains("Ctrl+G") || line.contains("Ctrl+S")) {
            continue;
        }
        let lowered = line.to_ascii_lowercase();
        for forbidden in ["send", "queue", "steer", "submit"] {
            assert!(
                !lowered.contains(forbidden),
                "stash chords must not be advertised as a send/queue/steer path: {line:?}"
            );
        }
    }

    std::thread::sleep(PASTE_GUARD_SETTLE);
    tui.pump();
    tui.send(keys::key::enter())?;
    for marker in markers.iter().skip(1) {
        type_and_submit(&mut tui, marker)?;
    }

    // One request for the initial turn plus one per follow-up.
    let expected_requests = markers.len() + 1;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        tui.pump();
        if responder.observed().len() >= expected_requests {
            break;
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "only {:?} of {expected_requests} requests arrived\n{}",
                responder.observed(),
                tui.debug_dump()
            ));
        }
        std::thread::sleep(Duration::from_millis(80));
    }

    let observed = responder.observed();
    let expected: Vec<usize> = (0..=markers.len()).collect();
    assert_eq!(
        observed,
        expected,
        "queued follow-ups must dispatch exactly once each, in order; \
         a missing index is a dropped message and a repeated one is a double send\n{}",
        tui.debug_dump()
    );

    let _ = tui.shutdown();
    Ok(())
}

#[derive(Clone)]
struct BenchFanoutResponder {
    child_requests: Arc<AtomicUsize>,
    workers: usize,
}

impl Respond for BenchFanoutResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body = request.body_json::<Value>().unwrap_or(Value::Null);
        let raw = body.to_string();

        if raw.contains("stay busy worker") && !raw.contains("launch benchmark QA workers") {
            self.child_requests.fetch_add(1, Ordering::SeqCst);
            return sse_response(text_sse(DEEPSEEK_TEST_MODEL, "child-finished-too-soon"))
                .set_delay(Duration::from_secs(60));
        }

        if raw.contains("launch benchmark QA workers") {
            return sse_response(fanout_tool_call_sse_n(self.workers));
        }

        sse_response(text_sse(DEEPSEEK_TEST_MODEL, "unexpected-request"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RssSample {
    Kib(u64),
    Unavailable(&'static str),
}

impl RssSample {
    fn required_kib(self, phase: &str) -> Result<u64> {
        match self {
            Self::Kib(value) => Ok(value),
            Self::Unavailable(reason) => Err(anyhow!(
                "RSS UNAVAILABLE during {phase}: {reason}; this Unix benchmark requires every sample"
            )),
        }
    }
}

fn rss_kib(pid: Option<u32>) -> RssSample {
    let Some(pid) = pid else {
        return RssSample::Unavailable("process_id_unavailable");
    };
    let out = match std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
    {
        Ok(output) => output,
        Err(_) => return RssSample::Unavailable("ps_command_unavailable"),
    };
    if !out.status.success() {
        return RssSample::Unavailable("ps_nonzero_or_process_exited");
    }
    match std::str::from_utf8(&out.stdout)
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse().ok())
    {
        Some(value) => RssSample::Kib(value),
        None => RssSample::Unavailable("ps_output_invalid"),
    }
}

fn engine_turn_receipts(ws: &SealedWorkspace, pid: u32) -> Result<String> {
    let log_dir = ws.home().join(".codewhale").join("logs");
    if !log_dir.is_dir() {
        return Ok(String::new());
    }

    let pid_suffix = format!("-{pid}.log");
    let mut receipts = String::new();
    for entry in std::fs::read_dir(&log_dir)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().ends_with(&pid_suffix) {
            receipts.push_str(&std::fs::read_to_string(entry.path())?);
        }
    }
    Ok(receipts)
}

fn wait_for_interrupted_engine_turn_receipt(
    tui: &mut Harness,
    ws: &SealedWorkspace,
    timeout: Duration,
) -> Result<()> {
    let pid = tui
        .pid()
        .ok_or_else(|| anyhow!("engine completion receipt unavailable: process id missing"))?;
    let deadline = Instant::now() + timeout;
    loop {
        tui.pump();
        let receipts = engine_turn_receipts(ws, pid)?;
        if receipts.lines().any(|line| {
            line.contains("engine turn completion settled")
                && line.contains("status=Interrupted")
                && line.contains("delivered=true")
        }) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "typed engine TurnComplete(Interrupted) receipt did not arrive within {timeout:?}; \
                 rendered cancellation text is not a settlement receipt\nengine log:\n{receipts}\n{}",
                tui.debug_dump()
            ));
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// #4014 acceptance benchmark: 32 concurrent loopback workers must keep the
/// TUI live. Ignored by default (heavy storm); run explicitly with
/// `cargo test -p codewhale-tui --test release_runtime_qa --locked -- \
///  --ignored bench_thirty_two --nocapture --test-threads=1`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "heavy 32-worker storm; run explicitly for #4014 evidence"]
async fn release_bench_thirty_two_worker_fanout_stays_live() -> Result<()> {
    const WORKERS: usize = 32;
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server, &[DEEPSEEK_TEST_MODEL]).await;
    let child_requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(BenchFanoutResponder {
            child_requests: Arc::clone(&child_requests),
            workers: WORKERS,
        })
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        format!(
            "[subagents]\nmax_concurrent = {WORKERS}\nlaunch_concurrency = {WORKERS}\nmax_admitted = {WORKERS}\n"
        ),
    )?;
    let mut tui = common_tui_builder(&ws)
        .env("RUST_LOG", "warn,engine.turn=info")
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", DEEPSEEK_TEST_MODEL)
        .args(["--yolo", "--max-subagents", &WORKERS.to_string()])
        .spawn()?;
    enter_launch_session(&mut tui)?;
    let pid = tui.pid();
    let rss_idle = rss_kib(pid);

    let spawn_started = Instant::now();
    type_and_submit(
        &mut tui,
        "launch benchmark QA workers and keep the parent turn open",
    )?;
    wait_for_counter(&mut tui, &child_requests, WORKERS, Duration::from_secs(60))?;
    let all_children_live = spawn_started.elapsed();
    // The Ocean work surface owns the exact copy around the aggregate count.
    // Keep this runtime benchmark coupled only to the typed count glyph in the
    // regular/wide phase strip; labels and available actions legitimately
    // change with layout, and compact layouts intentionally omit the count.
    tui.wait_for(
        |frame| {
            let text = frame.text();
            text.contains(&format!("×{WORKERS}"))
        },
        Duration::from_secs(10),
    )?;
    let aggregate_visible = spawn_started.elapsed();
    let rss_storm = rss_kib(pid);

    // Echo latency under storm: three samples.
    let mut echo_samples = Vec::new();
    for i in 0..3 {
        let marker = format!("bench-live-marker-{i}");
        let t = Instant::now();
        tui.send(keys::key::text(&marker))?;
        tui.wait_for_text(&marker, Duration::from_secs(5))?;
        echo_samples.push(t.elapsed());
        // Clear the composer for the next sample.
        for _ in 0..marker.len() {
            tui.send(b"\x7f")?;
        }
    }

    let cancel_started = Instant::now();
    tui.send(b"\x1b")?;
    wait_for_interrupted_engine_turn_receipt(&mut tui, &ws, Duration::from_secs(10))?;
    let cancel_latency = cancel_started.elapsed();

    // Anchor retention evidence to the typed engine cancellation settlement,
    // then schedule absolute 1/3/5-second samples on another runtime worker so
    // the independent post-cancel input proof cannot shift their epoch.
    let post_cancel_observation_started = Instant::now();
    let mut rss_retention_samples = Vec::with_capacity(4);
    rss_retention_samples.push((
        Duration::ZERO,
        post_cancel_observation_started.elapsed(),
        rss_kib(pid),
    ));
    let rss_sampler = tokio::spawn(async move {
        let mut samples = Vec::with_capacity(3);
        for target in [
            Duration::from_secs(1),
            Duration::from_secs(3),
            Duration::from_secs(5),
        ] {
            tokio::time::sleep_until(tokio::time::Instant::from_std(
                post_cancel_observation_started + target,
            ))
            .await;
            samples.push((
                target,
                post_cancel_observation_started.elapsed(),
                rss_kib(pid),
            ));
        }
        samples
    });

    tui.send(keys::key::text("post-cancel-live"))?;
    tui.wait_for_text("post-cancel-live", Duration::from_secs(5))?;
    let delayed_samples = tokio::time::timeout(Duration::from_secs(8), rss_sampler)
        .await
        .map_err(|_| anyhow!("RSS sampler exceeded its bounded post-cancel retention window"))?
        .map_err(|error| anyhow!("RSS sampler task failed: {error}"))?;
    rss_retention_samples.extend(delayed_samples);

    tui.send(keys::key::text("post-cancel-5s-live"))?;
    tui.wait_for_text("post-cancel-5s-live", Duration::from_secs(5))?;

    println!(
        "BENCH32: children_live={all_children_live:?} aggregate={aggregate_visible:?} \
         echo={echo_samples:?} cancel={cancel_latency:?} \
         rss_idle_kib={rss_idle:?} rss_storm_kib={rss_storm:?} \
         rss_retention_samples={rss_retention_samples:?}"
    );

    let worst_echo = echo_samples.iter().max().copied().unwrap_or_default();
    assert!(
        worst_echo < Duration::from_secs(2),
        "typing echo exceeded 2s under a {WORKERS}-worker storm: {echo_samples:?}"
    );
    assert!(
        cancel_latency < Duration::from_secs(5),
        "Esc cancellation exceeded 5s under a {WORKERS}-worker storm: {cancel_latency:?}"
    );
    let idle = rss_idle.required_kib("idle baseline")?;
    let storm = rss_storm.required_kib("live worker storm")?;
    let rss_ceiling = idle.saturating_mul(6).max(idle + 1_500_000);
    assert!(
        storm < rss_ceiling,
        "RSS exploded under storm: idle={idle} KiB storm={storm} KiB"
    );
    for (target, observed, sample) in &rss_retention_samples {
        assert!(
            *observed >= *target,
            "RSS sample preceded its target: target={target:?} observed={observed:?}"
        );
        assert!(
            *observed <= *target + Duration::from_secs(2),
            "RSS sample missed its bounded target window: target={target:?} observed={observed:?}"
        );
        let sample = sample.required_kib(&format!("post-cancel target {target:?}"))?;
        assert!(
            sample < rss_ceiling,
            "RSS exceeded the bounded storm ceiling at target={target:?} \
             observed={observed:?}: idle={idle} KiB storm={storm} KiB sample={sample} KiB"
        );
    }

    let _ = tui.shutdown();
    Ok(())
}

/// Dogfood of the named-Fleet journey against the release binary:
/// migration -> session-only route change -> explicit /fleet save -> restart
/// (selected Fleet operator applied) -> save-as. Every receipt is asserted
/// on-screen and every claim is checked against the on-disk files.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn release_fleet_route_save_journey() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let mock = MockServer::start().await;
    mount_text_model(&mock, "deepseek-v4-pro", "ok").await;
    let base_url = mock.uri();

    let ws = make_sealed_workspace()?;
    let agents_dir = ws.workspace().join(".codewhale").join("agents");
    std::fs::create_dir_all(&agents_dir)
        .map_err(|e| anyhow::anyhow!("agents dir {}: {e}", agents_dir.display()))?;
    std::fs::write(
        agents_dir.join("scout.toml"),
        r#"id = "scout"
role_hint = "scout"
model = "deepseek-v4-flash"
provider = "deepseek"
"#,
    )
    .map_err(|e| anyhow::anyhow!("scout profile: {e}"))?;
    let home_dir = ws.home().join(".codewhale");
    std::fs::create_dir_all(&home_dir)
        .map_err(|e| anyhow::anyhow!("home dir {}: {e}", home_dir.display()))?;
    std::fs::write(
        home_dir.join("config.toml"),
        format!(
            r#"provider = "deepseek"
[providers.deepseek]
api_key = "sk-release-qa"
base_url = "{base_url}"
"#
        ),
    )
    .map_err(|e| anyhow::anyhow!("home config: {e}"))?;

    let mut tui = common_tui_builder(&ws).spawn()?;
    enter_launch_session(&mut tui)?;

    // 1. /fleet fleets (secondary named-Fleet picker) shows the migration
    // banner and no selection. Bare /fleet stays on the roster face.
    type_and_submit(&mut tui, "/fleet fleets")?;
    tui.wait_for_text("legacy role profile", INTERACTION_TIMEOUT)?;
    tui.wait_for_text("No Fleet selected", INTERACTION_TIMEOUT)?;
    // 2. m migrates with a receipt; Esc closes the pager, Esc closes the list.
    tui.send(keys::key::text("m"))?;
    tui.wait_for_text("Migrated", INTERACTION_TIMEOUT)?;
    tui.send(keys::key::esc())?;
    std::thread::sleep(Duration::from_millis(300));
    tui.send(keys::key::esc())?;
    std::thread::sleep(Duration::from_millis(300));
    tui.pump();

    // 3. A route change is session-only and names the explicit commands.
    type_and_submit(&mut tui, "/model deepseek-v4-pro")?;
    tui.wait_for_text("session only", INTERACTION_TIMEOUT)?;
    tui.wait_for_text("/fleet save", INTERACTION_TIMEOUT)?;

    // 4. /fleet save writes the operator; the receipt names the file.
    type_and_submit(&mut tui, "/fleet save")?;
    tui.wait_for_text("now runs on deepseek/deepseek-v4-pro", INTERACTION_TIMEOUT)?;

    // 5. The updated Fleet file is the migrated Default (v2 schema with the
    // operator route pinned to the session model).
    let fleet_file = ws
        .home()
        .join(".codewhale")
        .join("fleets")
        .join("default.toml");
    let fleet_text = std::fs::read_to_string(&fleet_file)?;
    assert!(
        fleet_text.contains("schema = \"fleet\"") && fleet_text.contains("deepseek-v4-pro"),
        "saved fleet must be v2 with the operator: {fleet_text}"
    );

    // 6. Restart: the selected Fleet's operator is the session route.
    tui.shutdown()
        .ok_or_else(|| anyhow::anyhow!("graceful shutdown failed"))?;
    let mut tui = common_tui_builder(&ws).spawn()?;
    enter_launch_session(&mut tui)?;
    tui.wait_for_text("deepseek-v4-pro", INTERACTION_TIMEOUT)?;

    // 7. The named-Fleet picker shows the saved Fleet with its user scope
    // and selection (operator summary, not a filesystem path).
    type_and_submit(&mut tui, "/fleet fleets")?;
    tui.wait_for_text("DeepSeek", INTERACTION_TIMEOUT)?;
    tui.wait_for_text("[user]", INTERACTION_TIMEOUT)?;
    tui.wait_for_text("Selected", INTERACTION_TIMEOUT)?;
    tui.send(keys::key::esc())?;
    std::thread::sleep(Duration::from_millis(300));
    tui.send(keys::key::esc())?;
    std::thread::sleep(Duration::from_millis(300));
    tui.pump();

    // 8. save-as creates and selects a second user-global Fleet.
    type_and_submit(&mut tui, "/model deepseek-v4-flash")?;
    tui.wait_for_text("session only", INTERACTION_TIMEOUT)?;
    type_and_submit(&mut tui, "/fleet save-as")?;
    // The receipt wraps across lines at this width; assert on fragments that
    // land on a single row.
    tui.wait_for_text("as new Fleet", INTERACTION_TIMEOUT)?;
    tui.wait_for_text("user-global default", INTERACTION_TIMEOUT)?;
    // The new Fleet is named after the route: `DeepSeek deepseek-v4-flash`.
    let second_file = ws
        .home()
        .join(".codewhale")
        .join("fleets")
        .join("deepseek-deepseek-v4-flash.toml");
    assert!(
        second_file.is_file(),
        "save-as must create the second fleet file"
    );
    // The legacy profile file was left untouched.
    assert!(
        std::fs::read_to_string(
            ws.workspace()
                .join(".codewhale")
                .join("agents")
                .join("scout.toml")
        )?
        .contains("deepseek-v4-flash")
    );

    let _ = tui.shutdown();
    Ok(())
}

/// Named custom provider used by `release_resume_restores_route_identity` as
/// the restored ("route B") identity. Deliberately a `[providers.<name>]`
/// custom table so the restored identity differs from the startup route in
/// provider kind, exact identity, endpoint, and model at once.
const RESTORED_PROVIDER_KEY: &str = "qa-remote";
const RESTORED_TEST_MODEL: &str = "qa-remote-model-x";

/// Sealed-home config: keep the harness's silent-notifications block and add
/// one named custom OpenAI-compatible provider pointing at the restored-route
/// mock server. The startup route stays env-configured DeepSeek.
fn write_restored_route_provider_config(ws: &SealedWorkspace, base_url: &str) -> Result<()> {
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        format!(
            "[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n\n\
             [providers.{RESTORED_PROVIDER_KEY}]\n\
             kind = \"openai-compatible\"\n\
             base_url = \"{base_url}\"\n\
             api_key = \"qa-remote-local-test-key\"\n\
             model = \"{RESTORED_TEST_MODEL}\"\n"
        ),
    )?;
    Ok(())
}

/// A persisted session whose metadata declares the restored route B:
/// `model_provider = "custom"` + `model_provider_id = "qa-remote"` is exactly
/// what the live save path writes for a named custom route (see
/// `SessionMetadata::set_model_provider_route`).
fn write_restored_route_session(
    ws: &SealedWorkspace,
    id: &str,
    message_count: usize,
) -> Result<std::path::PathBuf> {
    let messages = (0..message_count)
        .map(|index| {
            json!({
                "role": if index % 2 == 0 { "user" } else { "assistant" },
                "content": [{
                    "type": "text",
                    "text": format!("restored route dialogue item {index:02}"),
                    "cache_control": null
                }]
            })
        })
        .collect::<Vec<_>>();
    let session_path = ws.workspace().join(format!("{id}.json"));
    std::fs::write(
        &session_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "metadata": {
                "id": id,
                "title": format!("Route identity PTY {id}"),
                "created_at": "2026-08-08T00:00:00Z",
                "updated_at": "2026-08-08T00:00:00Z",
                "message_count": messages.len(),
                "total_tokens": 64,
                "model": RESTORED_TEST_MODEL,
                "model_provider": "custom",
                "model_provider_id": RESTORED_PROVIDER_KEY,
                "workspace": ws.workspace(),
                "mode": "agent",
                "cost": {},
                "cumulative_turn_secs": 0
            },
            "messages": messages,
            "system_prompt": null,
            "work_state": null
        }))?,
    )?;
    Ok(session_path)
}

/// Regression for the v0.9.6 known issue: "A resumed session can still display
/// the startup provider/model instead of the restored route identity."
///
/// Three identities are recorded separately and must agree after `/load`:
/// 1. persisted — what the session JSON metadata declares (route B:
///    custom `qa-remote`, model `qa-remote-model-x`),
/// 2. displayed — the header route label after the load,
/// 3. outbound — which mock server (and which request-body model) receives
///    the next submitted turn.
///
/// The startup route A (env-configured DeepSeek against its own loopback
/// server) first completes a real turn, so any stale "effective route" state a
/// drifting restore would leave behind genuinely exists before the load.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_resume_restores_route_identity() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let startup_server = MockServer::start().await;
    let restored_server = MockServer::start().await;
    mount_text_model(&startup_server, DEEPSEEK_TEST_MODEL, "startup-route-ok").await;
    mount_text_model(&restored_server, RESTORED_TEST_MODEL, "restored-route-ok").await;

    let ws = make_sealed_workspace()?;
    write_restored_route_provider_config(&ws, &restored_server.uri())?;
    let session_path = write_restored_route_session(&ws, "route-identity-pty", 4)?;

    let mut tui = compaction_tui_builder(&ws, &startup_server).spawn()?;
    enter_launch_session(&mut tui)?;

    // Startup identity: route A on the header before anything else happens.
    let startup_route_label = format!("DeepSeek · {DEEPSEEK_TEST_MODEL}");
    let startup_needle = startup_route_label.clone();
    tui.wait_for(
        move |frame| frame.row(0).contains(&startup_needle),
        INTERACTION_TIMEOUT,
    )?;

    // A completed startup turn against route A. This both proves the startup
    // route works and materializes the per-turn route state (`pending_turn_route`
    // and friends) whose staleness a broken restore could later display.
    type_and_submit(&mut tui, "startup route probe")?;
    tui.wait_for_text("startup-route-ok", INTERACTION_TIMEOUT)?;

    // Identity 1 (persisted): declared by the session file written above.
    let persisted_provider = RESTORED_PROVIDER_KEY;
    let persisted_model = RESTORED_TEST_MODEL;

    load_session(&mut tui, &session_path, 4)?;

    // Identity 2 (displayed): the header must show the restored route, not the
    // startup route. This is the exact drift named in the v0.9.6 known issue.
    let restored_route_label = format!("{persisted_provider} · {persisted_model}");
    let displayed_needle = restored_route_label.clone();
    if let Err(err) = tui.wait_for(
        move |frame| frame.row(0).contains(&displayed_needle),
        INTERACTION_TIMEOUT,
    ) {
        let header = tui.frame().row(0);
        return Err(anyhow!(
            "displayed route identity diverged from the persisted session route: \
             persisted `{restored_route_label}`, header shows `{header}` \
             (startup route was `{startup_route_label}`): {err}"
        ));
    }
    let displayed_header = tui.frame().row(0);
    assert!(
        !displayed_header.contains(&startup_route_label),
        "header still displays the startup route alongside the restored one: {displayed_header}"
    );

    // Identity 3 (outbound): the next submitted turn must reach the restored
    // provider's endpoint with the restored model in the request body.
    type_and_submit(&mut tui, "restored route outbound probe")?;
    tui.wait_for_text("restored-route-ok", INTERACTION_TIMEOUT)?;

    let startup_chat = chat_requests(&startup_server.received_requests().await.unwrap_or_default());
    let restored_chat = chat_requests(
        &restored_server
            .received_requests()
            .await
            .unwrap_or_default(),
    );
    assert_eq!(
        startup_chat.len(),
        1,
        "outbound identity diverged: the startup provider received {} chat request(s) \
         after the load instead of only its pre-load probe: {startup_chat:#?}",
        startup_chat.len().saturating_sub(1)
    );
    assert_eq!(
        restored_chat.len(),
        1,
        "restored provider endpoint did not receive exactly the post-load turn: {restored_chat:#?}"
    );
    assert_eq!(
        restored_chat[0]["model"], persisted_model,
        "outbound request-body model diverged from the persisted session model"
    );
    assert!(
        restored_chat[0]
            .to_string()
            .contains("restored route outbound probe"),
        "restored-route request does not carry the post-load turn: {:#?}",
        restored_chat[0]
    );
    assert!(
        startup_chat[0].to_string().contains("startup route probe"),
        "startup-route request should be the pre-load probe only: {:#?}",
        startup_chat[0]
    );

    let _ = tui.shutdown();
    Ok(())
}

/// Write the restored-route session into the sealed home's canonical sessions
/// directory so id-based resume surfaces (startup `--resume`, the `/resume`
/// picker) can find it.
fn install_restored_route_session_in_sessions_dir(
    ws: &SealedWorkspace,
    id: &str,
    message_count: usize,
) -> Result<()> {
    let session_path = write_restored_route_session(ws, id, message_count)?;
    let sessions_dir = ws.home().join(".codewhale").join("sessions");
    std::fs::create_dir_all(&sessions_dir)?;
    std::fs::rename(&session_path, sessions_dir.join(format!("{id}.json")))?;
    Ok(())
}

/// Same three-identity contract as `release_resume_restores_route_identity`,
/// on the startup resume surface: `codewhale --resume <id>` must boot straight
/// into the restored route identity, not the env-configured startup route.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_startup_resume_restores_route_identity() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let startup_server = MockServer::start().await;
    let restored_server = MockServer::start().await;
    mount_text_model(&startup_server, DEEPSEEK_TEST_MODEL, "startup-route-ok").await;
    mount_text_model(&restored_server, RESTORED_TEST_MODEL, "restored-route-ok").await;

    let ws = make_sealed_workspace()?;
    write_restored_route_provider_config(&ws, &restored_server.uri())?;
    install_restored_route_session_in_sessions_dir(&ws, "route-identity-startup", 4)?;

    let mut tui = compaction_tui_builder(&ws, &startup_server)
        .args(["--resume", "route-identity-startup"])
        .spawn()?;
    enter_launch_session(&mut tui)?;

    // Displayed identity: the restored route must be on the header at boot.
    let restored_route_label = format!("{RESTORED_PROVIDER_KEY} · {RESTORED_TEST_MODEL}");
    let displayed_needle = restored_route_label.clone();
    if let Err(err) = tui.wait_for(
        move |frame| frame.row(0).contains(&displayed_needle),
        INTERACTION_TIMEOUT,
    ) {
        let header = tui.frame().row(0);
        return Err(anyhow!(
            "startup --resume displayed route identity diverged: persisted \
             `{restored_route_label}`, header shows `{header}`: {err}"
        ));
    }

    // Outbound identity: the first submitted turn must reach the restored
    // provider with the restored model; the startup provider gets nothing.
    type_and_submit(&mut tui, "restored route outbound probe")?;
    tui.wait_for_text("restored-route-ok", INTERACTION_TIMEOUT)?;

    let startup_chat = chat_requests(&startup_server.received_requests().await.unwrap_or_default());
    let restored_chat = chat_requests(
        &restored_server
            .received_requests()
            .await
            .unwrap_or_default(),
    );
    assert_eq!(
        startup_chat.len(),
        0,
        "outbound identity diverged: the startup provider received chat request(s) \
         in a session resumed onto another route: {startup_chat:#?}"
    );
    assert_eq!(
        restored_chat.len(),
        1,
        "restored provider endpoint did not receive exactly the resumed turn: {restored_chat:#?}"
    );
    assert_eq!(
        restored_chat[0]["model"], RESTORED_TEST_MODEL,
        "outbound request-body model diverged from the persisted session model"
    );

    let _ = tui.shutdown();
    Ok(())
}

/// Same three-identity contract on the interactive picker surface: `/resume`
/// with no argument opens the session picker; selecting the persisted session
/// must swap the header and the outbound route to the restored identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_session_picker_restores_route_identity() -> Result<()> {
    let _guard = RELEASE_RUNTIME_QA_LOCK.lock().await;
    let startup_server = MockServer::start().await;
    let restored_server = MockServer::start().await;
    mount_text_model(&startup_server, DEEPSEEK_TEST_MODEL, "startup-route-ok").await;
    mount_text_model(&restored_server, RESTORED_TEST_MODEL, "restored-route-ok").await;

    let ws = make_sealed_workspace()?;
    write_restored_route_provider_config(&ws, &restored_server.uri())?;
    install_restored_route_session_in_sessions_dir(&ws, "route-identity-picker", 4)?;

    let mut tui = compaction_tui_builder(&ws, &startup_server)
        .env("RUST_LOG", crate::qa_harness::view_log::VIEW_STACK_RUST_LOG)
        .spawn()?;
    enter_launch_session(&mut tui)?;

    // Startup route A completes a real turn first, exactly like the /load
    // scenario, so stale per-turn route state exists before the picker swap.
    let startup_route_label = format!("DeepSeek · {DEEPSEEK_TEST_MODEL}");
    let startup_needle = startup_route_label.clone();
    tui.wait_for(
        move |frame| frame.row(0).contains(&startup_needle),
        INTERACTION_TIMEOUT,
    )?;
    type_and_submit(&mut tui, "startup route probe")?;
    tui.wait_for_text("startup-route-ok", INTERACTION_TIMEOUT)?;

    // Open the picker. The action rail paints with the modal, so it is the
    // robust "picker is open" signal.
    type_and_submit(&mut tui, "/resume")?;
    if let Err(err) = tui.wait_for_text("Enter resume", INTERACTION_TIMEOUT) {
        let events = crate::qa_harness::view_log::read_events(ws.home())
            .map(|events| format!("{events:#?}"))
            .unwrap_or_else(|log_err| format!("<no view log: {log_err}>"));
        return Err(anyhow!(
            "session picker never opened after /resume: {err}\nview-stack events: {events}"
        ));
    }
    // The picker also lists the autosaved live session, which sorts first.
    // Search-filter to the persisted session id so Enter cannot resume the
    // wrong row.
    tui.send(keys::key::text("/route-identity-picker"))?;
    tui.wait_for_text("1. route-id", INTERACTION_TIMEOUT)?;
    std::thread::sleep(PASTE_GUARD_SETTLE);
    tui.pump();
    tui.send(keys::key::enter())?; // leave search mode, keep the filtered row
    std::thread::sleep(PASTE_GUARD_SETTLE);
    tui.pump();
    tui.send(keys::key::enter())?; // resume the selected session
    tui.wait_for_text("restored route dialogue item 00", INTERACTION_TIMEOUT)?;
    // Picker resume owns a durable transcript receipt, matching `/load` —
    // the transient status toast alone loses the record to footer churn.
    tui.wait_for_text("Session loaded (ID:", INTERACTION_TIMEOUT)?;

    // Displayed identity after the picker swap.
    let restored_route_label = format!("{RESTORED_PROVIDER_KEY} · {RESTORED_TEST_MODEL}");
    let displayed_needle = restored_route_label.clone();
    if let Err(err) = tui.wait_for(
        move |frame| frame.row(0).contains(&displayed_needle),
        INTERACTION_TIMEOUT,
    ) {
        let header = tui.frame().row(0);
        return Err(anyhow!(
            "session-picker displayed route identity diverged: persisted \
             `{restored_route_label}`, header shows `{header}` \
             (startup route was `{startup_route_label}`): {err}"
        ));
    }

    // Outbound identity after the picker swap.
    type_and_submit(&mut tui, "restored route outbound probe")?;
    tui.wait_for_text("restored-route-ok", INTERACTION_TIMEOUT)?;

    let startup_chat = chat_requests(&startup_server.received_requests().await.unwrap_or_default());
    let restored_chat = chat_requests(
        &restored_server
            .received_requests()
            .await
            .unwrap_or_default(),
    );
    assert_eq!(
        startup_chat.len(),
        1,
        "outbound identity diverged: the startup provider received {} chat request(s) \
         after the picker swap instead of only its pre-swap probe: {startup_chat:#?}",
        startup_chat.len().saturating_sub(1)
    );
    assert_eq!(
        restored_chat.len(),
        1,
        "restored provider endpoint did not receive exactly the post-swap turn: {restored_chat:#?}"
    );
    assert_eq!(
        restored_chat[0]["model"], RESTORED_TEST_MODEL,
        "outbound request-body model diverged from the persisted session model"
    );

    let _ = tui.shutdown();
    Ok(())
}
