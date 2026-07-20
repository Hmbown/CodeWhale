//! End-to-end TUI scenarios driven through a real pseudo-terminal.
//!
//! Each scenario boots `deepseek-tui` in a sealed workspace + sealed `$HOME`,
//! sends scripted input through the PTY, and asserts on the parsed terminal
//! frame and on the workspace filesystem. See `support/qa_harness/README.md`
//! for design + how-to.
//!
//! These tests are gated to Unix for now. Windows ConPTY behaviour (#923,
//! #765, #802) needs a separate audit before scenarios light up there.

#![cfg(unix)]

#[path = "support/qa_harness/mod.rs"]
mod qa_harness;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use qa_harness::harness::{Harness, make_sealed_workspace};
use qa_harness::keys;

const BOOT_TIMEOUT: Duration = Duration::from_secs(15);
const KEY_TIMEOUT: Duration = Duration::from_secs(5);
const COMPOSER_READY_TEXT: &str = "Write a task";
static QA_PTY_TEST_LOCK: Mutex<()> = Mutex::new(());

fn qa_pty_test_lock() -> MutexGuard<'static, ()> {
    QA_PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn boot_minimal() -> anyhow::Result<(qa_harness::harness::SealedWorkspace, Harness)> {
    let ws = make_sealed_workspace()?;
    spawn_minimal_with_env(ws, &[])
}

fn boot_minimal_over_ssh() -> anyhow::Result<(qa_harness::harness::SealedWorkspace, Harness)> {
    let ws = make_sealed_workspace()?;
    spawn_minimal_with_env(ws, &[("SSH_CONNECTION", "192.0.2.10 51234 192.0.2.20 22")])
}

fn boot_minimal_without_retry() -> anyhow::Result<(qa_harness::harness::SealedWorkspace, Harness)> {
    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".deepseek").join("config.toml"),
        "[retry]\nenabled = false\n",
    )?;
    spawn_minimal_with_env(ws, &[])
}

fn spawn_minimal_with_env(
    ws: qa_harness::harness::SealedWorkspace,
    extra_env: &[(&str, &str)],
) -> anyhow::Result<(qa_harness::harness::SealedWorkspace, Harness)> {
    let mut builder = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        // Provide a stub key so the onboarding screen is bypassed and the TUI
        // boots straight into the composer. The harness never makes a live
        // request — we just need the binary to think a key exists.
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        // Force a known base URL so the doctor / model probe never escapes
        // the box. 127.0.0.1:1 will refuse instantly.
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        // PTY scenarios assert state transitions, not animation cadence. Freeze
        // ambient motion so wait_for_idle measures product state instead of a
        // decorative ocean frame.
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
        ])
        .size(40, 140);
    for (key, value) in extra_env {
        builder = builder.env(*key, *value);
    }
    let mut h = builder.spawn()?;
    enter_launch_session(&mut h)?;
    Ok((ws, h))
}

/// PTY scenarios exercise composer/runtime behavior. The default startup now
/// enters a session directly; users who explicitly enable `launch_screen`
/// retain the separate launch surface, covered by unit rendering tests.
fn enter_launch_session(h: &mut Harness) -> anyhow::Result<()> {
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    Ok(())
}

fn write_skill(root: std::path::PathBuf, name: &str, description: &str) -> anyhow::Result<()> {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nUse {name}.\n"),
    )?;
    Ok(())
}

fn spawn_approval_fixture_server() -> anyhow::Result<(String, std::thread::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut request_index = 0usize;
        while request_index < 2 && Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut request = [0u8; 64 * 1024];
            let _ = stream.read(&mut request);
            let body = if request_index == 0 {
                [
                    format!(
                        "data: {}\n\n",
                        serde_json::json!({
                            "id":"chatcmpl-approval",
                            "object":"chat.completion.chunk",
                            "model":"deepseek-v4-flash",
                            "choices":[{"index":0,"delta":{"tool_calls":[{
                                "index":0,
                                "id":"call_approval_pty",
                                "type":"function",
                                "function":{"name":"write_file","arguments":"{\"path\":\"approval-proof.txt\",\"content\":\"must-not-write\"}"}
                            }]},"finish_reason":null}]
                        })
                    ),
                    format!(
                        "data: {}\n\n",
                        serde_json::json!({
                            "id":"chatcmpl-approval",
                            "object":"chat.completion.chunk",
                            "model":"deepseek-v4-flash",
                            "choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],
                            "usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}
                        })
                    ),
                    "data: [DONE]\n\n".to_string(),
                ]
                .join("")
            } else {
                [
                    format!(
                        "data: {}\n\n",
                        serde_json::json!({
                            "id":"chatcmpl-denied",
                            "object":"chat.completion.chunk",
                            "model":"deepseek-v4-flash",
                            "choices":[{"index":0,"delta":{"content":"DENIAL-HONORED"},"finish_reason":null}]
                        })
                    ),
                    format!(
                        "data: {}\n\n",
                        serde_json::json!({
                            "id":"chatcmpl-denied",
                            "object":"chat.completion.chunk",
                            "model":"deepseek-v4-flash",
                            "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
                            "usage":{"prompt_tokens":20,"completion_tokens":4,"total_tokens":24}
                        })
                    ),
                    "data: [DONE]\n\n".to_string(),
                ]
                .join("")
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            request_index += 1;
        }
    });
    Ok((format!("http://{address}"), handle))
}

fn first_non_blank_row(frame: &qa_harness::Frame) -> Option<u16> {
    (0..frame.rows()).find(|&row| !frame.row(row).trim().is_empty())
}

fn assert_viewport_starts_at_top(frame: &qa_harness::Frame) {
    let dump = frame.debug_dump();
    let first_row = first_non_blank_row(frame).expect("expected visible frame text");
    assert_eq!(
        first_row, 0,
        "viewport content drifted below row 0:\n{dump}"
    );
    let header = frame.row(0).to_ascii_lowercase();
    assert!(
        header.contains("plan")
            || header.contains("act")
            || header.contains("agent")
            || header.contains("operate")
            || header.contains("yolo")
            || header.contains("deepseek"),
        "expected header content on row 0:\n{dump}"
    );
}

/// Smoke: the binary boots into an alt-screen, paints a composer, and the
/// header shows the project label. If this fails, the harness itself is
/// broken before we worry about any scenario.
#[test]
fn smoke_boot_paints_composer() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal()?;

    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;

    let f = h.frame();
    assert!(
        f.any_visible_text(),
        "expected non-empty frame after boot:\n{}",
        f.debug_dump()
    );

    let _ = h.shutdown();
    Ok(())
}

/// Returning users with a missing hosted-provider key must enter the normal
/// provider picker rather than a dead-end key screen. This runs with a sealed
/// HOME and no API key, so opening the picker is also proof that recovery does
/// not require a live provider request. Esc must not rewrite the configured
/// Kimi Code route.
#[test]
fn returning_missing_kimi_code_key_opens_picker() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    let config_path = ws.home().join(".codewhale").join("config.toml");
    let config_before = r#"provider = "moonshot"

[providers.moonshot]
base_url = "https://api.kimi.com/coding/v1"
model = "k3"
"#;
    std::fs::write(&config_path, config_before)?;
    std::fs::write(ws.home().join(".codewhale").join(".onboarded"), "")?;
    // This is a returning user who has already settled the independent setup
    // checkpoint. Keep that checkpoint out of the scenario so the assertion is
    // about missing-key recovery rather than a constitution-update modal.
    let mut setup_state = codewhale_config::SetupState::default();
    for step in [
        codewhale_config::SetupStep::Language,
        codewhale_config::SetupStep::TrustSandbox,
        codewhale_config::SetupStep::Constitution,
    ] {
        setup_state.set_step(
            step,
            codewhale_config::StepEntry::new(
                codewhale_config::StepStatus::Verified,
                true,
                "0.8.67",
            ),
        );
    }
    setup_state.set_step(
        codewhale_config::SetupStep::ProviderModel,
        codewhale_config::StepEntry::new(codewhale_config::StepStatus::NeedsAction, true, "0.8.67"),
    );
    setup_state.runtime_posture_source = codewhale_config::RuntimePostureSource::Confirmed;
    setup_state
        .complete_constitution_checkpoint("0.8.67", codewhale_config::ConstitutionChoice::Bundled);
    setup_state.constitution_source = codewhale_config::ConstitutionSource::Bundled;
    setup_state.save_to(
        &ws.home()
            .join(".codewhale")
            .join(codewhale_config::setup_state::SETUP_STATE_FILE_NAME),
    )?;
    std::fs::create_dir_all(ws.workspace().join(".deepseek"))?;
    std::fs::write(ws.workspace().join(".deepseek").join("trusted"), "")?;

    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
        ])
        .size(40, 160)
        .spawn()?;

    h.wait_for_text("Moonshot/Kimi", BOOT_TIMEOUT)?;
    h.wait_for_text("api.kimi.com", BOOT_TIMEOUT)?;
    h.wait_for_text("does not import Kimi CLI credentials", BOOT_TIMEOUT)?;

    // The picker rendered without mutating the configured route.
    assert_eq!(
        std::fs::read_to_string(&config_path)?,
        config_before,
        "opening recovery must leave the configured route unchanged"
    );

    let _ = h.shutdown();
    Ok(())
}

/// Esc from missing-key recovery must return a returning user to the offline
/// composer without mutating the saved route. The state transition is covered
/// by `back_from_provider_onboarding` unit behavior; this end-to-end leg is
/// ignored because the qa PTY harness exhibits input starvation during the
/// recovery boot window: instrumentation shows `run_event_loop` entered and
/// the terminal input pump spawned, yet `event::poll` never surfaces any byte
/// written to the PTY in this scenario (the same harness delivers input to
/// the composer/trust flows). Needs a dedicated investigation of boot-time
/// terminal queries vs. the non-responding test PTY.
#[test]
#[ignore = "qa PTY input starvation during recovery boot; see doc comment"]
fn returning_missing_kimi_code_key_esc_preserves_route() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    let config_path = ws.home().join(".codewhale").join("config.toml");
    let config_before = r#"provider = "moonshot"

[providers.moonshot]
base_url = "https://api.kimi.com/coding/v1"
model = "k3"
"#;
    std::fs::write(&config_path, config_before)?;
    std::fs::write(ws.home().join(".codewhale").join(".onboarded"), "")?;

    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
        ])
        .size(40, 160)
        .spawn()?;

    h.wait_for_text("api.kimi.com", BOOT_TIMEOUT)?;
    h.send(keys::key::esc())?;
    h.wait_for_text(COMPOSER_READY_TEXT, KEY_TIMEOUT)?;
    assert_eq!(
        std::fs::read_to_string(&config_path)?,
        config_before,
        "Esc from recovery must leave the configured route unchanged"
    );

    let _ = h.shutdown();
    Ok(())
}

/// Regression for v0.8.61 startup: the dispatcher-side config writer produced
/// camelCase keys plus `[features.enabled]`, while the TUI config reader only
/// accepted snake_case and flat `[features]` booleans. That failed before the
/// TUI log initialized and looked like an interactive launch crash from the
/// facade. Boot through a real PTY and prove early init reaches the trust
/// prompt and accepts input.
#[test]
fn interactive_init_accepts_input_with_dispatcher_written_config() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".codewhale").join("config.toml"),
        r#"
provider = "zai"
fallbackProviders = []
apiKey = "deepseek-test-key"
defaultTextModel = "deepseek-v4-pro"
authMode = "api_key"

[providers.zai]
apiKey = "zai-test-key"
authMode = "api_key"

[providers.zai.httpHeaders]

[features.enabled]
shell_tool = true
subagents = true
web_search = true
"#,
    )?;

    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
        ])
        .size(40, 140)
        .spawn()?;

    h.wait_for_text("Press Enter to continue", BOOT_TIMEOUT)?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Choose your language", BOOT_TIMEOUT)?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Know this workspace", BOOT_TIMEOUT)?;
    h.wait_for_text("Press 1/Y to trust and continue", BOOT_TIMEOUT)?;
    // Decline through the explicit trust hotkey. Enter's fail-closed behavior
    // is covered by the deterministic onboarding unit tests; this PTY leg is
    // the early-init/config compatibility sentinel and should not depend on a
    // transient status-toast redraw before proving input reaches the process.
    h.send(keys::key::ch('2'))?;
    assert_eq!(h.wait_for_exit(KEY_TIMEOUT), Some(0));
    Ok(())
}

/// Regression for #1085: after a turn exits through the error path, terminal
/// origin/scroll-region state must not leave blank rows above the TUI.
#[test]
fn viewport_origin_stays_row_zero_after_failed_turn() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal_without_retry()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    assert_viewport_starts_at_top(h.frame());

    h.send(keys::key::text("trigger a failed turn"))?;
    h.wait_for_idle(Duration::from_millis(200), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for(
        |frame| {
            frame.contains("Turn failed")
                || frame.contains("Connection refused")
                || frame.contains("error")
        },
        Duration::from_secs(15),
    )?;
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(3))?;
    assert_viewport_starts_at_top(h.frame());

    let _ = h.shutdown();
    Ok(())
}

/// Verifies the harness actually sees keystrokes — type a character and watch
/// it appear in the composer. This is the lowest-effort sanity check before
/// we lean on it for real scenarios.
#[test]
fn smoke_keystroke_reaches_composer() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;

    h.send(keys::key::text("hello-from-pty"))?;
    h.wait_for_text("hello-from-pty", KEY_TIMEOUT)?;

    let _ = h.shutdown();
    Ok(())
}

#[test]
fn printable_v_stays_in_composer_and_alt_help_fallback_works() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;

    h.send(keys::key::ch('v'))?;
    h.wait_for_text("v", KEY_TIMEOUT)?;
    assert!(
        h.frame().contains("v"),
        "bare v must remain composer-owned:\n{}",
        h.debug_dump()
    );
    h.send(b"\x15")?; // Ctrl+U clears the composer before testing Alt/Option.
    h.send(keys::key::alt('?'))?;
    h.wait_for(
        |frame| frame.contains("Help") || frame.contains("Keyboard") || frame.contains("Shortcuts"),
        KEY_TIMEOUT,
    )?;

    let _ = h.shutdown();
    Ok(())
}

#[test]
fn resize_and_mouse_wheel_preserve_composer_ownership() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
            "--mouse-capture",
        ])
        .size(40, 140)
        .spawn()?;
    enter_launch_session(&mut h)?;

    h.resize(24, 80)?;
    h.wait_for_idle(Duration::from_millis(200), Duration::from_secs(3))?;
    assert_eq!((h.frame().rows(), h.frame().cols()), (24, 80));
    h.send(keys::mouse::wheel_down(5, 40))?;
    h.send(keys::mouse::click(22, 20))?;
    h.send(keys::key::text("mouse-resize-proof"))?;
    h.wait_for_text("mouse-resize-proof", KEY_TIMEOUT)?;
    let dump = h.debug_dump();
    assert!(
        !dump.contains("[<65"),
        "mouse bytes leaked into composer:\n{dump}"
    );

    let _ = h.shutdown();
    Ok(())
}

#[test]
fn work_surface_real_rows_own_click_wheel_and_resize() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    let session_path = ws.workspace().join("mouse-work-session.json");
    let todos = (0..14)
        .map(|index| {
            serde_json::json!({
                "id": index + 1,
                "content": format!("todo-mouse-{index:02}"),
                "status": if index == 0 { "in_progress" } else { "pending" }
            })
        })
        .collect::<Vec<_>>();
    std::fs::write(
        &session_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "metadata": {
                "id": "pty-work-mouse",
                "title": "Mouse work surface",
                "created_at": "2026-07-13T00:00:00Z",
                "updated_at": "2026-07-13T00:00:00Z",
                "message_count": 0,
                "total_tokens": 0,
                "model": "deepseek-v4-pro",
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

    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
            "--mouse-capture",
            "--yolo",
        ])
        .size(32, 100)
        .spawn()?;
    enter_launch_session(&mut h)?;
    h.send(keys::key::text(&format!(
        "/load {}",
        session_path.to_string_lossy()
    )))?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("todo-mouse-00", KEY_TIMEOUT)?;

    let (first_row, first_col) = h
        .frame()
        .find_text("todo-mouse-00")
        .expect("real rendered first To-do row");
    for _ in 0..8 {
        h.send(keys::mouse::wheel_down(first_row, first_col))?;
        h.wait_for_idle(Duration::from_millis(40), Duration::from_secs(1))?;
    }
    h.wait_for_text("todo-mouse-13", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(200), Duration::from_secs(3))?;
    assert!(
        !h.debug_dump().contains("[<65"),
        "wheel over work surface leaked into the transcript/composer:\n{}",
        h.debug_dump()
    );

    h.resize(24, 80)?;
    h.wait_for_idle(Duration::from_millis(200), Duration::from_secs(3))?;
    h.wait_for_text("todo-mouse-13", KEY_TIMEOUT)?;
    let target = "todo-mouse-13";
    let (row, col) = h.frame().find_text(target).expect("row survived resize");
    h.send(keys::mouse::click(row, col))?;
    h.wait_for_text("Work", KEY_TIMEOUT)?;
    h.wait_for_text(target, KEY_TIMEOUT)?;
    h.wait_for_text("q/Esc close", KEY_TIMEOUT)?;
    let _ = h.shutdown();
    Ok(())
}

#[test]
fn approval_modal_keeps_wheel_for_review_and_denies_without_side_effect() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (base_url, server) = spawn_approval_fixture_server()?;
    let ws = make_sealed_workspace()?;
    let denied_path = ws.workspace().join("approval-proof.txt");
    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", &base_url)
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
            "--mouse-capture",
        ])
        .size(32, 100)
        .spawn()?;
    enter_launch_session(&mut h)?;

    let prompt = "Request the fixture write_file call; do not change its arguments.";
    // This is a whole prompt, not simulated human typing. Send it as the
    // bracketed paste a real terminal would emit so the raw-key paste-burst
    // heuristic cannot absorb the following Enter as a pasted newline.
    h.paste(prompt)?;
    h.wait_for_text(prompt, KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(100), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Approve once", Duration::from_secs(10))?;
    h.wait_for_text("Deny this call", KEY_TIMEOUT)?;

    let (deny_row, deny_col) = h
        .frame()
        .find_text("Deny this call")
        .expect("rendered denial option");
    h.send(keys::key::page_up())?;
    h.wait_for_text("❯ [1 / y]", KEY_TIMEOUT)?;
    h.send(keys::mouse::wheel_down(deny_row, deny_col))?;
    h.wait_for_text("❯ [1 / y]", KEY_TIMEOUT)?;
    h.resize(24, 80)?;
    h.wait_for(
        |frame| frame.rows() == 24 && frame.cols() == 80,
        KEY_TIMEOUT,
    )?;
    h.wait_for_idle(Duration::from_millis(200), Duration::from_secs(3))?;
    h.wait_for_text("Deny this call", KEY_TIMEOUT)?;
    let (deny_row, deny_col) = h
        .frame()
        .find_text("Deny this call")
        .expect("denial option survived resize");
    h.send(keys::mouse::wheel_down(deny_row, deny_col))?;
    h.wait_for_text("❯ [1 / y]", KEY_TIMEOUT)?;
    h.send(keys::key::down())?;
    h.send(keys::key::down())?;
    h.wait_for_text("❯ [3 / d / n]", KEY_TIMEOUT)?;
    h.send(keys::mouse::click(deny_row, deny_col))?;
    if let Err(err) = h.wait_for_text("DENIAL-HONORED", Duration::from_secs(10)) {
        let logs = std::fs::read_dir(ws.home().join(".codewhale/logs"))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(anyhow::anyhow!("{err:#}\napproval logs:\n{logs}"));
    }
    assert!(
        !denied_path.exists(),
        "denied approval executed its write_file side effect: {}",
        denied_path.display()
    );

    let _ = h.shutdown();
    server.join().expect("approval fixture server thread");
    Ok(())
}

/// Release stopship coverage: a real built TUI restores durable Work state and
/// keeps both Work and the effective permission posture visible at each
/// supported compact evidence size. No model turn is sent.
#[test]
fn work_and_permission_are_visible_at_release_terminal_sizes() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();

    for (cols, rows) in [(120_u16, 32_u16), (100, 30), (80, 24), (60, 16), (40, 12)] {
        let ws = make_sealed_workspace()?;
        let codewhale_home = ws.home().join(".codewhale");
        let codex_home = ws.home().join(".codex");
        std::fs::create_dir_all(&codex_home)?;
        std::fs::write(
            codewhale_home.join("config.toml"),
            "allow_shell = true\nreasoning_effort = \"low\"\n",
        )?;
        std::fs::write(
            codewhale_home.join("settings.toml"),
            "permission_posture = \"full-access\"\n",
        )?;
        std::fs::write(
            codex_home.join("models_cache.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "fetched_at": chrono::Utc::now(),
                "models": [{"slug": "gpt-pty-fixture", "priority": 1}]
            }))?,
        )?;

        let session_path = ws.workspace().join("release-work-session.json");
        std::fs::write(
            &session_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 1,
                "metadata": {
                    "id": format!("pty-{cols}x{rows}"),
                    "title": "Release Work continuity",
                    "created_at": "2026-07-10T00:00:00Z",
                    "updated_at": "2026-07-10T00:00:00Z",
                    "message_count": 0,
                    "total_tokens": 0,
                    "model": "deepseek-v4-pro",
                    "model_provider": "deepseek",
                    "workspace": ws.workspace(),
                    "mode": "operate",
                    "cost": {},
                    "cumulative_turn_secs": 0
                },
                "messages": [],
                "system_prompt": null,
                "work_state": {
                    "todos": {
                        "items": [
                            {"id": 1, "content": "persisted inspect", "status": "completed"},
                            {"id": 2, "content": "persisted patch", "status": "in_progress"}
                        ],
                        "completion_pct": 50,
                        "in_progress_id": 2
                    },
                    "plan": {
                        "objective": "Keep release Work visible",
                        "items": [
                            {"step": "verify PTY", "status": "in_progress"}
                        ]
                    }
                }
            }))?,
        )?;

        let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
            .cwd(ws.workspace())
            .clear_env()
            .seal_home(ws.home())
            .env("CODEWHALE_HOME", codewhale_home.to_string_lossy())
            .env(
                "DEEPSEEK_CONFIG_PATH",
                codewhale_home.join("config.toml").to_string_lossy(),
            )
            .env("CODEX_HOME", codex_home.to_string_lossy())
            .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
            .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
            .env("NO_ANIMATIONS", "1")
            .env("RUST_LOG", "warn")
            .args([
                "--workspace",
                ws.workspace().to_str().expect("utf-8 workspace path"),
                "--no-project-config",
                "--skip-onboarding",
            ])
            .size(rows, cols)
            .spawn()?;

        enter_launch_session(&mut h)?;
        h.send(keys::key::text(&format!(
            "/load {}",
            session_path.to_string_lossy()
        )))?;
        h.wait_for_text("/load", KEY_TIMEOUT)?;
        h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
        h.send(keys::key::enter())?;
        h.wait_for_text("Work", KEY_TIMEOUT)?;
        h.wait_for_text("Full Access", KEY_TIMEOUT)?;
        h.wait_for_idle(Duration::from_millis(250), Duration::from_secs(3))?;

        let frame = h.frame();
        let dump = frame.debug_dump();
        assert!(
            frame.contains("Work"),
            "Work missing at {cols}x{rows}:\n{dump}"
        );
        assert!(
            frame.contains("persisted") || frame.contains("2 items"),
            "Work state missing at {cols}x{rows}:\n{dump}"
        );
        assert!(
            frame.contains("Full Access"),
            "effective permission missing at {cols}x{rows}:\n{dump}"
        );
        assert!(
            frame.contains("Operate") || frame.contains("operate"),
            "restored mode missing at {cols}x{rows}:\n{dump}"
        );
        let header = frame.row(0);
        let effort_receipt = if cols >= 60 {
            " · high · Full Access"
        } else {
            " · h · Full Access"
        };
        assert!(
            header.contains(effort_receipt),
            "effective effort missing at {cols}x{rows}: {header:?}\n{dump}"
        );

        if let Some(dir) = std::env::var_os("CODEWHALE_QA_EVIDENCE_DIR") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join(format!("tui-{cols}x{rows}.txt")), dump)?;
        }

        let _ = h.shutdown();
    }
    Ok(())
}

/// WG6 integrated proof: a real TUI migrates legacy Plan/To-do state, records
/// Ctrl+T as a typed receipt, preserves it across explicit save/export/save,
/// and restores the same graph-backed Work state after process restart.
#[test]
fn legacy_work_ctrl_t_save_export_and_restart_are_consistent() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    let codewhale_home = ws.home().join(".codewhale");
    let codex_home = ws.home().join(".codex");
    std::fs::create_dir_all(&codex_home)?;
    std::fs::write(
        codewhale_home.join("config.toml"),
        "allow_shell = true\nreasoning_effort = \"low\"\n",
    )?;
    std::fs::write(
        codewhale_home.join("settings.toml"),
        "permission_posture = \"full-access\"\n",
    )?;
    std::fs::write(
        codex_home.join("models_cache.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "fetched_at": chrono::Utc::now(),
            "models": [{"slug": "gpt-pty-fixture", "priority": 1}]
        }))?,
    )?;

    let legacy_path = ws.workspace().join("legacy-work-session.json");
    std::fs::write(
        &legacy_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "metadata": {
                "id": "pty-wg6-legacy",
                "title": "WG6 legacy continuity",
                "created_at": "2026-07-10T00:00:00Z",
                "updated_at": "2026-07-10T00:00:00Z",
                "message_count": 0,
                "total_tokens": 0,
                "model": "deepseek-v4-pro",
                "model_provider": "deepseek",
                "workspace": ws.workspace(),
                "mode": "operate",
                "cost": {},
                "cumulative_turn_secs": 0
            },
            "messages": [],
            "system_prompt": null,
            "work_state": {
                "todos": {
                    "items": [
                        {"id": 1, "content": "persisted inspect", "status": "completed"},
                        {"id": 2, "content": "persisted patch", "status": "in_progress"}
                    ],
                    "completion_pct": 50,
                    "in_progress_id": 2
                },
                "plan": {
                    "objective": "Keep WG6 Work durable",
                    "items": [{"step": "verify integrated PTY", "status": "in_progress"}]
                }
            }
        }))?,
    )?;

    let spawn = || {
        Harness::builder(Harness::cargo_bin("codewhale-tui"))
            .cwd(ws.workspace())
            .clear_env()
            .seal_home(ws.home())
            .env("CODEWHALE_HOME", codewhale_home.to_string_lossy())
            .env(
                "DEEPSEEK_CONFIG_PATH",
                codewhale_home.join("config.toml").to_string_lossy(),
            )
            .env("CODEX_HOME", codex_home.to_string_lossy())
            .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
            .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
            .env("NO_ANIMATIONS", "1")
            .env("RUST_LOG", "warn")
            .args([
                "--workspace",
                ws.workspace().to_str().expect("utf-8 workspace path"),
                "--no-project-config",
                "--skip-onboarding",
            ])
            .size(16, 60)
            .spawn()
    };

    let mut h = spawn()?;
    enter_launch_session(&mut h)?;
    h.send(keys::key::text(&format!(
        "/load {}",
        legacy_path.to_string_lossy()
    )))?;
    h.wait_for_text("/load", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Work", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(250), Duration::from_secs(3))?;
    assert!(
        h.frame().contains("persisted"),
        "{}",
        h.frame().debug_dump()
    );

    h.send(b"\x14")?;
    h.wait_for_text("Reasoning effort: max", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    let cycled = h.frame();
    assert!(
        cycled.row(0).contains(" · max · Full Access"),
        "Ctrl+T effort missing from narrow header:\n{}",
        cycled.debug_dump()
    );
    assert!(cycled.contains("Work"), "{}", cycled.debug_dump());

    let before_path = ws.workspace().join("wg6-before-export.json");
    h.send(keys::key::text(&format!(
        "/save {}",
        before_path.to_string_lossy()
    )))?;
    h.wait_for_text("/save", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Session saved to", KEY_TIMEOUT)?;

    let export_path = ws.workspace().join("wg6-export.md");
    h.send(keys::key::text(&format!(
        "/export {}",
        export_path.to_string_lossy()
    )))?;
    h.wait_for_text("/export", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Conversation exported to", KEY_TIMEOUT)?;

    let after_path = ws.workspace().join("wg6-after-export.json");
    h.send(keys::key::text(&format!(
        "/save {}",
        after_path.to_string_lossy()
    )))?;
    h.wait_for_text("/save", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Session saved to", KEY_TIMEOUT)?;

    let deadline = Instant::now() + KEY_TIMEOUT;
    while (!before_path.exists() || !after_path.exists() || !export_path.exists())
        && Instant::now() < deadline
    {
        h.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(before_path.exists(), "first save was not written");
    assert!(after_path.exists(), "post-export save was not written");
    assert!(export_path.exists(), "export was not written");

    let before: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&before_path)?)?;
    let after: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&after_path)?)?;
    let work = &before["work_state"];
    assert!(work["graph"].is_object(), "migrated graph missing: {work}");
    assert_eq!(
        work["todos"]["items"][1]["content"], "persisted patch",
        "visible legacy To-do state drifted"
    );
    assert_eq!(
        work["plan"]["objective"], "Keep WG6 Work durable",
        "visible legacy Plan state drifted"
    );
    let activity = work["graph"]["activities"]
        .as_array()
        .and_then(|activities| activities.last())
        .expect("Ctrl+T Work activity");
    assert_eq!(activity["kind"], "reasoning_effort_changed");
    assert_eq!(activity["requested"], "max");
    assert_eq!(activity["effective"], "max");
    assert_eq!(activity["provider"], "deepseek");
    let receipt = activity.as_object().expect("typed activity object");
    for forbidden in ["text", "content", "reasoning", "reasoning_text"] {
        assert!(
            !receipt.contains_key(forbidden),
            "activity leaked forbidden field {forbidden}: {activity}"
        );
    }
    assert_eq!(
        before["work_state"], after["work_state"],
        "export mutated graph-backed Work state"
    );
    assert!(
        std::fs::read_to_string(&export_path)?.contains("# Codewhale conversation export"),
        "full export artifact missing"
    );
    let _ = h.shutdown();

    let mut restored = spawn()?;
    enter_launch_session(&mut restored)?;
    restored.send(keys::key::text(&format!(
        "/load {}",
        after_path.to_string_lossy()
    )))?;
    restored.wait_for_text("/load", KEY_TIMEOUT)?;
    restored.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    restored.send(keys::key::enter())?;
    restored.wait_for_text("Work", KEY_TIMEOUT)?;
    restored.wait_for_idle(Duration::from_millis(250), Duration::from_secs(3))?;
    let frame = restored.frame();
    assert!(frame.contains("persisted"), "{}", frame.debug_dump());
    assert!(
        frame.row(0).contains(" · high · Full Access"),
        "restart lost narrow effort/permission truth:\n{}",
        frame.debug_dump()
    );
    let _ = restored.shutdown();
    Ok(())
}

/// A composer `!` command is a host-owned shell turn. Cancelling it must
/// settle the transcript card instead of leaving a permanent `run running`
/// spinner after the process has been killed.
#[test]
fn cancelled_bang_shell_settles_transcript_card() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        // Match the Android/Termux release probe: `--skip-onboarding` with no
        // provider credential leaves the bang shell as the first transcript
        // cell, which is the cache-transition edge this regression covers.
        .env("DEEPSEEK_API_KEY", "")
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
            "--yolo",
        ])
        .size(32, 120)
        .spawn()?;

    enter_launch_session(&mut h)?;
    let command = "! echo $$ > shell.pid; sleep 30 & echo $! > sleep.pid; \
                   echo CWQA_SHELL_STARTED; wait";
    h.send(keys::key::text(command))?;
    h.wait_for_text("CWQA_SHELL_STARTED", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("run running", KEY_TIMEOUT)?;
    let process_deadline = std::time::Instant::now() + KEY_TIMEOUT;
    while (!ws.workspace().join("shell.pid").exists() || !ws.workspace().join("sleep.pid").exists())
        && std::time::Instant::now() < process_deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ws.workspace().join("shell.pid").exists());
    assert!(ws.workspace().join("sleep.pid").exists());

    h.send(b"\x03")?;
    h.wait_for_text("Request cancelled", KEY_TIMEOUT)?;
    h.wait_for(
        |frame| !frame.contains("run running"),
        Duration::from_secs(5),
    )?;
    h.wait_for_idle(Duration::from_millis(250), Duration::from_secs(5))?;

    let frame = h.frame();
    let dump = frame.debug_dump();
    assert!(
        !frame.contains("run running"),
        "cancelled bang shell stayed live in transcript:\n{dump}"
    );
    assert!(
        frame.contains("run issue") || frame.contains("interrupted"),
        "cancelled bang shell did not expose a terminal card:\n{dump}"
    );
    assert!(
        !frame.contains("turn completed"),
        "cancelled bang shell was reported as a completed turn:\n{dump}"
    );

    let _ = h.shutdown();
    Ok(())
}

/// Regression: `/skills` should reflect the same merged discovery set as the
/// slash menu and model-visible skills block, not just the first selected
/// skills directory.
#[test]
fn skills_menu_shows_local_and_global_skills() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    write_skill(ws.user_skills_dir(), "global-alpha", "Global alpha skill")?;
    write_skill(
        ws.workspace().join(".agents").join("skills"),
        "workspace-beta",
        "Workspace beta skill",
    )?;

    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
        ])
        .size(40, 140)
        .spawn()?;

    enter_launch_session(&mut h)?;
    h.send(keys::key::text("/skills"))?;
    h.wait_for_text("/skills", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Available skills", KEY_TIMEOUT)?;
    h.wait_for_text("global-alpha", KEY_TIMEOUT)?;
    h.wait_for_text("workspace-beta", KEY_TIMEOUT)?;

    let f = h.frame();
    let dump = f.debug_dump();
    assert!(f.contains("global-alpha"), "global skill missing:\n{dump}");
    assert!(
        f.contains("workspace-beta"),
        "workspace skill missing:\n{dump}"
    );

    let _ = h.shutdown();
    Ok(())
}

// ===========================================================================
// #1073 — pasting multi-line text with a trailing newline must NOT auto-submit
// ===========================================================================

/// Bracketed-paste path: terminal wraps the payload in `ESC[200~ … ESC[201~`,
/// crossterm delivers an `Event::Paste(text)`, and the TUI's bracketed path
/// inserts it into the composer. The trailing `\n` should leave the composer
/// holding the text, not start a turn.
#[test]
fn paste_bracketed_with_trailing_newline_does_not_autosubmit() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;

    // ~200 chars matching the original report. Trailing newline is the
    // payload that historically triggered the auto-submit.
    let payload = "first line of the multi-line paste body\n\
         second line continuing the paragraph until the end\n\
         third line that finishes with a trailing newline character\n";
    h.paste(payload)?;
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(2))?;

    let f = h.frame();
    let dump = f.debug_dump();

    // Auto-submit would replace the composer with a "working / thinking"
    // status chip and clear the composer text. Either signal indicates the
    // bug fired.
    assert!(
        !f.contains("Working") && !f.contains("thinking") && !f.contains("Thinking"),
        "bracketed paste with trailing newline auto-submitted:\n{dump}"
    );
    assert!(
        f.contains("first line") || f.contains("third line"),
        "pasted text should be visible in composer:\n{dump}"
    );

    let _ = h.shutdown();
    Ok(())
}

/// A macOS terminal's Cmd+V is handled on the client: it injects bracketed
/// paste bytes into the Linux SSH PTY. SSH detection must not divert that
/// event into the remote host clipboard path.
#[test]
fn paste_bracketed_from_macos_client_into_linux_ssh_stays_in_composer() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal_over_ssh()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;

    let payload = "mac-client-to-linux-host\nsecond-line-stays-in-composer";
    h.paste(payload)?;
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(2))?;

    let frame = h.frame();
    let dump = frame.debug_dump();
    assert!(
        frame.contains("mac-client-to-linux-host"),
        "SSH bracketed paste was not inserted:\n{dump}"
    );
    assert!(
        !frame.contains("Working") && !frame.contains("thinking"),
        "SSH bracketed paste unexpectedly submitted a turn:\n{dump}"
    );

    let _ = h.shutdown();
    Ok(())
}

/// End-to-end regression for SSH inside stock tmux: make a real Codewhale
/// selection, press the in-app Ctrl+C binding, and verify the text reaches the
/// tmux paste buffer through `load-buffer -w`. A stock `/dev/null` tmux config
/// keeps `allow-passthrough` off, which is the case the old DCS wrapper lost.
#[test]
fn copy_selection_over_ssh_uses_default_tmux_clipboard_path() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    if !Command::new("tmux")
        .arg("-V")
        .status()
        .is_ok_and(|status| status.success())
    {
        eprintln!("skipping SSH tmux PTY test: tmux is unavailable");
        return Ok(());
    }

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let socket = format!("codewhale-pty-{}-{nonce}", std::process::id());
    struct TmuxServer(String);
    impl Drop for TmuxServer {
        fn drop(&mut self) {
            let _ = Command::new("tmux")
                .args(["-L", self.0.as_str(), "kill-server"])
                .status();
        }
    }
    let server = TmuxServer(socket);
    let started = Command::new("tmux")
        .args([
            "-L",
            server.0.as_str(),
            "-f",
            "/dev/null",
            "new-session",
            "-d",
        ])
        .status()?;
    anyhow::ensure!(started.success(), "isolated tmux server failed to start");
    let tmux_env = Command::new("tmux")
        .args([
            "-L",
            server.0.as_str(),
            "display-message",
            "-p",
            "-t",
            "0",
            "#{socket_path},#{session_id},#{window_id}",
        ])
        .output()?;
    anyhow::ensure!(tmux_env.status.success(), "could not resolve TMUX value");
    let tmux_env = String::from_utf8(tmux_env.stdout)?.trim().to_string();
    let path = std::env::var("PATH").unwrap_or_default();

    let ws = make_sealed_workspace()?;
    let (_ws, mut h) = spawn_minimal_with_env(
        ws,
        &[
            ("SSH_CONNECTION", "192.0.2.10 51234 192.0.2.20 22"),
            ("TMUX", tmux_env.as_str()),
            ("PATH", path.as_str()),
        ],
    )?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;

    let text = "copy-over-ssh-tmux";
    h.send(keys::key::text(text))?;
    h.wait_for_text(text, KEY_TIMEOUT)?;
    let shift_left = b"\x1b[1;2D".repeat(text.chars().count());
    h.send(&shift_left)?;
    h.send(b"\x03")?; // Ctrl+C in raw mode.
    h.wait_for_text("Selection copied", KEY_TIMEOUT)?;

    let buffer = Command::new("tmux")
        .args(["-L", server.0.as_str(), "show-buffer"])
        .output()?;
    anyhow::ensure!(buffer.status.success(), "tmux buffer could not be read");
    assert_eq!(buffer.stdout, text.as_bytes());

    let _ = h.shutdown();
    Ok(())
}

/// Unbracketed-paste path: terminal does NOT wrap the payload, so crossterm
/// sees the bytes as ordinary keystrokes. The TUI's `paste_burst` detector is
/// supposed to recognize the rapid stream and treat it as a single paste, but
/// historically the trailing `\r` (Enter) of the burst leaks through and
/// triggers submit while the burst flush dumps the text into the now-empty
/// composer.
///
/// This is the Windows / PowerShell repro from #1073.
#[test]
fn paste_unbracketed_with_trailing_newline_does_not_autosubmit() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    // Let the boot fully settle so input handling is wired up.
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(3))?;

    let payload = "first line of the multi-line paste body\n\
         second line continuing the paragraph until the end\n\
         third line that finishes with a trailing newline character\n";
    h.paste_unbracketed(payload)?;
    h.wait_for_idle(Duration::from_millis(400), Duration::from_secs(3))?;

    let f = h.frame();
    let dump = f.debug_dump();
    eprintln!("=== AFTER UNBRACKETED PASTE ===\n{dump}");

    // The visible signal of an auto-submit: the text appears in the
    // transcript above the composer (sent as a user message). The composer
    // is also typically reset, but #1073 reports residual text in addition
    // to the auto-submit, so checking the transcript is more reliable.
    let count = dump.matches("first line").count();
    assert!(
        count <= 1,
        "'first line' appears {count} times — auto-submitted into transcript AND \
         composer:\n{dump}"
    );
    // And the pasted text should be visible somewhere.
    assert!(
        f.contains("first line"),
        "pasted text should be on-screen somewhere:\n{dump}"
    );

    let _ = h.shutdown();
    Ok(())
}

/// A loopback SSE fixture that answers the first chat request with one long
/// assistant message so the transcript exceeds several viewports. Later
/// requests get a short stop so the server thread always drains.
fn spawn_long_reply_fixture(
    content: String,
) -> anyhow::Result<(String, std::thread::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut served = 0usize;
        while served < 4 && Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut request = [0u8; 64 * 1024];
            let _ = stream.read(&mut request);
            let reply = if served == 0 {
                content.as_str()
            } else {
                "SCROLLPROBE-EXTRA"
            };
            let body = [
                format!(
                    "data: {}\n\n",
                    serde_json::json!({
                        "id":"chatcmpl-scroll",
                        "object":"chat.completion.chunk",
                        "model":"deepseek-v4-flash",
                        "choices":[{"index":0,"delta":{"content":reply},"finish_reason":null}]
                    })
                ),
                format!(
                    "data: {}\n\n",
                    serde_json::json!({
                        "id":"chatcmpl-scroll",
                        "object":"chat.completion.chunk",
                        "model":"deepseek-v4-flash",
                        "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14}
                    })
                ),
                "data: [DONE]\n\n".to_string(),
            ]
            .join("");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            served += 1;
        }
    });
    Ok((format!("http://{address}"), handle))
}

enum ScrollDir {
    Up,
    Down,
}

/// Scroll the transcript one step at a time — letting each step settle past the
/// input-coalescing/redraw throttle — until `needle` is on-screen. Each step
/// sends both a page key and a wheel event so it works regardless of which the
/// transcript honors. Returns whether the needle became visible.
fn scroll_until(h: &mut Harness, dir: ScrollDir, needle: &str) -> bool {
    if h.frame().contains(needle) {
        return true;
    }
    for _ in 0..50 {
        match dir {
            ScrollDir::Up => {
                let _ = h.send(keys::key::page_up());
                let _ = h.send(keys::mouse::wheel_up(10, 10));
            }
            ScrollDir::Down => {
                let _ = h.send(keys::mouse::wheel_down(10, 10));
                let _ = h.send(keys::mouse::wheel_down(10, 10));
            }
        }
        let _ = h.wait_for_idle(Duration::from_millis(60), Duration::from_millis(400));
        if h.frame().contains(needle) {
            return true;
        }
    }
    false
}

/// #4603: long transcript output must be retained beyond the viewport and
/// remain reviewable by scrolling, with follow-tail restored on return to the
/// bottom. Provider-free: the reply is a sealed loopback SSE fixture.
#[test]
fn long_output_scrolls_and_restores_follow_tail() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();

    // A reply well over three 24-row viewports: a head marker, ~90 numbered
    // lines, a very wide line (horizontal overflow), and a tail marker.
    let mut lines = vec!["SCROLLPROBE-HEAD".to_string()];
    for i in 1..=90 {
        lines.push(format!("SCROLLPROBE-LINE-{i:03}"));
    }
    lines.push(format!("SCROLLPROBE-WIDE-START{}WIDE-END", "x".repeat(200)));
    lines.push("SCROLLPROBE-TAIL".to_string());
    let content = lines.join("\n");

    let (base_url, server) = spawn_long_reply_fixture(content)?;
    let ws = make_sealed_workspace()?;
    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", &base_url)
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
            "--mouse-capture",
        ])
        .size(24, 100)
        .spawn()?;
    enter_launch_session(&mut h)?;

    // One turn that produces the long reply.
    let prompt = "Emit the long scroll probe.";
    h.paste(prompt)?;
    h.wait_for_text(prompt, KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(100), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;

    // The tail lands in view (follow-tail) and the head has scrolled off:
    // the content exists beyond the viewport rather than being truncated away.
    h.wait_for_text("SCROLLPROBE-TAIL", Duration::from_secs(10))?;
    assert!(
        !h.frame().contains("SCROLLPROBE-HEAD"),
        "head should be above the viewport once the long reply settles:\n{}",
        h.frame().debug_dump()
    );

    // Scroll up: the retained head becomes reviewable and the tail leaves view.
    // Scroll incrementally, letting each step settle — the TUI coalesces a
    // rapid input burst, so one page/wheel event at a time is what a real user
    // (and a reliable test) applies.
    assert!(
        scroll_until(&mut h, ScrollDir::Up, "SCROLLPROBE-HEAD"),
        "head must be reachable by scrolling up:\n{}",
        h.frame().debug_dump()
    );
    assert!(
        !h.frame().contains("SCROLLPROBE-TAIL"),
        "scrolled away from the tail, so the tail marker should be gone:\n{}",
        h.frame().debug_dump()
    );

    // Resize (reflow) preserves the ability to review earlier content.
    h.resize(30, 80)?;
    h.wait_for(|f| f.rows() == 30 && f.cols() == 80, KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(200), Duration::from_secs(3))?;
    assert!(
        h.frame().contains("SCROLLPROBE-HEAD")
            || scroll_until(&mut h, ScrollDir::Up, "SCROLLPROBE-HEAD"),
        "head must stay reviewable after a reflow:\n{}",
        h.frame().debug_dump()
    );

    // Returning to the bottom restores follow-tail.
    assert!(
        scroll_until(&mut h, ScrollDir::Down, "SCROLLPROBE-TAIL"),
        "follow-tail must be restorable by scrolling back to the bottom:\n{}",
        h.frame().debug_dump()
    );

    let _ = h.shutdown();
    server.join().expect("scroll fixture server thread");
    Ok(())
}
