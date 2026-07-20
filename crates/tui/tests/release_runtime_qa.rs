//! Local-only release runtime QA through real pseudo-terminals.
//!
//! These scenarios cover the live TUI checks that unit tests cannot prove:
//! six-worker fanout liveness/cancellation, multi-terminal route isolation,
//! and queued steering via the terminal-safe Ctrl+G shortcut. Every provider is a loopback
//! wiremock
//! server and every process receives a sealed HOME.

#![cfg(unix)]

#[path = "support/qa_harness/mod.rs"]
mod qa_harness;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use qa_harness::harness::{Harness, SealedWorkspace, make_sealed_workspace};
use qa_harness::keys;
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

fn common_tui_builder(ws: &SealedWorkspace) -> qa_harness::harness::HarnessBuilder {
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

fn type_and_tab(harness: &mut Harness, text: &str) -> Result<()> {
    harness.send(keys::key::text(text))?;
    harness.wait_for_text(text, Duration::from_secs(3))?;
    harness.send(b"\t")?;
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
    enter_launch_session(&mut tui)?;

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
        .wait_for_text("Pick a theme", Duration::from_secs(1))
        .is_err()
    {
        // A PTY can deliver the first Enter inside the paste guard's trailing
        // suppression window. Once that window expires, the next deliberate
        // Enter must execute the retained draft.
        std::thread::sleep(PASTE_GUARD_SETTLE);
        tui.pump();
        tui.send(keys::key::enter())?;
        tui.wait_for_text("Pick a theme", INTERACTION_TIMEOUT)?;
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
        let selected = format!("▶ {}.", index + 1);
        tui.wait_for(
            |frame| frame.text().contains(&selected),
            INTERACTION_TIMEOUT,
        )?;
        let frame = tui.frame();
        let signature = (
            frame.colors_at(0, 0).expect("theme surface cell"),
            frame
                .first_symbol_colors("▶")
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

#[derive(Clone)]
struct SteeringResponder {
    initial_requests: Arc<AtomicUsize>,
    steer_requests: Arc<AtomicUsize>,
}

impl Respond for SteeringResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body = request.body_json::<Value>().unwrap_or(Value::Null);
        let raw = body.to_string();
        if raw.contains("queued steering from ctrl-g") {
            self.steer_requests.fetch_add(1, Ordering::SeqCst);
            return sse_response(text_sse(DEEPSEEK_TEST_MODEL, "steering-applied"));
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
async fn release_queued_steering_ctrl_g_sends_now_with_clear_status() -> Result<()> {
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

    type_and_tab(&mut tui, "queued steering from ctrl-g")?;
    tui.wait_for_text("Ctrl+G send", Duration::from_secs(5))?;
    assert!(
        tui.frame().contains("queued steering from ctrl-g"),
        "queued steering preview was not readable:\n{}",
        tui.debug_dump()
    );

    let steer_started = Instant::now();
    tui.send(b"\x07")?;
    wait_for_counter(&mut tui, &steer_requests, 1, INTERACTION_TIMEOUT)?;
    tui.wait_for_text("steering-applied", INTERACTION_TIMEOUT)?;
    assert!(
        steer_started.elapsed() < Duration::from_secs(10),
        "Ctrl+G steering was not incorporated promptly"
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

fn rss_kib(pid: u32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
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
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .env("DEEPSEEK_MODEL", DEEPSEEK_TEST_MODEL)
        .args(["--yolo", "--max-subagents", &WORKERS.to_string()])
        .spawn()?;
    enter_launch_session(&mut tui)?;
    let pid = tui.pid();
    let rss_idle = pid.and_then(rss_kib);

    let spawn_started = Instant::now();
    type_and_submit(
        &mut tui,
        "launch benchmark QA workers and keep the parent turn open",
    )?;
    wait_for_counter(&mut tui, &child_requests, WORKERS, Duration::from_secs(60))?;
    let all_children_live = spawn_started.elapsed();
    // The Ocean work surface reports both the active count and the worker
    // count in its compact summary. Do not couple this runtime benchmark to
    // the retired sidebar phrase ("N running").
    tui.wait_for(
        |frame| {
            let text = frame.text();
            (text.contains(&format!("Active {WORKERS}"))
                && text.contains(&format!("Workers {WORKERS}")))
                || (text.contains(&format!("run ×{WORKERS}")) && text.contains("[open] [stop]"))
        },
        Duration::from_secs(10),
    )?;
    let sidebar_visible = spawn_started.elapsed();
    let rss_storm = pid.and_then(rss_kib);

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
    tui.wait_for(
        |frame| {
            let text = frame.text().to_ascii_lowercase();
            text.contains("cancelled") || text.contains("interrupted")
        },
        Duration::from_secs(10),
    )?;
    let cancel_latency = cancel_started.elapsed();

    tui.send(keys::key::text("post-cancel-live"))?;
    tui.wait_for_text("post-cancel-live", Duration::from_secs(5))?;
    let rss_after = pid.and_then(rss_kib);

    println!(
        "BENCH32: children_live={all_children_live:?} sidebar={sidebar_visible:?} \
         echo={echo_samples:?} cancel={cancel_latency:?} \
         rss_idle_kib={rss_idle:?} rss_storm_kib={rss_storm:?} rss_after_kib={rss_after:?}"
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
    if let (Some(idle), Some(storm)) = (rss_idle, rss_storm) {
        assert!(
            storm < idle.saturating_mul(6).max(idle + 1_500_000),
            "RSS exploded under storm: idle={idle} KiB storm={storm} KiB"
        );
    }

    let _ = tui.shutdown();
    Ok(())
}
