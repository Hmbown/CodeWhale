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

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

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
    spawn_minimal(ws)
}

fn boot_minimal_without_retry() -> anyhow::Result<(qa_harness::harness::SealedWorkspace, Harness)> {
    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".deepseek").join("config.toml"),
        "[retry]\nenabled = false\n",
    )?;
    spawn_minimal(ws)
}

fn spawn_minimal(
    ws: qa_harness::harness::SealedWorkspace,
) -> anyhow::Result<(qa_harness::harness::SealedWorkspace, Harness)> {
    let h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
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
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
        ])
        .size(40, 140)
        .spawn()?;
    Ok((ws, h))
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
    assert!(
        frame.row(0).contains("Plan")
            || frame.row(0).contains("Agent")
            || frame.row(0).contains("Yolo")
            || frame.row(0).contains("DeepSeek"),
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
    h.wait_for_text("Trust Workspace", BOOT_TIMEOUT)?;
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
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
        ])
        .size(40, 140)
        .spawn()?;

    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
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
