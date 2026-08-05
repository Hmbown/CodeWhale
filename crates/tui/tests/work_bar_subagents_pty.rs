//! Owner report (2026-08-04): "the sub agents still aren't showing up in the
//! top bar so they aren't inspectable."
//!
//! Static reading of `work_surface/model.rs` says the rows are built and are
//! durable, so this probe refuses to reason about it: every assertion below is
//! made against a real pseudo-terminal frame produced by the real event loop,
//! with a loopback provider that dispatches genuine `agent` tool calls.
//!
//! The contract under test (`crates/tui/AGENTS.md`, "rows are objects"): every
//! work-bar row is a door — click it and the world behind it opens — and
//! keyboard Enter opens the same door a click does.

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
const INTERACTION_TIMEOUT: Duration = Duration::from_secs(20);
const PASTE_GUARD_SETTLE: Duration = Duration::from_millis(180);
const COMPOSER_READY_TEXT: &str = "Write a task";
const MODEL: &str = "deepseek-v4-pro";

/// The user prompt that triggers the fan-out. Only ever present in a *parent*
/// request, so the responder can tell parent from child without guessing.
const PARENT_PROMPT: &str = "spawn the work-bar probe workers now";
/// The objective handed to each child. Also the text the work-bar row shows.
const CHILD_MARKER: &str = "workbarprobe";

static WORK_BAR_PTY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn sse_chunk(value: Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::to_string(&value).expect("SSE JSON")
    )
}

fn text_sse(text: &str) -> String {
    [
        sse_chunk(json!({
            "id": "chatcmpl-workbar",
            "object": "chat.completion.chunk",
            "model": MODEL,
            "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-workbar",
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
                "id": format!("call_workbar_{worker}"),
                "type": "function",
                "function": {
                    "name": "agent",
                    "arguments": serde_json::to_string(&json!({
                        "message": format!("{CHILD_MARKER}{worker} keep working"),
                        "agent_type": "explorer",
                        // Explicit fresh context: a forked child would carry the
                        // parent prompt into its own requests and defeat the
                        // parent/child discrimination in the responder.
                        "fork_context": false,
                        "session_name": format!("workbar-{worker}")
                    }))
                    .expect("agent arguments")
                }
            })
        })
        .collect::<Vec<_>>();

    [
        sse_chunk(json!({
            "id": "chatcmpl-workbar-fanout",
            "object": "chat.completion.chunk",
            "model": MODEL,
            "choices": [{"index": 0, "delta": {"tool_calls": tool_calls}, "finish_reason": null}]
        })),
        sse_chunk(json!({
            "id": "chatcmpl-workbar-fanout",
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

/// Dispatches one fan-out on the first parent turn, then answers the parent
/// plainly. `child_hold` decides whether the workers stay running (a long
/// delay) or finish immediately.
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
            return sse_response(text_sse("workbar child receipt")).set_delay(self.child_hold);
        }
        if raw.contains(PARENT_PROMPT) {
            if self.parent_turns.fetch_add(1, Ordering::SeqCst) == 0 {
                return sse_response(agent_tool_call_sse(self.workers));
            }
            return sse_response(text_sse("workbar parent wrapped up"));
        }
        sse_response(text_sse("unexpected-request"))
    }
}

fn tui_builder(ws: &SealedWorkspace, server_uri: &str) -> qa_harness::harness::HarnessBuilder {
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
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
            "--mouse-capture",
            "--yolo",
            "--max-subagents",
            "2",
        ])
        .size(42, 150)
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

fn is_divider_row(frame: &qa_harness::Frame, y: u16) -> bool {
    frame
        .row(y)
        .chars()
        .filter(|&c| c == '─' || c == '━')
        .count()
        >= 40
}

/// The `▾ Subagents N` group header the Top strip paints above its worker
/// rows. Its absence *is* the owner-reported bug, so it is the anchor every
/// other strip probe hangs off rather than a screen-wide text search.
fn subagents_header_row(harness: &mut Harness) -> Option<u16> {
    let frame = harness.frame();
    (0..frame.rows()).find(|&y| frame.row(y).contains("Subagents") && !is_divider_row(frame, y))
}

/// Every row painted in the work bar, header included.
fn work_bar_text(harness: &mut Harness) -> String {
    let frame = harness.frame();
    let rows = frame.rows();
    // The strip sits between the ocean header rule and the transcript rule.
    let dividers: Vec<u16> = (0..rows).filter(|&y| is_divider_row(frame, y)).collect();
    let (start, end) = match dividers.as_slice() {
        [first, second, ..] => (first.saturating_add(1), *second),
        _ => (0, rows),
    };
    (start..end)
        .map(|y| frame.row(y))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A worker row inside the strip: the rows the `Subagents` header owns, up to
/// the strip's closing rule. Returns `(row, column)` for a real SGR click.
fn work_bar_worker_row(harness: &mut Harness) -> Option<(u16, u16)> {
    let header = subagents_header_row(harness)?;
    let frame = harness.frame();
    let rows = frame.rows();
    (header.saturating_add(1)..rows)
        .take_while(|&y| !is_divider_row(frame, y))
        .find_map(|y| {
            let text = frame.row(y);
            let trimmed = text.trim_start();
            if trimmed.is_empty() {
                return None;
            }
            let col = u16::try_from(text.len() - trimmed.len()).ok()?;
            Some((y, col.saturating_add(2)))
        })
}

fn session_with_todos(ws: &SealedWorkspace, count: usize) -> Result<std::path::PathBuf> {
    let session_path = ws.workspace().join("workbar-session.json");
    let todos = (0..count)
        .map(|index| {
            json!({
                "id": index + 1,
                "content": format!("todo-workbar-{index:02}"),
                "status": if index == 0 { "in_progress" } else { "pending" }
            })
        })
        .collect::<Vec<_>>();
    std::fs::write(
        &session_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "metadata": {
                "id": "pty-workbar",
                "title": "Work bar sub-agent probe",
                "created_at": "2026-08-04T00:00:00Z",
                "updated_at": "2026-08-04T00:00:00Z",
                "message_count": 0,
                "total_tokens": 0,
                "model": MODEL,
                "model_provider": "deepseek",
                "workspace": ws.workspace(),
                "mode": "agent",
                "cost": {},
                "cumulative_turn_secs": 0
            },
            "messages": [],
            "system_prompt": null,
            "work_state": {
                "todos": {"items": todos, "completion_pct": 0, "in_progress_id": 1},
                "plan": {"objective": "", "items": []}
            }
        }))?,
    )?;
    Ok(session_path)
}

/// Baseline: with nothing competing for strip rows, a running sub-agent must
/// appear in the top bar, a real SGR click must open its detail, and keyboard
/// Enter must open the same door.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn work_bar_lists_a_running_subagent_and_opens_it_by_click_and_enter() -> Result<()> {
    let _guard = WORK_BAR_PTY_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server).await;
    let child_requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ProbeResponder {
            child_requests: Arc::clone(&child_requests),
            parent_turns: Arc::new(AtomicUsize::new(0)),
            workers: 2,
            child_hold: Duration::from_secs(25),
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

    tui.wait_for(
        |frame| frame.text().contains("Subagents"),
        Duration::from_secs(10),
    )?;

    let strip = work_bar_text(&mut tui);
    assert!(
        strip.contains("Subagents"),
        "the `Subagents` header is not inside the top work bar:\n{strip}\n---full---\n{}",
        tui.debug_dump()
    );
    let (row, col) = work_bar_worker_row(&mut tui).ok_or_else(|| {
        anyhow!(
            "no running sub-agent row rendered in the top work bar\n{}",
            tui.debug_dump()
        )
    })?;

    // Click the row: the door must open.
    tui.send(keys::mouse::click(row, col))?;
    tui.wait_for_text("Agent Details", Duration::from_secs(5))
        .map_err(|_| {
            anyhow!(
                "clicking the sub-agent row did not open its detail\n{}",
                tui.debug_dump()
            )
        })?;

    // Close, then prove keyboard parity: Alt+W focuses the strip, End selects
    // the last selectable row (a worker), Enter opens the same detail.
    tui.send(keys::key::esc())?;
    tui.wait_for(
        |frame| !frame.text().contains("Agent Details"),
        Duration::from_secs(5),
    )?;
    tui.send(keys::key::alt('w'))?;
    tui.send(b"\x1b[F")?; // End
    tui.send(keys::key::enter())?;
    tui.wait_for_text("Agent Details", Duration::from_secs(5))
        .map_err(|_| {
            anyhow!(
                "Enter on the selected work-bar row did not open the detail a click opens\n{}",
                tui.debug_dump()
            )
        })?;

    let _ = tui.shutdown();
    Ok(())
}

/// The dogfood shape: a session already carrying a to-do list, then a fan-out.
/// Sub-agents must remain visible and clickable in the top bar — a strip that
/// spends every row it has on to-dos and pushes the workers off the bottom is
/// exactly the owner-reported failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn work_bar_still_shows_subagents_when_todos_are_present() -> Result<()> {
    let _guard = WORK_BAR_PTY_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server).await;
    let child_requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ProbeResponder {
            child_requests: Arc::clone(&child_requests),
            parent_turns: Arc::new(AtomicUsize::new(0)),
            workers: 2,
            child_hold: Duration::from_secs(25),
        })
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        "[subagents]\nmax_concurrent = 2\nlaunch_concurrency = 2\nmax_admitted = 2\n",
    )?;
    let session_path = session_with_todos(&ws, 8)?;
    let mut tui = tui_builder(&ws, &server.uri()).spawn()?;
    tui.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    tui.send(keys::key::text(&format!(
        "/load {}",
        session_path.to_string_lossy()
    )))?;
    tui.wait_for_idle(Duration::from_millis(150), Duration::from_secs(3))?;
    tui.send(keys::key::enter())?;
    tui.wait_for_text("todo-workbar-00", Duration::from_secs(10))?;

    type_and_submit(&mut tui, PARENT_PROMPT)?;
    wait_for_counter(&mut tui, &child_requests, 2, INTERACTION_TIMEOUT)?;
    tui.wait_for_idle(Duration::from_millis(250), Duration::from_secs(6))?;

    let strip = work_bar_text(&mut tui);
    let worker = work_bar_worker_row(&mut tui);
    assert!(
        worker.is_some(),
        "a running sub-agent is not reachable in the top work bar while to-dos \
         occupy it — the strip painted only:\n{strip}\n---full---\n{}",
        tui.debug_dump()
    );
    let (row, col) = worker.expect("checked above");
    tui.send(keys::mouse::click(row, col))?;
    tui.wait_for_text("Agent Details", Duration::from_secs(5))
        .map_err(|_| {
            anyhow!(
                "clicking the sub-agent row did not open its detail\n{}",
                tui.debug_dump()
            )
        })?;

    let _ = tui.shutdown();
    Ok(())
}

/// The owner's *own* `~/.codewhale/settings.toml` (2026-08-04) carries
/// `rail_panel = "pinned"`. That is the configuration the "I spawned a sub
/// agent and the top bar showed nothing" screenshot was taken under, and it
/// is the one the other probes here miss: they all run on a sealed HOME with
/// no `settings.toml`, so they only ever exercise the default `tasks` panel.
///
/// With zero to-dos and no active goal the Pinned projection is empty, the
/// strip collapses to height 0, and a running sub-agent has nowhere in the
/// top bar to appear — there is no header chip or phase-strip fallback for
/// it. A panel choice must not be able to make running work uninspectable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn work_bar_shows_a_running_subagent_under_the_pinned_rail_panel() -> Result<()> {
    let _guard = WORK_BAR_PTY_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server).await;
    let child_requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ProbeResponder {
            child_requests: Arc::clone(&child_requests),
            parent_turns: Arc::new(AtomicUsize::new(0)),
            workers: 2,
            child_hold: Duration::from_secs(25),
        })
        .mount(&server)
        .await;

    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        "[subagents]\nmax_concurrent = 2\nlaunch_concurrency = 2\nmax_admitted = 2\n",
    )?;
    // Verbatim from the owner's settings.toml, minus the keys that do not
    // touch the rail.
    std::fs::write(
        ws.home().join(".codewhale").join("settings.toml"),
        "work_surface_placement = \"top\"\nwork_surface_top_height = 16\n\
         work_surface_side_width = 30\nrail_panel = \"pinned\"\n",
    )?;
    let mut tui = tui_builder(&ws, &server.uri()).spawn()?;
    tui.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    type_and_submit(&mut tui, PARENT_PROMPT)?;
    wait_for_counter(&mut tui, &child_requests, 2, INTERACTION_TIMEOUT)?;
    tui.wait_for_idle(Duration::from_millis(250), Duration::from_secs(6))?;

    let strip = work_bar_text(&mut tui);
    let worker = work_bar_worker_row(&mut tui);
    assert!(
        worker.is_some(),
        "a running sub-agent is invisible in the top bar under \
         `rail_panel = \"pinned\"` — the strip painted only:\n{strip}\n---full---\n{}",
        tui.debug_dump()
    );
    let (row, col) = worker.expect("checked above");
    tui.send(keys::mouse::click(row, col))?;
    tui.wait_for_text("Agent Details", Duration::from_secs(5))
        .map_err(|_| {
            anyhow!(
                "clicking the sub-agent row did not open its detail\n{}",
                tui.debug_dump()
            )
        })?;

    let _ = tui.shutdown();
    Ok(())
}

/// A finished agent must stay in the bar and stay clickable — quiet
/// completion, not eviction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn work_bar_keeps_a_finished_subagent_clickable() -> Result<()> {
    let _guard = WORK_BAR_PTY_LOCK.lock().await;
    let server = MockServer::start().await;
    mount_models(&server).await;
    let child_requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ProbeResponder {
            child_requests: Arc::clone(&child_requests),
            parent_turns: Arc::new(AtomicUsize::new(0)),
            workers: 1,
            child_hold: Duration::from_millis(0),
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
    wait_for_counter(&mut tui, &child_requests, 1, INTERACTION_TIMEOUT)?;
    // Let the child settle terminal and the parent turn finish.
    tui.wait_for_idle(Duration::from_millis(300), Duration::from_secs(10))?;

    let strip = work_bar_text(&mut tui);
    let worker = work_bar_worker_row(&mut tui);
    assert!(
        worker.is_some(),
        "a finished sub-agent disappeared from the top work bar; the strip \
         painted only:\n{strip}\n---full---\n{}",
        tui.debug_dump()
    );
    let (row, col) = worker.expect("checked above");
    tui.send(keys::mouse::click(row, col))?;
    tui.wait_for_text("Agent Details", Duration::from_secs(5))
        .map_err(|_| {
            anyhow!(
                "a finished sub-agent row is no longer a door\n{}",
                tui.debug_dump()
            )
        })?;

    let _ = tui.shutdown();
    Ok(())
}
