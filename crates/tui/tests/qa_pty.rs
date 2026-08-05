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
use sha2::{Digest, Sha256};
use unicode_width::UnicodeWidthStr;

const BOOT_TIMEOUT: Duration = Duration::from_secs(15);
const KEY_TIMEOUT: Duration = Duration::from_secs(5);
const SKILL_SCAN_TIMEOUT: Duration = Duration::from_secs(15);
const COMPOSER_READY_TEXT: &str = "Write a task";
/// Operate-mode composer placeholder, pinned as shipped copy since the
/// goal-first placeholder rewording (bf0478395): the mode ramp legs below
/// assert the exact user-visible text, not a substring of it.
const OPERATE_COMPOSER_TEXT: &str = "Describe the goal — Codewhale keeps working until it's done";
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
        "[retry]\nenabled = false\n\n[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n",
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

#[test]
fn composer_newline_and_stash_chords_keep_stable_roles() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (ws, mut h) = boot_minimal()?;

    h.send(keys::key::text("shift-line"))?;
    h.send(keys::key::shift_enter())?;
    h.send(keys::key::text("alt-line"))?;
    h.send(keys::key::alt_enter())?;
    h.send(keys::key::text("ctrl-j-line"))?;
    h.send(keys::key::ctrl_j())?;
    h.send(keys::key::text("last-line"))?;
    h.wait_for_text("last-line", KEY_TIMEOUT)?;

    let frame = h.frame();
    let rows = ["shift-line", "alt-line", "ctrl-j-line", "last-line"].map(|line| {
        frame
            .find_text(line)
            .expect("multiline draft stays visible")
            .0
    });
    assert!(
        rows.windows(2).all(|pair| pair[0] < pair[1]),
        "Shift+Enter, Alt+Enter, and Ctrl+J must each add a line:\n{}",
        frame.debug_dump()
    );

    h.send(keys::key::ctrl_g())?;
    h.wait_for_text("Draft stashed", KEY_TIMEOUT)?;
    h.wait_for_text(COMPOSER_READY_TEXT, KEY_TIMEOUT)?;
    let stash_path = ws.home().join(".codewhale/composer_stash.jsonl");
    let first_stash = std::fs::read_to_string(&stash_path)?;
    assert!(first_stash.contains("shift-line\\nalt-line\\nctrl-j-line\\nlast-line"));

    h.send(keys::key::text("/stash pop"))?;
    h.wait_for_text("/stash pop", KEY_TIMEOUT)?;
    std::thread::sleep(Duration::from_millis(180));
    h.send(keys::key::enter())?;
    h.wait_for_text("last-line", KEY_TIMEOUT)?;

    h.send(keys::key::ctrl_s())?;
    h.wait_for_text(COMPOSER_READY_TEXT, KEY_TIMEOUT)?;
    let second_stash = std::fs::read_to_string(&stash_path)?;
    assert!(second_stash.contains("shift-line\\nalt-line\\nctrl-j-line\\nlast-line"));

    let _ = h.shutdown();
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
                                "function":{"name":"File","arguments":"{\"action\":\"write\",\"path\":\"approval-proof.txt\",\"content\":\"must-not-write\"}"}
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

fn visible_row_with_text(frame: &qa_harness::Frame, needle: &str) -> Option<u16> {
    (0..frame.rows()).find(|&row| frame.row(row).contains(needle))
}

fn foreground_at_text(frame: &qa_harness::Frame, row: u16, needle: &str) -> qa_harness::Color {
    let col = frame
        .find_text_in_row(row, needle)
        .unwrap_or_else(|| panic!("{needle:?} missing from row {row}: {:?}", frame.row(row)));
    frame
        .colors_at(row, col)
        .unwrap_or_else(|| panic!("missing terminal cell at ({row}, {col})"))
        .0
}

fn composer_edge_rows(frame: &qa_harness::Frame, placeholder: &str) -> (u16, u16) {
    let input_row = visible_row_with_text(frame, placeholder)
        .unwrap_or_else(|| panic!("composer placeholder {placeholder:?} missing"));
    let minimum_rule_cells = usize::from(frame.cols() / 2);
    let is_rule = |row: u16| {
        frame
            .row(row)
            .chars()
            .filter(|ch| {
                matches!(
                    ch,
                    '-' | '─' | '━' | '╌' | '╍' | '┄' | '┅' | '┈' | '┉' | '═'
                )
            })
            .count()
            >= minimum_rule_cells
    };
    let top = (0..input_row)
        .rev()
        .find(|&row| is_rule(row))
        .expect("composer top edge");
    let bottom = (input_row.saturating_add(1)..frame.rows())
        .find(|&row| is_rule(row))
        .expect("composer bottom edge");
    (top, bottom)
}

/// Assert the user-visible labels and the split composer edges tell the same
/// agency/permission story in the ANSI cells emitted through the real PTY.
fn assert_control_grammar(
    frame: &qa_harness::Frame,
    mode: &str,
    permission: &str,
    placeholder: &str,
) -> (qa_harness::Color, qa_harness::Color) {
    let dump = frame.debug_dump();
    let header = frame.row(0);
    assert!(
        header.contains(mode),
        "mode {mode:?} missing from header {header:?}:\n{dump}"
    );
    assert!(
        header.contains(permission),
        "permission {permission:?} missing from header {header:?}:\n{dump}"
    );
    let mode_color = foreground_at_text(frame, 0, mode);
    let permission_color = foreground_at_text(frame, 0, permission);
    let (permission_edge, mode_edge) = composer_edge_rows(frame, placeholder);
    assert_eq!(
        frame
            .colors_at(permission_edge, 1)
            .expect("permission edge cell")
            .0,
        permission_color,
        "header permission and composer top edge diverged:\n{dump}"
    );
    assert_eq!(
        frame.colors_at(mode_edge, 1).expect("mode edge cell").0,
        mode_color,
        "header mode and composer bottom edge diverged:\n{dump}"
    );
    (mode_color, permission_color)
}

fn assert_real_pty_frame_geometry(frame: &qa_harness::Frame, cols: u16, rows: u16) {
    let dump = frame.debug_dump();
    assert_eq!(frame.cols(), cols, "parsed PTY width changed:\n{dump}");
    assert_eq!(frame.rows(), rows, "parsed PTY height changed:\n{dump}");
    let (cursor_row, cursor_col) = frame.cursor();
    assert!(
        cursor_row < rows && cursor_col < cols,
        "cursor escaped {cols}x{rows}: ({cursor_row}, {cursor_col})\n{dump}"
    );
    for row in 0..rows {
        let width = UnicodeWidthStr::width(frame.row(row).as_str());
        assert!(
            width <= usize::from(cols),
            "row {row} clips at width {width} in {cols}x{rows}:\n{dump}"
        );
    }
    for fatal in [
        "panicked at",
        "fatal runtime error",
        "thread 'main' panicked",
    ] {
        assert!(!frame.contains(fatal), "TUI exposed {fatal:?}:\n{dump}");
    }
}

fn assert_empty_state_hierarchy(frame: &qa_harness::Frame, ascii_safe: bool) {
    let dump = frame.debug_dump();
    let context = visible_row_with_text(frame, "codewhale").expect("empty-state context row");
    let composer = visible_row_with_text(frame, COMPOSER_READY_TEXT).expect("composer row");
    assert!(
        context < composer,
        "empty-state facts must precede the composer:\n{dump}"
    );
    if let Some(fleet) = visible_row_with_text(frame, "Fleet ready") {
        assert!(
            context < fleet && fleet < composer,
            "Fleet action must follow context and precede the composer:\n{dump}"
        );
        if let Some(help) = visible_row_with_text(frame, "/help") {
            assert!(
                fleet < help && help < composer,
                "optional help must follow Fleet and precede the composer:\n{dump}"
            );
        }
    } else {
        assert!(
            frame.rows() <= 12,
            "only the 12-row compact tier may shed the Fleet action:\n{dump}"
        );
    }

    let whale_row = (2..context).find(|&row| {
        let text = frame.row(row);
        if ascii_safe {
            text.chars().filter(|ch| *ch == '#').count() >= 8
        } else {
            text.chars()
                .filter(|ch| {
                    matches!(
                        ch,
                        '█' | '▄' | '▀' | '▗' | '▖' | '▙' | '▝' | '▚' | '▞' | '▐'
                    )
                })
                .count()
                >= 8
        }
    });
    // The idle whale is earned wherever the transcript can seat it, and the
    // rail now yields the rows rather than evicting it. 24 rows is the
    // shipped release-evidence size and the pre-rail contract; it stands.
    if frame.cols() >= 80 && frame.rows() >= 24 {
        assert!(
            whale_row.is_some(),
            "idle whale missing where the terminal earns decorative water:\n{dump}"
        );
    }
    if let Some(row) = whale_row {
        assert!(
            row < context,
            "idle whale must yield before functional empty-state facts:\n{dump}"
        );
    }
}

fn write_real_pty_evidence(
    name: &str,
    metadata: &str,
    frame: &qa_harness::Frame,
) -> anyhow::Result<()> {
    write_real_pty_evidence_dump(name, metadata, &frame.debug_dump())
}

fn write_real_pty_evidence_dump(
    name: &str,
    metadata: &str,
    frame_dump: &str,
) -> anyhow::Result<()> {
    let Some(dir) = std::env::var_os("CODEWHALE_QA_EVIDENCE_DIR") else {
        return Ok(());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(format!("v091-{name}.txt")),
        format!("real_pty=true\n{metadata}\n\n{frame_dump}"),
    )?;
    Ok(())
}

/// Capture the exact semantic frame that satisfies `predicate`. Animated
/// redraws may emit a clear and the replacement composition in separate PTY
/// drains, so pumping once more after `wait_for` can observe the in-between
/// clear instead of the product frame that actually met the assertion.
fn wait_for_frame_dump<F>(
    h: &mut Harness,
    mut predicate: F,
    timeout: Duration,
) -> anyhow::Result<String>
where
    F: FnMut(&qa_harness::Frame) -> bool,
{
    let mut captured = None;
    h.wait_for(
        |frame| {
            let matches = predicate(frame);
            if matches {
                captured = Some(frame.debug_dump());
            }
            matches
        },
        timeout,
    )?;
    Ok(captured.expect("matching PTY frame must be captured"))
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

/// v0.9.1 visual stopship: exercise the shipped shell through a real PTY at
/// every release evidence size. This is parsed terminal output, not a test
/// renderer or generated product image.
#[test]
fn v091_real_pty_visual_matrix_preserves_control_grammar() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let cases = [
        (40_u16, 12_u16, "terminal", false),
        (60, 16, "grayscale", false),
        (80, 24, "dark", true),
        (100, 30, "light", false),
        (140, 40, "dark", false),
    ];
    let mut theme_signatures = Vec::<(&str, String)>::new();

    for (cols, rows, theme, ascii_safe) in cases {
        let ws = make_sealed_workspace()?;
        let codewhale_home = ws.home().join(".codewhale");
        let codex_home = ws.home().join(".codex");
        std::fs::create_dir_all(&codex_home)?;
        std::fs::write(
            codewhale_home.join("config.toml"),
            "reasoning_effort = \"low\"\n\n[update]\ncheck_for_updates = false\n\n[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n",
        )?;
        std::fs::write(
            codewhale_home.join("settings.toml"),
            format!(
                "theme = \"{theme}\"\nlocale = \"en\"\ndefault_mode = \"agent\"\npermission_posture = \"ask\"\nlow_motion = false\nfancy_animations = true\ncomposer_border = true\n"
            ),
        )?;
        std::fs::write(
            codex_home.join("models_cache.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "fetched_at": chrono::Utc::now(),
                "models": [{"slug": "gpt-pty-fixture", "priority": 1}]
            }))?,
        )?;

        let mut builder = Harness::builder(Harness::cargo_bin("codewhale-tui"))
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
            // This runtime overlay must win over the saved animation opt-in.
            .env("NO_ANIMATIONS", "1")
            .env("RUST_LOG", "warn")
            .args([
                "--workspace",
                ws.workspace().to_str().expect("utf-8 workspace path"),
                "--no-project-config",
                "--skip-onboarding",
            ])
            .size(rows, cols);
        if ascii_safe {
            builder = builder.env("CODEWHALE_ASCII_SAFE", "1");
        }
        let mut h = builder.spawn()?;
        enter_launch_session(&mut h)?;
        h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(3))?;

        let first = h.frame().text();
        // Motion evidence needs two real frames separated in wall-clock time.
        std::thread::sleep(Duration::from_millis(450));
        h.pump();
        let second = h.frame().text();
        assert_eq!(
            first, second,
            "NO_ANIMATIONS frame moved at {cols}x{rows} ({theme})"
        );

        {
            let frame = h.frame();
            let dump = frame.debug_dump();
            assert_real_pty_frame_geometry(frame, cols, rows);
            assert_empty_state_hierarchy(frame, ascii_safe);
            assert_control_grammar(frame, "act", "ask", COMPOSER_READY_TEXT);
            if ascii_safe {
                assert!(
                    frame.text().is_ascii(),
                    "ASCII-safe PTY emitted non-ASCII cells:\n{dump}"
                );
            }

            let signature = format!("{:?}", frame.colors_at(0, 0).expect("header mark cell"));
            if let Some((_, previous)) = theme_signatures
                .iter()
                .find(|(previous_theme, _)| *previous_theme == theme)
            {
                assert_eq!(
                    &signature, previous,
                    "theme {theme} changed ANSI signature with terminal size"
                );
            } else {
                for (previous_theme, previous) in &theme_signatures {
                    assert_ne!(
                        &signature, previous,
                        "themes {previous_theme} and {theme} emitted the same ANSI signature"
                    );
                }
                theme_signatures.push((theme, signature));
            }
            write_real_pty_evidence(
                &format!(
                    "matrix-{theme}-{cols}x{rows}{}",
                    if ascii_safe { "-ascii" } else { "" }
                ),
                &format!(
                    "size={cols}x{rows}\ntheme={theme}\nmode=act\npermission=ask\nreduced_motion=true\nascii_safe={ascii_safe}"
                ),
                frame,
            )?;
        }

        // Prove the cool agency ramp and warm permission ramp end to end in
        // both dark and light themes. Header labels and split composer edges
        // must change together, and each state must retain its own ANSI color.
        if cols == 140 || theme == "light" {
            let (act, ask) = assert_control_grammar(h.frame(), "act", "ask", COMPOSER_READY_TEXT);

            h.send(b"\t")?;
            h.wait_for(
                |frame| frame.row(0).contains("operate") && frame.contains(OPERATE_COMPOSER_TEXT),
                KEY_TIMEOUT,
            )?;
            let (operate, _) =
                assert_control_grammar(h.frame(), "operate", "ask", OPERATE_COMPOSER_TEXT);
            write_real_pty_evidence(
                &format!("agency-operate-{theme}-{cols}x{rows}"),
                &format!(
                    "size={cols}x{rows}\ntheme={theme}\nmode=operate\npermission=ask\nreduced_motion=true\nascii_safe=false"
                ),
                h.frame(),
            )?;

            h.send(b"\t")?;
            h.wait_for(
                |frame| {
                    frame.row(0).contains("plan")
                        && frame.row(0).contains("read only")
                        && frame.contains(COMPOSER_READY_TEXT)
                },
                KEY_TIMEOUT,
            )?;
            let (plan, _) =
                assert_control_grammar(h.frame(), "plan", "read only", COMPOSER_READY_TEXT);
            write_real_pty_evidence(
                &format!("agency-plan-{theme}-{cols}x{rows}"),
                &format!(
                    "size={cols}x{rows}\ntheme={theme}\nmode=plan\npermission=read-only\nreduced_motion=true\nascii_safe=false"
                ),
                h.frame(),
            )?;
            assert_ne!(plan, act, "Plan and Act collapsed to one ANSI color");
            assert_ne!(
                plan, operate,
                "Plan and Operate collapsed to one ANSI color"
            );
            assert_ne!(act, operate, "Act and Operate collapsed to one ANSI color");

            h.send(b"\t")?;
            h.wait_for(
                |frame| {
                    frame.row(0).contains("act")
                        && frame.row(0).contains("ask")
                        && frame.contains(COMPOSER_READY_TEXT)
                },
                KEY_TIMEOUT,
            )?;
            assert_control_grammar(h.frame(), "act", "ask", COMPOSER_READY_TEXT);

            h.send(keys::key::backtab())?;
            h.wait_for(
                |frame| frame.row(0).contains("act") && frame.row(0).contains("auto"),
                KEY_TIMEOUT,
            )?;
            let (_, auto) = assert_control_grammar(h.frame(), "act", "auto", COMPOSER_READY_TEXT);
            write_real_pty_evidence(
                &format!("permission-auto-{theme}-{cols}x{rows}"),
                &format!(
                    "size={cols}x{rows}\ntheme={theme}\nmode=act\npermission=auto\nreduced_motion=true\nascii_safe=false"
                ),
                h.frame(),
            )?;

            h.send(keys::key::backtab())?;
            h.wait_for(
                |frame| frame.row(0).contains("act") && frame.row(0).contains("Full Access"),
                KEY_TIMEOUT,
            )?;
            let (_, full_access) =
                assert_control_grammar(h.frame(), "act", "Full Access", COMPOSER_READY_TEXT);
            write_real_pty_evidence(
                &format!("permission-full-access-{theme}-{cols}x{rows}"),
                &format!(
                    "size={cols}x{rows}\ntheme={theme}\nmode=act\npermission=full-access\nreduced_motion=true\nascii_safe=false"
                ),
                h.frame(),
            )?;
            assert_ne!(ask, auto, "Ask and Auto collapsed to one ANSI color");
            assert_ne!(
                ask, full_access,
                "Ask and Full Access collapsed to one ANSI color"
            );
            assert_ne!(
                auto, full_access,
                "Auto and Full Access collapsed to one ANSI color"
            );
        }

        if let Some(status) = h.wait_for_exit(Duration::from_millis(1)) {
            return Err(anyhow::anyhow!(
                "TUI exited during {cols}x{rows} {theme} matrix case with {status}:\n{}",
                h.debug_dump()
            ));
        }
        let _ = h.shutdown();
    }

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
    // A returning user has also answered the first-run telemetry notice. Without
    // this the notice is owed, and it renders on the TTY and blocks on stdin
    // before the TUI starts — which would make this a test about the notice
    // rather than about missing-key recovery.
    setup_state.record_telemetry_notice(codewhale_config::TELEMETRY_NOTICE_VERSION, false);
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
    h.wait_for_text("missing MOONSHOT_API_KEY / KIMI_API_KEY", BOOT_TIMEOUT)?;

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

    // The first-run telemetry notice is the first thing an interactive launch
    // shows, before the terminal enters raw mode. Enter takes the pre-selected
    // "No thanks" — which is the whole point of the pre-selection, and is what
    // this harness exercises: a user who presses Enter through onboarding never
    // enables telemetry.
    h.wait_for_text("Enable telemetry?", BOOT_TIMEOUT)?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Press Enter to continue", BOOT_TIMEOUT)?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Choose your language", BOOT_TIMEOUT)?;
    h.send(keys::key::enter())?;
    // The Appearance step (#3937) sits between language and trust; Enter
    // keeps the current theme and advances. This is also the only PTY-level
    // execution of that step's event-loop wiring, so a hang here is a real
    // wiring bug, not a script gap.
    h.wait_for_text("Make It Yours", BOOT_TIMEOUT)?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Know this workspace", BOOT_TIMEOUT)?;
    h.wait_for_text("Press 1/Y to trust and continue", BOOT_TIMEOUT)?;
    // Decline through the explicit quit hotkey: since bf0478395 the number
    // keys mirror the footer's reading order (1 trust, 2 continue untrusted,
    // 3 quit), so 3/N/Esc is the decline-and-exit leg. Enter's fail-closed
    // behavior is covered by the deterministic onboarding unit tests; this
    // PTY leg is the early-init/config compatibility sentinel and should not
    // depend on a transient status-toast redraw before proving input reaches
    // the process.
    h.send(keys::key::ch('3'))?;
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

    // The top Work surface auto-fits its row count to the plan (capped), so
    // the divider is not a fixed offset from the first selectable row — find
    // the rendered horizontal rule itself. Send genuine SGR down/drag/up
    // bytes and prove the resized surface exposes additional real rows
    // before exercising scroll.
    let divider_row = (first_row..first_row.saturating_add(20))
        .find(|&y| {
            h.frame()
                .row(y)
                .chars()
                .filter(|&c| c == '─' || c == '━')
                .count()
                >= 20
        })
        .expect("rendered divider row below the to-do rows");
    h.send(keys::mouse::down(divider_row, first_col))?;
    h.send(keys::mouse::drag(divider_row.saturating_add(4), first_col))?;
    h.send(keys::mouse::up(divider_row.saturating_add(4), first_col))?;
    h.wait_for_text("todo-mouse-04", KEY_TIMEOUT)?;

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

/// Owner-reported (2026-08-04): "you cannot click a work-bar row AT ALL".
/// The bare-session click test above passes, so this probe reproduces the
/// dogfood shape it does not cover: an active goal title occupying the
/// strip's header row, then a real SGR click on a to-do row. The click must
/// open the row's world (the Work inspector pager), not just move focus.
#[test]
fn work_surface_rows_stay_clickable_under_a_goal_title() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".deepseek").join("config.toml"),
        "[retry]\nenabled = false\n\n[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n",
    )?;
    let session_path = ws.workspace().join("goal-click-session.json");
    let todos = (0..6)
        .map(|index| {
            serde_json::json!({
                "id": index + 1,
                "content": format!("todo-goal-{index:02}"),
                "status": if index == 0 { "in_progress" } else { "pending" }
            })
        })
        .collect::<Vec<_>>();
    std::fs::write(
        &session_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "metadata": {
                "id": "pty-goal-click",
                "title": "Goal-title click probe",
                "created_at": "2026-08-04T00:00:00Z",
                "updated_at": "2026-08-04T00:00:00Z",
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
        .size(40, 140)
        .spawn()?;
    enter_launch_session(&mut h)?;
    h.send(keys::key::text(&format!(
        "/load {}",
        session_path.to_string_lossy()
    )))?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("todo-goal-00", KEY_TIMEOUT)?;

    // Declare a goal: the strip now pins `Goal: …` above the rows, which is
    // the header-offset condition the bare click test never exercises. The
    // /goal command also fires a turn; the refused base URL fails it fast.
    h.send(keys::key::text("/goal click-probe objective"))?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Goal:", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(5))?;

    let target = "todo-goal-03";
    let (row, col) = h.frame().find_text(target).expect("rendered to-do row");
    h.send(keys::mouse::click(row, col))?;
    h.wait_for_text("q/Esc close", KEY_TIMEOUT)?;
    h.wait_for_text(target, KEY_TIMEOUT)?;

    let _ = h.shutdown();
    Ok(())
}

/// A loopback SSE fixture that streams one content chunk, then deliberately
/// holds the connection open before finishing, so a PTY scenario can interact
/// with the TUI while a turn is genuinely live (`is_loading == true`).
fn spawn_slow_stream_fixture(
    hold: Duration,
) -> anyhow::Result<(String, std::thread::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut served = 0usize;
        while served < 3 && Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut request = [0u8; 64 * 1024];
            let _ = stream.read(&mut request);
            let first = format!(
                "data: {}\n\n",
                serde_json::json!({
                    "id":"chatcmpl-slow",
                    "object":"chat.completion.chunk",
                    "model":"deepseek-v4-flash",
                    "choices":[{"index":0,"delta":{"content":"SLOW-STREAM-HOLD"},"finish_reason":null}]
                })
            );
            let rest = [
                format!(
                    "data: {}\n\n",
                    serde_json::json!({
                        "id":"chatcmpl-slow",
                        "object":"chat.completion.chunk",
                        "model":"deepseek-v4-flash",
                        "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14}
                    })
                ),
                "data: [DONE]\n\n".to_string(),
            ]
            .join("");
            // No Content-Length: the reader must see the first chunk while
            // the turn is still open, then the tail after the hold.
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{first}"
                )
                .as_bytes(),
            );
            let _ = stream.flush();
            if served == 0 {
                std::thread::sleep(hold);
            }
            let _ = stream.write_all(rest.as_bytes());
            let _ = stream.flush();
            served += 1;
        }
    });
    Ok((format!("http://{address}"), handle))
}

/// Second owner-repro probe: click a to-do row while a turn is actively
/// streaming. The bare and goal-title probes both pass idle; dogfood clicks
/// happen mid-run, so pin the live-turn path too.
#[test]
fn work_surface_rows_stay_clickable_during_a_live_turn() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".deepseek").join("config.toml"),
        "[retry]\nenabled = false\n\n[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n",
    )?;
    let session_path = ws.workspace().join("live-click-session.json");
    let todos = (0..6)
        .map(|index| {
            serde_json::json!({
                "id": index + 1,
                "content": format!("todo-live-{index:02}"),
                "status": if index == 0 { "in_progress" } else { "pending" }
            })
        })
        .collect::<Vec<_>>();
    std::fs::write(
        &session_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "metadata": {
                "id": "pty-live-click",
                "title": "Live-turn click probe",
                "created_at": "2026-08-04T00:00:00Z",
                "updated_at": "2026-08-04T00:00:00Z",
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

    let (base_url, server) = spawn_slow_stream_fixture(Duration::from_secs(6))?;
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
            "--yolo",
        ])
        .size(40, 140)
        .spawn()?;
    enter_launch_session(&mut h)?;
    h.send(keys::key::text(&format!(
        "/load {}",
        session_path.to_string_lossy()
    )))?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("todo-live-00", KEY_TIMEOUT)?;

    // Start a turn; the fixture streams one chunk then holds ~6s.
    h.send(keys::key::text("hold the stream open"))?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("SLOW-STREAM-HOLD", Duration::from_secs(10))?;

    // Mid-stream: the turn is live. Click a to-do row and require its world
    // to open, exactly as it must when idle.
    let target = "todo-live-03";
    let (row, col) = h.frame().find_text(target).expect("rendered to-do row");
    h.send(keys::mouse::click(row, col))?;
    h.wait_for_text("q/Esc close", KEY_TIMEOUT)?;

    let _ = h.shutdown();
    drop(server);
    Ok(())
}

/// Third owner-repro probe: a user whose settings select a non-Tasks rail
/// panel (the classic sidebar_focus migration lands many upgraders on
/// Pinned) must still be able to click a to-do row and have its world open.
/// Before the 2026-08-04 fix, non-Tasks panels wiped every hitbox — clicks
/// did nothing at all, which is exactly what the owner reported.
#[test]
fn pinned_panel_rows_stay_clickable_in_a_real_pty() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    std::fs::write(
        ws.home().join(".deepseek").join("config.toml"),
        "[retry]\nenabled = false\n\n[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n",
    )?;
    let settings_dir = ws.home().join(".codewhale");
    std::fs::create_dir_all(&settings_dir)?;
    std::fs::write(
        settings_dir.join("settings.toml"),
        "rail_panel = \"pinned\"\n",
    )?;

    let session_path = ws.workspace().join("pinned-click-session.json");
    let todos = (0..5)
        .map(|index| {
            serde_json::json!({
                "id": index + 1,
                "content": format!("todo-pinned-{index:02}"),
                "status": if index == 0 { "in_progress" } else { "pending" }
            })
        })
        .collect::<Vec<_>>();
    std::fs::write(
        &session_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "metadata": {
                "id": "pty-pinned-click",
                "title": "Pinned-panel click probe",
                "created_at": "2026-08-04T00:00:00Z",
                "updated_at": "2026-08-04T00:00:00Z",
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
        .size(40, 140)
        .spawn()?;
    enter_launch_session(&mut h)?;
    h.send(keys::key::text(&format!(
        "/load {}",
        session_path.to_string_lossy()
    )))?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("todo-pinned-00", KEY_TIMEOUT)?;

    let target = "todo-pinned-02";
    let (row, col) = h.frame().find_text(target).expect("rendered to-do row");
    h.send(keys::mouse::click(row, col))?;
    h.wait_for_text("q/Esc close", KEY_TIMEOUT)?;
    h.wait_for_text(target, KEY_TIMEOUT)?;

    let _ = h.shutdown();
    Ok(())
}

#[test]
fn real_coordination_details_use_typed_persisted_receipts_in_a_unix_pty() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    let state_dir = ws.workspace().join(".codewhale").join("state");
    std::fs::create_dir_all(&state_dir)?;
    std::fs::write(
        state_dir.join("subagents.v1.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "snapshot_sequence": 6,
            "agents": [],
            "workers": [],
            "coordination": {
                "schema_version": 1,
                "sequence": 6,
                "decisions": [
                    {
                        "decision_id": "decision-a",
                        "subject": "release shell",
                        "status": "accepted",
                        "owner": "worker-a",
                        "scope": ["path:crates/tui"],
                        "constraints": ["PRIVATE-TRANSCRIPT-MARKER"],
                        "evidence_handles": [],
                        "version": 2,
                        "sequence": 1
                    },
                    {
                        "decision_id": "decision-b",
                        "subject": "release shell",
                        "status": "superseded",
                        "owner": "worker-b",
                        "scope": ["path:crates/tui"],
                        "constraints": [],
                        "evidence_handles": [],
                        "version": 1,
                        "sequence": 2
                    }
                ],
                "write_claims": [{
                    "claim": {
                        "owner": "worker-a",
                        "roots": ["crates/tui"],
                        "exact_files": [],
                        "contracts": ["ui-contract"]
                    },
                    "sequence": 3,
                    "isolated_worktree": false
                }],
                "reconciliations": [{
                    "reconciliation_id": "reconcile-release-shell",
                    "subject": "release shell",
                    "owner": "release-owner",
                    "input_decisions": ["decision-a", "decision-b"],
                    "outcome": "candidate-a",
                    "evidence_handles": [],
                    "candidate_handles": ["branch:candidate-a", "branch:candidate-b"],
                    "retry_count": 1,
                    "retry_limit": 3,
                    "reviewer_evidence_handles": ["agent:reviewer"],
                    "verifier_evidence_handles": ["agent:verifier"],
                    "verification_outcome": "verified",
                    "sequence": 4
                }],
                "projections": [{
                    "child_id": "worker-a",
                    "decision_ids": ["decision-a"],
                    "projected_bytes": 128,
                    "deduplicated": 1,
                    "omitted": 0,
                    "sequence": 5
                }],
                "contentions": [{
                    "claimant": "worker-b",
                    "conflicting_owner": "worker-a",
                    "roots": ["crates/tui"],
                    "exact_files": ["Cargo.toml"],
                    "contracts": ["ui-contract"],
                    "disposition": "blocked_pending_isolation_or_serialization",
                    "sequence": 6
                }]
            }
        }))?,
    )?;

    let (ws, mut h) = spawn_minimal_with_env(ws, &[])?;
    h.wait_for_idle(Duration::from_millis(250), Duration::from_secs(3))?;
    let ambient = h.frame().debug_dump();
    assert!(
        !ambient.contains("Coordination Work") && !ambient.contains("PRIVATE-TRANSCRIPT-MARKER"),
        "coordination details leaked into ambient chrome:\n{ambient}"
    );
    let _ = h.shutdown();

    std::fs::write(
        ws.home().join(".codewhale").join("settings.toml"),
        "work_surface_placement = \"right\"\n",
    )?;
    let (_ws, mut h) = spawn_minimal_with_env(ws, &[])?;
    h.wait_for_text("Coordination Work", KEY_TIMEOUT)?;
    h.send(keys::key::alt('w'))?;
    h.wait_for_idle(Duration::from_millis(80), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    let required_details = [
        "decision-a · release shell",
        "status accepted · owner worker-a · version 2",
        "claimant worker-b · owner worker-a",
        "paths crates/tui, Cargo.toml",
        "contracts ui-contract",
        "disposition blocked_pending_isolation_or_serialization",
        "release shell · 2 candidates · retry 1/3",
        "reviewer agent:reviewer",
        "verifier agent:verifier",
        "verification verified",
        "worker-a · decisions decision-a · 128 bytes · 1 deduplicated · 0 omitted",
    ];
    for required in required_details {
        h.wait_for_text(required, KEY_TIMEOUT)?;
    }
    let wide = wait_for_frame_dump(
        &mut h,
        |frame| required_details.iter().all(|detail| frame.contains(detail)),
        KEY_TIMEOUT,
    )?;
    assert!(!wide.contains("PRIVATE-TRANSCRIPT-MARKER"), "{wide}");
    write_real_pty_evidence_dump(
        "coordination-details-wide-140x40",
        "size=140x40\nstate=persisted-coordination\nplacement=right\naction=Alt+W then Enter\nprivate_marker_rendered=false",
        &wide,
    )?;

    h.resize(18, 60)?;
    let narrow = wait_for_frame_dump(
        &mut h,
        |frame| {
            frame.rows() == 18
                && frame.cols() == 60
                && frame.contains("Coordination Work")
                && frame.contains("decision-a")
        },
        KEY_TIMEOUT,
    )?;
    assert!(!narrow.contains("PRIVATE-TRANSCRIPT-MARKER"), "{narrow}");
    assert!(narrow.contains("decision-a"), "{narrow}");
    write_real_pty_evidence_dump(
        "coordination-details-narrow-60x18",
        "size=60x18\nstate=persisted-coordination\naction=resize with pager open\nprivate_marker_rendered=false",
        &narrow,
    )?;

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

    let prompt = "Request the fixture File.write call; do not change its arguments.";
    // This is a whole prompt, not simulated human typing. Send it as the
    // bracketed paste a real terminal would emit so the raw-key paste-burst
    // heuristic cannot absorb the following Enter as a pasted newline.
    h.paste(prompt)?;
    h.wait_for_text(prompt, KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(100), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Allow once", Duration::from_secs(10))?;
    h.wait_for_text("Deny this call", KEY_TIMEOUT)?;

    let (deny_row, deny_col) = h
        .frame()
        .find_text("Deny this call")
        .expect("rendered denial option");
    h.send(keys::key::page_up())?;
    h.wait_for_text("❯ [3 / d / n]", KEY_TIMEOUT)?;
    h.send(keys::mouse::wheel_down(deny_row, deny_col))?;
    h.wait_for_text("❯ [3 / d / n]", KEY_TIMEOUT)?;
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
        "denied approval executed its File.write side effect: {}",
        denied_path.display()
    );

    let _ = h.shutdown();
    server.join().expect("approval fixture server thread");
    Ok(())
}

/// Release stopship coverage: a real built TUI restores durable To-do state and
/// keeps both active To-dos and the effective permission posture visible at each
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
            "allow_shell = true\nreasoning_effort = \"low\"\n\n[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n",
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
        h.wait_for_text("To-do ·", KEY_TIMEOUT)?;
        h.wait_for_text("Full Access", KEY_TIMEOUT)?;
        h.wait_for_idle(Duration::from_millis(250), Duration::from_secs(3))?;

        let frame = h.frame();
        let dump = frame.debug_dump();
        assert!(
            frame.contains("To-do · 1/3 · 2 left"),
            "numbered To-do progress missing at {cols}x{rows}:\n{dump}"
        );
        assert!(
            frame.contains("1 ·") && frame.contains("verify PTY"),
            "canonical current To-do missing at {cols}x{rows}:\n{dump}"
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
        let effort_receipt = if cols >= 60 { " · low " } else { " · l " };
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
        "allow_shell = true\nreasoning_effort = \"low\"\n\n[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n",
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
    h.wait_for_text("To-do ·", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(250), Duration::from_secs(3))?;
    assert!(
        h.frame().contains("To-do · 1/3 · 2 left")
            && h.frame().contains("1 ·")
            && h.frame().contains("verify integrated PTY"),
        "{}",
        h.frame().debug_dump()
    );

    h.send(b"\x14")?;
    h.wait_for_text("Reasoning effort: max", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    let cycled = h.frame();
    assert!(
        cycled.row(0).contains(" · max ") && cycled.row(0).contains("Full Access"),
        "Ctrl+T effort missing from narrow header:\n{}",
        cycled.debug_dump()
    );
    assert!(cycled.contains("To-do ·"), "{}", cycled.debug_dump());

    let before_path = ws.workspace().join("wg6-before-export.json");
    h.send(keys::key::text("/save wg6-before-export.json"))?;
    h.wait_for_text("/save", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Session saved to", KEY_TIMEOUT)?;

    let export_path = ws.workspace().join("wg6-export.md");
    h.send(keys::key::text("/export wg6-export.md"))?;
    h.wait_for_text("/export", KEY_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    h.wait_for_text("Conversation exported to", KEY_TIMEOUT)?;

    let after_path = ws.workspace().join("wg6-after-export.json");
    h.send(keys::key::text("/save wg6-after-export.json"))?;
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
    // The Ctrl+T selection above is the user's last explicit choice, so it is
    // the startup default the next launch must come up with — before any
    // session is loaded. Asserting it here separates the two mechanisms that
    // could otherwise both explain a `max` header after `/load`: a persisted
    // startup default, or session-restored state.
    assert!(
        restored.frame().row(0).contains(" · max "),
        "fresh launch lost the persisted effort selection:\n{}",
        restored.frame().debug_dump()
    );
    restored.send(keys::key::text(&format!(
        "/load {}",
        after_path.to_string_lossy()
    )))?;
    restored.wait_for_text("/load", KEY_TIMEOUT)?;
    restored.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    restored.send(keys::key::enter())?;
    restored.wait_for_text("To-do ·", KEY_TIMEOUT)?;
    restored.wait_for_idle(Duration::from_millis(250), Duration::from_secs(3))?;
    let frame = restored.frame();
    assert!(
        frame.contains("To-do · 1/3 · 2 left")
            && frame.contains("1 ·")
            && frame.contains("verify integrated PTY"),
        "{}",
        frame.debug_dump()
    );
    assert!(
        frame.row(0).contains(" · max ") && frame.row(0).contains("Full Access"),
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

/// Bare `/skills` opens the unified Skills Manager (owned-only, zero network).
/// Compatible roots (e.g. `.agents/skills`) appear only after toggling scan mode.
#[test]
#[ignore = "runs in its own CI process to avoid PTY event interference from the full suite"]
fn skills_opens_manager_owned_then_compatible() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let ws = make_sealed_workspace()?;
    // Compatible external root — hidden until the user presses `c`.
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
    h.wait_for_text("Skills Manager", KEY_TIMEOUT)?;
    h.wait_for_text("scan=owned   import-target=global   idle", KEY_TIMEOUT)?;

    let owned = h.frame();
    let owned_dump = owned.debug_dump();
    assert!(
        !owned.contains("workspace-beta"),
        "compatible skill must stay hidden in owned-only scan:\n{owned_dump}"
    );
    assert!(
        !owned.contains("Available skills"),
        "bare /skills must open manager, not the legacy text list:\n{owned_dump}"
    );
    assert!(
        !owned.contains("Fetching registry") && !owned.contains("registry.json"),
        "default manager must stay zero-network:\n{owned_dump}"
    );

    // Toggle to compatible scan so external roots appear. The mode change runs
    // a bounded filesystem audit synchronously; on cold Linux CI filesystems,
    // hashing the bundled skill tree can legitimately take longer than an
    // ordinary key response. Wait for the explicit mode receipt with the scan
    // budget, then use the ordinary interaction budget for the rendered row.
    h.send(keys::key::ch('c'))?;
    h.wait_for_text("scan=compatible", SKILL_SCAN_TIMEOUT)?;
    h.wait_for_text("workspace-beta", KEY_TIMEOUT)?;
    let compat = h.frame();
    let compat_dump = compat.debug_dump();
    assert!(
        compat.contains("workspace-beta"),
        "compatible skill missing after toggle:\n{compat_dump}"
    );

    h.send(keys::key::esc())?;
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(2))?;
    h.wait_for_text(COMPOSER_READY_TEXT, KEY_TIMEOUT)?;
    let after = h.frame();
    assert!(
        !after.contains("Skills Manager"),
        "Esc should close the skills manager:\n{}",
        after.debug_dump()
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

/// Markers the TUI paints once a turn has actually been dispatched. The PTY
/// scenarios point at `127.0.0.1:1`, so a submitted turn fails immediately and
/// leaves one of these on screen; an Enter that was swallowed by paste-burst
/// suppression leaves none of them.
fn frame_shows_dispatched_turn(frame: &qa_harness::Frame) -> bool {
    frame.contains("Turn failed") || frame.contains("Connection refused") || frame.contains("error")
}

/// Paste-burst Enter suppression must be *bounded*. After an unbracketed
/// paste flushes, the ~120ms window may absorb one Enter as the paste's
/// possible trailing newline — but absorbing it must not re-arm the window.
/// It used to, so every Enter bought another 120ms and a user pressing Enter
/// to send just watched newlines pile up in a composer that never submitted.
#[test]
fn paste_unbracketed_then_repeated_enter_still_submits() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal_without_retry()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(3))?;

    // No trailing newline: the user pastes a prompt and then presses Enter
    // themselves. This is the gesture the suppression window used to eat.
    h.paste_unbracketed("qa paste burst enter release payload")?;
    h.wait_for_text("qa paste burst enter release payload", KEY_TIMEOUT)?;
    let frame = h.frame();
    let dump = frame.debug_dump();
    assert!(
        !frame_shows_dispatched_turn(frame),
        "nothing may be dispatched before Enter is pressed:\n{dump}"
    );

    // Press Enter repeatedly at a human "it didn't send?" cadence. The gaps
    // are shorter than the 120ms window, so the old re-arming behaviour kept
    // suppression alive indefinitely and none of these ever submitted.
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(60));
        h.send(keys::key::enter())?;
    }

    h.wait_for(frame_shows_dispatched_turn, Duration::from_secs(15))
        .map_err(|error| {
            anyhow::anyhow!("pasted prompt never submitted despite repeated Enter: {error:#}")
        })?;

    let _ = h.shutdown();
    Ok(())
}

/// IME Enter ambiguity: a CJK message typed one candidate commit at a time is
/// ordinary typing, not a paste. Each commit used to re-arm the full ~120ms
/// Enter-suppression window, so the Enter that follows a Chinese sentence was
/// swallowed into a newline and the message never sent.
#[test]
fn ime_committed_cjk_then_enter_submits() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let (_ws, mut h) = boot_minimal_without_retry()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    h.wait_for_idle(Duration::from_millis(300), Duration::from_secs(3))?;

    // An IME delivers each committed character as its own key event, with
    // human-scale gaps between them — far slower than the 8ms burst interval.
    for ch in "你好世".chars() {
        h.send(keys::key::ch(ch))?;
        std::thread::sleep(Duration::from_millis(60));
    }
    // Sync on the echo so the child is provably caught up before the last
    // commit — the gap that follows must be measured from when the TUI
    // *processes* that character, not from when we wrote it.
    h.wait_for_text("你好世", KEY_TIMEOUT)?;
    let frame = h.frame();
    let dump = frame.debug_dump();
    assert!(
        !frame_shows_dispatched_turn(frame),
        "nothing may be dispatched before Enter is pressed:\n{dump}"
    );

    // Final commit, then one Enter at a realistic remove from it: far beyond
    // the 8ms burst interval, comfortably inside the old 120ms window.
    h.send(keys::key::ch('界'))?;
    std::thread::sleep(Duration::from_millis(50));
    h.send(keys::key::enter())?;

    h.wait_for(frame_shows_dispatched_turn, Duration::from_secs(15))
        .map_err(|error| {
            anyhow::anyhow!("IME-typed CJK message never submitted on Enter: {error:#}")
        })?;

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

fn read_http_request(stream: &mut std::net::TcpStream) -> anyhow::Result<String> {
    // Reqwest may establish the next loopback connection before the real PTY
    // test releases a blocked tool. Keep that idle socket bounded, but long
    // enough for the deliberately observed reasoning/read phases to finish.
    // Darwin can propagate O_NONBLOCK from the listener to accepted sockets;
    // restore blocking reads before applying the timeout or an early accept
    // races the first request byte and reports EAGAIN as a network failure.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut request = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);

        let Some(header_end) = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
        else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        });
        if content_length.is_none_or(|length| request.len() >= header_end + length) {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&request).into_owned())
}

fn pty_tool_call_sse(id: &str, name: &str, arguments: serde_json::Value) -> String {
    [
        format!(
            "data: {}\n\n",
            serde_json::json!({
                "id": format!("chatcmpl-{id}"),
                "object": "chat.completion.chunk",
                "model": "deepseek-v4-pro",
                "choices": [{
                    "index": 0,
                    "delta": {"tool_calls": [{
                        "index": 0,
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": serde_json::to_string(&arguments)
                                .expect("tool arguments JSON")
                        }
                    }]},
                    "finish_reason": null
                }]
            })
        ),
        format!(
            "data: {}\n\n",
            serde_json::json!({
                "id": format!("chatcmpl-{id}"),
                "object": "chat.completion.chunk",
                "model": "deepseek-v4-pro",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                "usage": {"prompt_tokens": 12, "completion_tokens": 4, "total_tokens": 16}
            })
        ),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

fn pty_text_sse(content: &str) -> String {
    [
        format!(
            "data: {}\n\n",
            serde_json::json!({
                "id": "chatcmpl-pty-lifecycle-final",
                "object": "chat.completion.chunk",
                "model": "deepseek-v4-pro",
                "choices": [{
                    "index": 0,
                    "delta": {"content": content},
                    "finish_reason": null
                }]
            })
        ),
        format!(
            "data: {}\n\n",
            serde_json::json!({
                "id": "chatcmpl-pty-lifecycle-final",
                "object": "chat.completion.chunk",
                "model": "deepseek-v4-pro",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 24, "completion_tokens": 80, "total_tokens": 104}
            })
        ),
        "data: [DONE]\n\n".to_string(),
    ]
    .join("")
}

/// Sealed loopback fixture for the #4636 File-mutation receipt. One canonical
/// `File.patch` call performs an update, create, delete, and byte-identical
/// delete/create rename in a single transaction; the second response settles
/// the turn. No provider or external network is involved.
fn spawn_file_mutation_screen_fixture(
    tool_allowed: bool,
) -> anyhow::Result<(String, std::thread::JoinHandle<anyhow::Result<()>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let patch = r"diff --git a/old-name.txt b/old-name.txt
--- a/old-name.txt
+++ /dev/null
@@ -1 +0,0 @@
-RENAME-SENTINEL
diff --git a/new-name.txt b/new-name.txt
--- /dev/null
+++ b/new-name.txt
@@ -0,0 +1 @@
+RENAME-SENTINEL
diff --git a/update.txt b/update.txt
--- a/update.txt
+++ b/update.txt
@@ -1 +1 @@
-DIFF-OLD-SENTINEL
+DIFF-NEW-SENTINEL
diff --git a/create.txt b/create.txt
--- /dev/null
+++ b/create.txt
@@ -0,0 +1 @@
+CREATE-SENTINEL
diff --git a/delete.txt b/delete.txt
--- a/delete.txt
+++ /dev/null
@@ -1 +0,0 @@
-DELETE-SENTINEL
";
    let replies = [
        pty_tool_call_sse(
            "call_file_mutation_pty",
            "File",
            serde_json::json!({"action": "patch", "patch": patch}),
        ),
        pty_text_sse("FILE-MUTATION-FIXTURE-DONE"),
    ];
    let expected_result_marker = if tool_allowed {
        "files_applied"
    } else {
        "destructive action requires explicit review"
    };

    let handle = std::thread::spawn(move || -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(45);
        let mut chat_index = 0_usize;
        let mut contract_errors = Vec::new();
        while chat_index < replies.len() && Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let request = read_http_request(&mut stream)?;
            let request_line = request.lines().next().unwrap_or_default();
            let (content_type, body) = if request_line.starts_with("GET ")
                && request_line.contains("/models")
            {
                (
                    "application/json",
                    serde_json::json!({
                        "object": "list",
                        "data": [{"id": "deepseek-v4-pro", "object": "model"}]
                    })
                    .to_string(),
                )
            } else if request_line.starts_with("POST ")
                && request_line.contains("/chat/completions")
            {
                let request_body = request
                    .split_once("\r\n\r\n")
                    .map(|(_, body)| body)
                    .unwrap_or_default();
                let request_json: serde_json::Value = serde_json::from_str(request_body)?;
                let request_contract = request_json.to_string();
                match chat_index {
                    0 if !request_contract
                        .contains("exercise the canonical File mutation receipt") =>
                    {
                        contract_errors.push("initial request omitted the fixture prompt".into());
                    }
                    1 if !(request_contract.contains("call_file_mutation_pty")
                        && request_contract.contains(expected_result_marker)
                        && request_contract.contains("\"role\":\"tool\"")) =>
                    {
                        let sample = request_contract.chars().take(1_200).collect::<String>();
                        contract_errors.push(format!(
                            "settling request omitted the expected File result: {sample}"
                        ));
                    }
                    0 | 1 => {}
                    _ => unreachable!("bounded File fixture"),
                }
                let body = replies[chat_index].clone();
                chat_index += 1;
                ("text/event-stream", body)
            } else {
                ("text/plain", "not found".to_string())
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes())?;
            stream.flush()?;
        }
        anyhow::ensure!(
            chat_index == replies.len(),
            "File fixture served {chat_index}/{} chat requests",
            replies.len()
        );
        if !contract_errors.is_empty() {
            anyhow::bail!(
                "File fixture contract errors:\n{}",
                contract_errors.join("\n")
            );
        }
        Ok(())
    });
    Ok((format!("http://{address}"), handle))
}

fn spawn_file_mutation_harness(
    ws: &qa_harness::harness::SealedWorkspace,
    base_url: &str,
    rows: u16,
    cols: u16,
    ascii_safe: bool,
) -> anyhow::Result<Harness> {
    let codewhale_home = ws.home().join(".codewhale");
    let codex_home = ws.home().join(".codex");
    let mut builder = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env("CODEWHALE_HOME", codewhale_home.to_string_lossy())
        .env(
            "DEEPSEEK_CONFIG_PATH",
            codewhale_home.join("config.toml").to_string_lossy(),
        )
        .env("CODEX_HOME", codex_home.to_string_lossy())
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", base_url)
        .env("CODEWHALE_BASE_URL", base_url)
        .env("DEEPSEEK_MODEL", "deepseek-v4-pro")
        .env("CODEWHALE_MODEL", "deepseek-v4-pro")
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
            "--mouse-capture",
        ])
        .size(rows, cols);
    if ascii_safe {
        builder = builder.env("CODEWHALE_ASCII_SAFE", "1");
    }
    builder.spawn()
}

/// #4636: real terminal frames for the persisted full/summary/off contract.
/// The three cases jointly cover Ask/Auto/Full Access, narrow/wide, dark/light,
/// reduced-motion, and ASCII-safe operation. The off case is changed through
/// `/config --save` and then rebooted before execution, proving the setting
/// survives restart.
#[test]
fn work_surface_file_mutation_modes_are_truthful_in_real_pty_frames() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    let cases = [
        (
            "full", 140_u16, 40_u16, "dark", false, false, "ask", "ask", true,
        ),
        (
            "summary", 100, 32, "light", false, false, "auto", "auto", false,
        ),
        (
            "off",
            80,
            24,
            "dark",
            true,
            true,
            "full-access",
            "Full Access",
            false,
        ),
    ];

    for (
        mode,
        cols,
        rows,
        theme,
        ascii_safe,
        persist_through_restart,
        permission_posture,
        permission_label,
        approve_once,
    ) in cases
    {
        let ws = make_sealed_workspace()?;
        let codewhale_home = ws.home().join(".codewhale");
        let codex_home = ws.home().join(".codex");
        std::fs::create_dir_all(&codex_home)?;
        std::fs::write(
            codewhale_home.join("config.toml"),
            "reasoning_effort = \"low\"\n\n[retry]\nenabled = false\n\n[update]\ncheck_for_updates = false\n\n[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n",
        )?;
        let initial_mode = if persist_through_restart {
            "full"
        } else {
            mode
        };
        std::fs::write(
            codewhale_home.join("settings.toml"),
            format!(
                "theme = \"{theme}\"\nlocale = \"en\"\ndefault_mode = \"agent\"\npermission_posture = \"{permission_posture}\"\ninline_diffs = \"{initial_mode}\"\nlow_motion = true\nfancy_animations = false\ncomposer_border = true\n"
            ),
        )?;
        std::fs::write(
            codex_home.join("models_cache.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "fetched_at": chrono::Utc::now(),
                "models": [{"slug": "deepseek-v4-pro", "priority": 1}]
            }))?,
        )?;
        std::fs::write(ws.workspace().join("old-name.txt"), "RENAME-SENTINEL\n")?;
        std::fs::write(ws.workspace().join("update.txt"), "DIFF-OLD-SENTINEL\n")?;
        std::fs::write(ws.workspace().join("delete.txt"), "DELETE-SENTINEL\n")?;

        if persist_through_restart {
            let mut setup =
                spawn_file_mutation_harness(&ws, "http://127.0.0.1:1", rows, cols, ascii_safe)?;
            enter_launch_session(&mut setup)?;
            setup.paste("/config inline_diffs off --save")?;
            setup.wait_for_text("/config inline_diffs off --save", KEY_TIMEOUT)?;
            setup.send(keys::key::enter())?;
            setup.wait_for_text("inline_diffs = off (saved)", KEY_TIMEOUT)?;
            let _ = setup.shutdown();
            let persisted = std::fs::read_to_string(codewhale_home.join("settings.toml"))?;
            anyhow::ensure!(
                persisted.contains("inline_diffs = \"off\""),
                "off mode did not persist before restart: {persisted}"
            );
        }

        // Auto-Review deliberately has no approval escape hatch for destructive
        // create/delete work; Ask and Full Access can complete the transaction.
        let tool_allowed = permission_posture != "auto";
        let (base_url, server) = spawn_file_mutation_screen_fixture(tool_allowed)?;
        let mut h = spawn_file_mutation_harness(&ws, &base_url, rows, cols, ascii_safe)?;
        enter_launch_session(&mut h)?;
        assert_real_pty_frame_geometry(h.frame(), cols, rows);
        assert_control_grammar(h.frame(), "act", permission_label, COMPOSER_READY_TEXT);

        let prompt = "exercise the canonical File mutation receipt";
        h.paste(prompt)?;
        h.wait_for_text(prompt, KEY_TIMEOUT)?;
        h.send(keys::key::enter())?;
        if approve_once {
            h.wait_for_text("Allow once", Duration::from_secs(10))?;
            h.send(b"y")?;
        }
        h.wait_for_text("FILE-MUTATION-FIXTURE-DONE", Duration::from_secs(20))?;
        if tool_allowed {
            h.wait_for(
                |frame| frame.contains("4 files") && frame.contains("done"),
                Duration::from_secs(10),
            )?;
        } else {
            h.wait_for(
                |frame| {
                    frame.contains("tool issue")
                        && frame.contains("destructive action")
                        && frame.contains("done")
                },
                Duration::from_secs(10),
            )?;
        }
        h.wait_for_idle(Duration::from_millis(250), Duration::from_secs(3))?;

        if tool_allowed {
            assert!(
                !h.frame().contains("Wrote 4 files"),
                "completed file-operation summary leaked into ambient chrome:\n{}",
                h.frame().debug_dump()
            );
            assert_eq!(
                std::fs::read_to_string(ws.workspace().join("new-name.txt"))?,
                "RENAME-SENTINEL\n"
            );
            assert!(!ws.workspace().join("old-name.txt").exists());
            assert_eq!(
                std::fs::read_to_string(ws.workspace().join("update.txt"))?,
                "DIFF-NEW-SENTINEL\n"
            );
            assert_eq!(
                std::fs::read_to_string(ws.workspace().join("create.txt"))?,
                "CREATE-SENTINEL\n"
            );
            assert!(!ws.workspace().join("delete.txt").exists());
        } else {
            assert_eq!(
                std::fs::read_to_string(ws.workspace().join("old-name.txt"))?,
                "RENAME-SENTINEL\n"
            );
            assert_eq!(
                std::fs::read_to_string(ws.workspace().join("update.txt"))?,
                "DIFF-OLD-SENTINEL\n"
            );
            assert!(!ws.workspace().join("new-name.txt").exists());
            assert!(!ws.workspace().join("create.txt").exists());
            assert_eq!(
                std::fs::read_to_string(ws.workspace().join("delete.txt"))?,
                "DELETE-SENTINEL\n"
            );
        }

        let settled_frame = h.frame().text();
        std::thread::sleep(Duration::from_millis(300));
        h.pump();
        assert_eq!(
            settled_frame,
            h.frame().text(),
            "reduced-motion settled frame moved in {mode} mode"
        );
        assert_real_pty_frame_geometry(h.frame(), cols, rows);
        if ascii_safe {
            assert!(
                h.frame().text().is_ascii(),
                "ASCII-safe mutation frame emitted non-ASCII cells:\n{}",
                h.frame().debug_dump()
            );
        }

        match mode {
            "full" => {
                assert!(
                    scroll_until(&mut h, ScrollDir::Up, "DIFF-NEW-SENTINEL"),
                    "full mode omitted the added line:\n{}",
                    h.frame().debug_dump()
                );
                assert!(h.frame().contains("DIFF-OLD-SENTINEL"));
                let old_row = visible_row_with_text(h.frame(), "DIFF-OLD-SENTINEL")
                    .expect("deleted line row");
                let new_row =
                    visible_row_with_text(h.frame(), "DIFF-NEW-SENTINEL").expect("added line row");
                let old_color = foreground_at_text(h.frame(), old_row, "DIFF-OLD-SENTINEL");
                let new_color = foreground_at_text(h.frame(), new_row, "DIFF-NEW-SENTINEL");
                assert_ne!(old_color, qa_harness::Color::Default);
                assert_ne!(new_color, qa_harness::Color::Default);
                assert_ne!(old_color, new_color, "added/deleted ANSI roles collapsed");
            }
            "summary" => {
                assert!(
                    scroll_until(&mut h, ScrollDir::Up, "+3 / -3"),
                    "held Auto-Review mutation omitted semantic stats:\n{}",
                    h.frame().debug_dump()
                );
                assert!(h.frame().contains("explicit review"));
                assert!(!scroll_until(&mut h, ScrollDir::Up, "DIFF-NEW-SENTINEL"));
                assert!(!scroll_until(&mut h, ScrollDir::Down, "DIFF-NEW-SENTINEL"));
            }
            "off" => {
                assert!(
                    scroll_until(&mut h, ScrollDir::Up, "4 files"),
                    "off mode lost the concise File outcome:\n{}",
                    h.frame().debug_dump()
                );
                assert!(!scroll_until(&mut h, ScrollDir::Up, "+2 -2"));
                assert!(!scroll_until(&mut h, ScrollDir::Down, "+2 -2"));
                assert!(!scroll_until(&mut h, ScrollDir::Up, "DIFF-NEW-SENTINEL"));
                assert!(!scroll_until(&mut h, ScrollDir::Down, "DIFF-NEW-SENTINEL"));

                h.send(keys::key::alt('v'))?;
                h.wait_for_text("Raw detail", KEY_TIMEOUT)?;
                assert!(
                    scroll_until(&mut h, ScrollDir::Down, "Exact File change"),
                    "off mode lost the exact-evidence section:\n{}",
                    h.frame().debug_dump()
                );
                assert!(
                    scroll_until(&mut h, ScrollDir::Down, "DIFF-NEW-SENTINEL"),
                    "off mode exact evidence omitted the applied diff:\n{}",
                    h.frame().debug_dump()
                );
            }
            _ => unreachable!("bounded diff mode matrix"),
        }

        write_real_pty_evidence(
            &format!("file-mutation-{mode}-{cols}x{rows}"),
            &format!(
                "size={cols}x{rows}\ntheme={theme}\ninline_diffs={mode}\npermission={permission_posture}\nreduced_motion=true\nascii_safe={ascii_safe}\nprovider=sealed-loopback"
            ),
            h.frame(),
        )?;
        let _ = h.shutdown();
        server.join().expect("File fixture server thread")?;
    }
    Ok(())
}

/// Three-turn loopback fixture for the real screen acceptance path:
/// `work_update` establishes canonical To-do/Work state, then an actual Bash
/// call waits on a test-owned workspace sentinel before emitting enough exact
/// output to exercise the bounded plain preview, then a long final answer makes
/// transcript retention and scrolling observable.
fn spawn_tool_lifecycle_screen_fixture(
    release_signal: &str,
    final_answer: String,
) -> anyhow::Result<(String, std::thread::JoinHandle<anyhow::Result<()>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    // The Bash adapter self-bounds each stream to ~30 KB, so a single stream
    // would fit inside the hybrid 32 KiB + 8 KiB preview budget under the
    // 32_768-token evidence threshold. Fill stdout AND stderr (~60 KB
    // combined) so the envelope still omits a middle range; the sentinel
    // rides stderr at filler line 50 — inside the shell tool's own head bound
    // (so the artifact retains it) but beyond the preview's 32 KiB head (so
    // the model receipt omits it).
    //
    // The window is narrow and #5212 put the sentinel outside it: it wrote
    // line 100 against a "22 KB head bound" that does not exist.
    // `shell_output` keeps `TRUNCATED_HEAD_BYTES` = 30_000/5 = 6_000 bytes of
    // head plus 24_000 of tail, so line 100 (~8.7 KB into stderr) fell in the
    // stream's own omitted middle and the artifact never carried the
    // sentinel. Above the head bound sits ~line 68; below it, the bounded
    // stdout section (~30.1 KB) plus the `STDERR:` separator puts stderr line
    // *n* at roughly 30_115 + 87n bytes, so anything before ~line 31 is still
    // inside the preview's 32 KiB head. Line 50 is the middle of [31, 68].
    // `tests/adaptive_evidence_acceptance.rs` carries the same constant for
    // the same reason.
    let shell_command = format!(
        "printf 'PTY-TOOL-START\\n'; while [ ! -f {release_signal} ]; do sleep 0.05; done; i=0; while [ \"$i\" -lt 2800 ]; do printf 'PTY-EVIDENCE-%04d-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n' \"$i\"; i=$((i + 1)); done; {{ j=0; while [ \"$j\" -lt 2800 ]; do if [ \"$j\" -eq 50 ]; then printf 'PTY-EVIDENCE-DEEP-SENTINEL\\n'; fi; printf 'PTY-EVIDENCE-ERR-%04d-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n' \"$j\"; j=$((j + 1)); done; }} >&2; printf 'PTY-TOOL-END\\n'"
    );
    let replies = [
        pty_tool_call_sse(
            "call_work_pty",
            "work_update",
            serde_json::json!({
                "todos": [{
                    "content": "PTY lifecycle acceptance",
                    "status": "in_progress"
                }]
            }),
        ),
        pty_tool_call_sse(
            "call_bash_pty",
            "Bash",
            serde_json::json!({
                "action": "run",
                "command": shell_command,
                "timeout_ms": 60_000
            }),
        ),
        pty_text_sse(&final_answer),
    ];

    let handle = std::thread::spawn(move || -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(75);
        let mut chat_index = 0_usize;
        let mut contract_errors = Vec::new();
        let mut connection_errors = Vec::new();
        while chat_index < replies.len() && Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let request = match read_http_request(&mut stream) {
                Ok(request) if !request.trim().is_empty() => request,
                Ok(_) => continue,
                Err(error) => {
                    connection_errors.push(format!("request read failed: {error:#}"));
                    continue;
                }
            };
            let request_line = request.lines().next().unwrap_or_default();
            let mut is_chat_response = false;
            let (content_type, body) = if request_line.starts_with("GET ")
                && request_line.contains("/models")
            {
                (
                    "application/json",
                    serde_json::json!({
                        "object": "list",
                        "data": [{"id": "deepseek-v4-pro", "object": "model"}]
                    })
                    .to_string(),
                )
            } else if request_line.starts_with("POST ")
                && request_line.contains("/chat/completions")
            {
                let request_body = request
                    .split_once("\r\n\r\n")
                    .map(|(_, body)| body)
                    .unwrap_or_default();
                let request_json: serde_json::Value = match serde_json::from_str(request_body) {
                    Ok(request_json) => request_json,
                    Err(error) => {
                        contract_errors.push(format!(
                            "chat request JSON parse failed for {request_line}: {error}"
                        ));
                        let body = "invalid JSON".to_string();
                        let response = format!(
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.flush();
                        continue;
                    }
                };
                let request_contract = request_json.to_string();
                match chat_index {
                    0 if !request_contract.contains("exercise the real PTY tool lifecycle") => {
                        contract_errors.push("initial request omitted the user prompt".into());
                    }
                    1 if !(request_contract.contains("call_work_pty")
                        && request_contract.contains("\"role\":\"tool\"")) =>
                    {
                        contract_errors
                            .push("second request omitted the work_update result".into());
                    }
                    2 => {
                        let bash_result = request_json
                            .get("messages")
                            .and_then(serde_json::Value::as_array)
                            .and_then(|messages| {
                                messages.iter().find(|message| {
                                    message.get("role").and_then(serde_json::Value::as_str)
                                        == Some("tool")
                                        && message
                                            .get("tool_call_id")
                                            .and_then(serde_json::Value::as_str)
                                            == Some("call_bash_pty")
                                })
                            })
                            .and_then(|message| message.get("content"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        // Honest bounded-preview contract: the footer states
                        // the omission, names the on-disk artifact, and names
                        // `retrieve_tool_result` — the route the model can
                        // actually take from this receipt to read the omitted
                        // range back. The deep sentinel stays out of the
                        // inline receipt. This used to assert the *absence*
                        // of `retrieve_tool_result`, a leftover from #5018's
                        // "no storage language" pass;
                        // `tests/adaptive_evidence_acceptance.rs` proves end
                        // to end that the named ref returns the omitted bytes.
                        if !bash_result.contains("of output omitted")
                            || !bash_result.contains("full output at")
                            || !bash_result.contains("art_call_bash_pty.txt")
                            || bash_result.contains("Exact evidence retained")
                            || !bash_result.contains("retrieve_tool_result")
                            || bash_result.contains("PTY-EVIDENCE-DEEP-SENTINEL")
                        {
                            contract_errors.push(format!(
                                    "final request violated the honest bounded Bash preview contract (omission={}, path_footer={}, artifact_path={}, legacy_receipt={}, retrieval_tool={}, deep_sentinel={})",
                                    bash_result.contains("of output omitted"),
                                    bash_result.contains("full output at"),
                                    bash_result.contains("art_call_bash_pty.txt"),
                                    bash_result.contains("Exact evidence retained"),
                                    bash_result.contains("retrieve_tool_result"),
                                    bash_result.contains("PTY-EVIDENCE-DEEP-SENTINEL"),
                                ));
                        }
                    }
                    0 | 1 => {}
                    _ => unreachable!("bounded lifecycle fixture"),
                }
                let body = replies[chat_index].clone();
                is_chat_response = true;
                ("text/event-stream", body)
            } else {
                ("text/plain", "not found".to_string())
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            if let Err(error) = stream
                .write_all(response.as_bytes())
                .and_then(|()| stream.flush())
            {
                connection_errors
                    .push(format!("response write failed for {request_line}: {error}"));
                continue;
            }
            if is_chat_response {
                chat_index += 1;
            }
        }
        anyhow::ensure!(
            chat_index == replies.len(),
            "fixture served {chat_index}/{} chat requests; ignored connection errors: {}",
            replies.len(),
            connection_errors.join(" | ")
        );
        if !contract_errors.is_empty() {
            anyhow::bail!(
                "tool lifecycle fixture contract errors:\n{}",
                contract_errors.join("\n")
            );
        }
        Ok(())
    });
    Ok((format!("http://{address}"), handle))
}

/// Stream an explicit private reasoning delta, hold it until the PTY has
/// captured the semantic `reasoning` phase, then issue a canonical File.read.
/// Follow-up turns issue a real Bash wait and a final receipt. Every pause is
/// controlled by a test-owned workspace primitive; no product frame is
/// generated or reconstructed outside the real terminal parser.
fn spawn_semantic_activity_motion_fixture(
    reasoning_release: std::path::PathBuf,
    fifo_name: &str,
    bash_release_name: &str,
) -> anyhow::Result<(String, std::thread::JoinHandle<anyhow::Result<()>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let reasoning_prefix = format!(
        "data: {}\n\n",
        serde_json::json!({
            "id": "chatcmpl-semantic-reasoning",
            "object": "chat.completion.chunk",
            "model": "deepseek-v4-pro",
            "choices": [{
                "index": 0,
                "delta": {
                    "reasoning_content": "PRIVATE-MOTION-TRACE-MUST-STAY-HIDDEN"
                },
                "finish_reason": null
            }]
        })
    );
    let read_tail = pty_tool_call_sse(
        "call_read_motion",
        "File",
        // Pin the read to the streaming path. The small-file fast path opens
        // a FIFO once for metadata and then opens it again for content, which
        // makes a real PTY fixture depend on a race-prone third reader. An
        // explicit range keeps the content read on the already-open file.
        serde_json::json!({"action": "read", "path": fifo_name, "start_line": 1}),
    );
    let shell_command = format!(
        "printf 'MOTION-BASH-START\\n'; while [ ! -f {bash_release_name} ]; do sleep 0.05; done; printf 'MOTION-BASH-END\\n'"
    );
    let bash_reply = pty_tool_call_sse(
        "call_bash_motion",
        "Bash",
        serde_json::json!({
            "action": "run",
            "command": shell_command,
            "timeout_ms": 60_000
        }),
    );
    let final_reply = pty_text_sse("SEMANTIC-MOTION-DONE");

    let handle = std::thread::spawn(move || -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut chat_index = 0_usize;
        let mut contract_errors = Vec::new();
        let mut connection_errors = Vec::new();
        while chat_index < 3 && Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let request = match read_http_request(&mut stream) {
                Ok(request) if !request.trim().is_empty() => request,
                Ok(_) => continue,
                Err(error) => {
                    connection_errors.push(format!("request read failed: {error:#}"));
                    continue;
                }
            };
            let request_line = request.lines().next().unwrap_or_default();
            if request_line.starts_with("GET ") && request_line.contains("/models") {
                let body = serde_json::json!({
                    "object": "list",
                    "data": [{"id": "deepseek-v4-pro", "object": "model"}]
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                if let Err(error) = stream
                    .write_all(response.as_bytes())
                    .and_then(|()| stream.flush())
                {
                    connection_errors.push(format!(
                        "model response write failed for {request_line}: {error}"
                    ));
                }
                continue;
            }
            if !(request_line.starts_with("POST ") && request_line.contains("/chat/completions")) {
                contract_errors.push(format!(
                    "unexpected semantic activity fixture request: {request_line}"
                ));
                let body = "not found";
                let response = format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                continue;
            }

            let request_body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .unwrap_or_default();
            let request_json: serde_json::Value = match serde_json::from_str(request_body) {
                Ok(request_json) => request_json,
                Err(error) => {
                    contract_errors.push(format!(
                        "chat request JSON parse failed for {request_line}: {error}"
                    ));
                    let body = "invalid JSON";
                    let response = format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                    continue;
                }
            };
            let request_contract = request_json.to_string();
            match chat_index {
                0 if !request_contract.contains("show semantic activity with less chrome") => {
                    contract_errors.push("initial request omitted semantic activity prompt".into());
                }
                1 if !(request_contract.contains("call_read_motion")
                    && request_contract.contains("MOTION-FIFO-CONTENT")
                    && request_contract.contains("\"role\":\"tool\"")) =>
                {
                    contract_errors.push(format!(
                        "Bash request omitted completed File.read evidence (call={}, content={}, tool_role={})",
                        request_contract.contains("call_read_motion"),
                        request_contract.contains("MOTION-FIFO-CONTENT"),
                        request_contract.contains("\"role\":\"tool\""),
                    ));
                }
                2 if !(request_contract.contains("call_bash_motion")
                    && request_contract.contains("MOTION-BASH-END")) =>
                {
                    contract_errors.push(format!(
                        "final request omitted completed Bash evidence (call={}, completion={})",
                        request_contract.contains("call_bash_motion"),
                        request_contract.contains("MOTION-BASH-END"),
                    ));
                }
                0..=2 => {}
                _ => unreachable!("bounded semantic activity fixture"),
            }

            if chat_index == 0 {
                let body_len = reasoning_prefix.len() + read_tail.len();
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {body_len}\r\nConnection: close\r\n\r\n"
                );
                if let Err(error) = stream
                    .write_all(headers.as_bytes())
                    .and_then(|()| stream.write_all(reasoning_prefix.as_bytes()))
                    .and_then(|()| stream.flush())
                {
                    connection_errors.push(format!(
                        "reasoning prefix write failed for {request_line}: {error}"
                    ));
                    continue;
                }

                let release_deadline = Instant::now() + Duration::from_secs(30);
                while !reasoning_release.exists() && Instant::now() < release_deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                anyhow::ensure!(
                    reasoning_release.exists(),
                    "reasoning phase was never released by the PTY test"
                );
                if let Err(error) = stream
                    .write_all(read_tail.as_bytes())
                    .and_then(|()| stream.flush())
                {
                    connection_errors.push(format!(
                        "File.read tail write failed for {request_line}: {error}"
                    ));
                    continue;
                }
            } else {
                let body = if chat_index == 1 {
                    &bash_reply
                } else {
                    &final_reply
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                if let Err(error) = stream
                    .write_all(response.as_bytes())
                    .and_then(|()| stream.flush())
                {
                    connection_errors.push(format!(
                        "chat response write failed for {request_line}: {error}"
                    ));
                    continue;
                }
            }
            chat_index += 1;
        }
        anyhow::ensure!(
            chat_index == 3,
            "semantic activity fixture served {chat_index}/3 chat requests; ignored connection errors: {}",
            connection_errors.join(" | ")
        );
        if !contract_errors.is_empty() {
            anyhow::bail!(
                "semantic activity fixture contract errors:\n{}",
                contract_errors.join("\n")
            );
        }
        Ok(())
    });
    Ok((format!("http://{address}"), handle))
}

fn open_semantic_fifo_writer(
    path: &std::path::Path,
    deadline: Instant,
) -> anyhow::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
        {
            Ok(writer) => return Ok(writer),
            Err(error)
                if matches!(error.raw_os_error(), Some(libc::ENXIO) | Some(libc::EINTR))
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                anyhow::bail!("timed out waiting for FIFO reader at {}", path.display());
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn release_semantic_read_fifo(
    path: std::path::PathBuf,
) -> std::thread::JoinHandle<anyhow::Result<()>> {
    std::thread::spawn(move || -> anyhow::Result<()> {
        // ReadFileTool opens once to sniff PDF magic, then keeps the main
        // explicit-range read on one already-open descriptor. Move the FIFO
        // after the sniff writer and recreate it at the requested path so the
        // content writer cannot attach to the short-lived sniff reader. The
        // nonblocking, deadline-bounded opens turn product exit into a useful
        // test error instead of an unbounded join.
        let sniff_path = path.with_extension("sniff");
        let mut sniff_writer =
            open_semantic_fifo_writer(&path, Instant::now() + Duration::from_secs(20))?;
        std::fs::rename(&path, &sniff_path)?;
        let mkfifo = Command::new("mkfifo").arg(&path).status()?;
        anyhow::ensure!(
            mkfifo.success(),
            "mkfifo failed while rotating {}",
            path.display()
        );
        sniff_writer.write_all(b"TEXT")?;
        sniff_writer.flush()?;
        drop(sniff_writer);

        let mut content_writer =
            open_semantic_fifo_writer(&path, Instant::now() + Duration::from_secs(20))?;
        content_writer.write_all(b"MOTION-FIFO-CONTENT\n")?;
        content_writer.flush()?;
        Ok(())
    })
}

fn whale_ansi_signature(frame: &qa_harness::Frame) -> Vec<qa_harness::Color> {
    const WHALE_BACK: &str = "▗▄▄▄▄▄▄▄▄▄▄▄▖";
    let (row, mut col) = frame
        .find_text(WHALE_BACK)
        .unwrap_or_else(|| panic!("idle BlueWhale silhouette missing:\n{}", frame.debug_dump()));
    WHALE_BACK
        .chars()
        .filter_map(|ch| {
            let color = frame.colors_at(row, col).map(|colors| colors.0);
            col = col.saturating_add(
                u16::try_from(unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1)).unwrap_or(1),
            );
            color
        })
        .collect()
}

fn colored_foreground(frame: &qa_harness::Frame, needle: &str) -> qa_harness::Color {
    let (row, col) = frame
        .find_text(needle)
        .unwrap_or_else(|| panic!("{needle:?} missing:\n{}", frame.debug_dump()));
    let foreground = frame
        .colors_at(row, col)
        .expect("rendered text cell should have ANSI colors")
        .0;
    assert_ne!(
        foreground,
        qa_harness::Color::Default,
        "{needle:?} lost its semantic foreground:\n{}",
        frame.debug_dump()
    );
    foreground
}

fn phase_marker_for_label(frame: &qa_harness::Frame, label: &str) -> char {
    // Transcript cards may carry the same semantic word (for example the
    // collapsed `reasoning hidden` receipt). The phase strip is the lowest
    // matching row, immediately above the composer, so search bottom-up.
    let row = (0..frame.rows())
        .rev()
        .find(|&row| frame.row(row).contains(label))
        .unwrap_or_else(|| panic!("phase {label:?} missing:\n{}", frame.debug_dump()));
    let row_text = frame.row(row);
    let label_start = row_text
        .find(label)
        .expect("matched phase row should still contain its label");
    row_text[..label_start]
        .chars()
        .rev()
        .find(|ch| !ch.is_whitespace())
        .expect("phase row should contain a marker")
}

fn maybe_transcript_marker_before_icon(
    frame: &qa_harness::Frame,
    needle: &str,
    icon: &str,
) -> Option<char> {
    let row = visible_row_with_text(frame, needle)?;
    frame
        .row(row)
        .split_once(icon)?
        .0
        .chars()
        .rev()
        .find(|ch| !ch.is_whitespace())
}

fn wait_for_transcript_marker_before_icon(
    h: &mut Harness,
    needle: &str,
    icon: &str,
    timeout: Duration,
) -> anyhow::Result<char> {
    let mut captured = None;
    h.wait_for(
        |frame| {
            captured = maybe_transcript_marker_before_icon(frame, needle, icon);
            captured.is_some()
        },
        timeout,
    )?;
    captured.ok_or_else(|| {
        anyhow::anyhow!(
            "transcript marker before {icon:?} on row {needle:?} missing after wait:\n{}",
            h.frame().debug_dump()
        )
    })
}

fn horizontal_rule_fills(frame: &qa_harness::Frame, row: u16, cols: u16) -> bool {
    let text = frame.row(row);
    UnicodeWidthStr::width(text.as_str()) == usize::from(cols)
        && (text.chars().all(|ch| ch == '─') || text.chars().all(|ch| ch == '-'))
}

/// `Harness::resize` updates the parser dimensions immediately, before the
/// child has emitted its resized composition. Require the product's full-width
/// header and composer rules before accepting the frame so a preserved old
/// frame (or the clear between frames) cannot masquerade as a settled resize.
fn resize_and_wait_for_composition<F>(
    h: &mut Harness,
    rows: u16,
    cols: u16,
    mut predicate: F,
    timeout: Duration,
) -> anyhow::Result<()>
where
    F: FnMut(&qa_harness::Frame) -> bool,
{
    let already_sized = {
        let frame = h.frame();
        frame.rows() == rows && frame.cols() == cols
    };
    if !already_sized {
        h.resize(rows, cols)?;
    }
    h.wait_for(
        |frame| {
            let full_width_rules = (0..rows)
                .filter(|&row| horizontal_rule_fills(frame, row, cols))
                .count();
            // Brand-aware header: `cw 🐳` (emoji chip) or legacy `cw  ` spacing.
            let header = frame.row(0);
            let brand_header =
                header.contains("cw  ") || header.contains("cw 🐳") || header.starts_with("cw ");
            frame.rows() == rows
                && frame.cols() == cols
                && brand_header
                && horizontal_rule_fills(frame, 1, cols)
                && full_width_rules >= 2
                && frame.contains(COMPOSER_READY_TEXT)
                && predicate(frame)
        },
        timeout,
    )
}

fn assert_running_tool_lifecycle_frame(
    frame: &qa_harness::Frame,
    cols: u16,
    rows: u16,
) -> (qa_harness::Color, qa_harness::Color) {
    assert_real_pty_frame_geometry(frame, cols, rows);
    let dump = frame.debug_dump();
    assert!(
        frame.row(0).contains("act"),
        "Act missing from header:\n{dump}"
    );
    assert!(
        frame.row(0).contains("Full Access"),
        "effective Full Access missing from header:\n{dump}"
    );
    assert!(
        frame.contains("To-do ·"),
        "active To-do chrome missing:\n{dump}"
    );
    assert!(
        frame.contains("PTY lifecycle"),
        "canonical Work item missing:\n{dump}"
    );
    assert!(
        frame.contains("using tool"),
        "statusline did not name live tool use:\n{dump}"
    );
    assert!(
        frame.contains("run running"),
        "real Bash card did not remain live:\n{dump}"
    );
    assert!(
        frame.find_text("▗▄▄▄▄▄▄▄▄▄▄▄▖").is_none(),
        "idle BlueWhale should yield to functional transcript activity:\n{dump}"
    );
    let tool_row = visible_row_with_text(frame, "run running").expect("live Bash row");
    let tool_running = foreground_at_text(frame, tool_row, "running");
    assert_ne!(
        tool_running,
        qa_harness::Color::Default,
        "Bash running state lost its semantic foreground:\n{dump}"
    );
    (colored_foreground(frame, "using tool"), tool_running)
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

/// Motion-enabled companion to [`scroll_until`]. The underwater field keeps
/// emitting real frames while browsing history, so "no PTY bytes arrived" is
/// not a valid settle signal. Poll the desired rendered state directly after
/// each bounded input instead.
fn scroll_until_with_motion(h: &mut Harness, dir: ScrollDir, needle: &str) -> bool {
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
        if h.wait_for(|frame| frame.contains(needle), Duration::from_millis(160))
            .is_ok()
        {
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

/// #2886: drive the actual shipped TUI through a Unix PTY and a sealed
/// provider fixture. The first real tool establishes canonical To-do state;
/// the second is a real Bash process held live by a workspace sentinel so the
/// running card and statusline can be inspected without a timing race.
/// Captures, when requested, are parsed PTY frames emitted by the product.
#[test]
fn real_tool_lifecycle_crosses_work_status_resize_and_scroll_in_a_unix_pty() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    const RELEASE_SIGNAL: &str = "pty-tool-release.signal";

    let mut answer_lines = vec!["PTY-LIFECYCLE-HEAD".to_string()];
    for line in 1..=72 {
        answer_lines.push(format!("PTY-LIFECYCLE-LINE-{line:03}"));
    }
    answer_lines.push("PTY-LIFECYCLE-TAIL".to_string());
    let (base_url, server) =
        spawn_tool_lifecycle_screen_fixture(RELEASE_SIGNAL, answer_lines.join("\n"))?;

    let ws = make_sealed_workspace()?;
    let codewhale_home = ws.home().join(".codewhale");
    let codex_home = ws.home().join(".codex");
    std::fs::create_dir_all(&codex_home)?;
    std::fs::write(
        codewhale_home.join("config.toml"),
        "allow_shell = true\nreasoning_effort = \"low\"\n\n[retry]\nenabled = false\n\n[update]\ncheck_for_updates = false\n\n[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n",
    )?;
    std::fs::write(
        codewhale_home.join("settings.toml"),
        "theme = \"dark\"\nlocale = \"en\"\ndefault_mode = \"agent\"\npermission_posture = \"full-access\"\nlow_motion = false\nfancy_animations = true\ncomposer_border = true\n",
    )?;
    std::fs::write(
        codex_home.join("models_cache.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "fetched_at": chrono::Utc::now(),
            "models": [{"slug": "deepseek-v4-pro", "priority": 1}]
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
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", &base_url)
        .env("CODEWHALE_BASE_URL", &base_url)
        .env("DEEPSEEK_MODEL", "deepseek-v4-pro")
        .env("CODEWHALE_MODEL", "deepseek-v4-pro")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
            "--mouse-capture",
            "--yolo",
        ])
        .size(24, 80)
        .spawn()?;
    enter_launch_session(&mut h)?;

    // The authored BlueWhale is part of the real idle composition, with ANSI
    // ink emitted by the terminal renderer. Its text silhouette is stable;
    // the opt-in caustic shimmer changes cell colors without moving the mark.
    let initial_whale = whale_ansi_signature(h.frame());
    assert!(
        initial_whale
            .iter()
            .any(|color| *color != qa_harness::Color::Default),
        "idle BlueWhale lost its ANSI ink:\n{}",
        h.frame().debug_dump()
    );
    let shimmer_deadline = Instant::now() + Duration::from_secs(5);
    let mut shimmer_observed = false;
    while Instant::now() < shimmer_deadline {
        std::thread::sleep(Duration::from_millis(80));
        h.pump();
        if whale_ansi_signature(h.frame()) != initial_whale {
            shimmer_observed = true;
            break;
        }
    }
    assert!(
        shimmer_observed,
        "animated idle BlueWhale never changed ANSI cells:\n{}",
        h.frame().debug_dump()
    );

    for (cols, rows) in [(80_u16, 24_u16), (100, 30), (140, 40)] {
        resize_and_wait_for_composition(
            &mut h,
            rows,
            cols,
            |frame| {
                frame.rows() == rows
                    && frame.cols() == cols
                    && frame.find_text("▗▄▄▄▄▄▄▄▄▄▄▄▖").is_some()
            },
            KEY_TIMEOUT,
        )?;
        let frame = h.frame();
        assert_real_pty_frame_geometry(frame, cols, rows);
        assert_empty_state_hierarchy(frame, false);
        assert!(
            whale_ansi_signature(frame)
                .iter()
                .any(|color| *color != qa_harness::Color::Default),
            "BlueWhale ANSI ink missing at {cols}x{rows}:\n{}",
            frame.debug_dump()
        );
        write_real_pty_evidence(
            &format!("tool-lifecycle-idle-{cols}x{rows}"),
            &format!(
                "size={cols}x{rows}\nphase=idle\nreal_pty=true\nprovider=loopback\nbluewhale=true"
            ),
            frame,
        )?;
    }

    resize_and_wait_for_composition(
        &mut h,
        24,
        80,
        |frame| frame.rows() == 24 && frame.cols() == 80,
        KEY_TIMEOUT,
    )?;
    let prompt = "exercise the real PTY tool lifecycle";
    h.paste(prompt)?;
    h.wait_for_text(prompt, KEY_TIMEOUT)?;
    std::thread::sleep(Duration::from_millis(180));
    h.pump();
    h.send(keys::key::enter())?;
    h.wait_for(
        |frame| {
            frame.contains("using tool")
                && frame.contains("run running")
                && frame.contains("PTY lifecycle")
        },
        Duration::from_secs(15),
    )?;

    // Typed tool liveness belongs to the phase strip once transcript activity
    // replaces the idle BlueWhale. Prove the actual emitted marker advances;
    // do not relabel the decorative idle silhouette as a running tool row.
    let initial_tool_marker = phase_marker_for_label(h.frame(), "using tool");
    let marker_deadline = Instant::now() + Duration::from_secs(2);
    let mut tool_marker_moved = false;
    while Instant::now() < marker_deadline {
        std::thread::sleep(Duration::from_millis(80));
        h.pump();
        if phase_marker_for_label(h.frame(), "using tool") != initial_tool_marker {
            tool_marker_moved = true;
            break;
        }
    }
    assert!(
        tool_marker_moved,
        "using-tool phase marker never advanced in the real PTY:\n{}",
        h.frame().debug_dump()
    );

    let mut live_colors = None;
    for (cols, rows) in [(80_u16, 24_u16), (100, 30), (140, 40)] {
        resize_and_wait_for_composition(
            &mut h,
            rows,
            cols,
            |frame| {
                frame.rows() == rows
                    && frame.cols() == cols
                    && frame.contains("using tool")
                    && frame.contains("run running")
                    && frame.contains("PTY lifecycle")
            },
            KEY_TIMEOUT,
        )?;
        let frame = h.frame();
        let colors = assert_running_tool_lifecycle_frame(frame, cols, rows);
        if let Some(expected) = live_colors {
            assert_eq!(
                colors,
                expected,
                "using-tool/transcript ANSI roles changed at {cols}x{rows}:\n{}",
                frame.debug_dump()
            );
        } else {
            live_colors = Some(colors);
        }
        write_real_pty_evidence(
            &format!("tool-lifecycle-running-{cols}x{rows}"),
            &format!("size={cols}x{rows}\nphase=using-tool\nreal_tool=Bash\nlive_chrome=To-do"),
            frame,
        )?;
    }

    // Release the real shell process only after every live-size assertion.
    std::fs::write(ws.workspace().join(RELEASE_SIGNAL), "release\n")?;
    h.wait_for_text("PTY-LIFECYCLE-TAIL", Duration::from_secs(20))?;
    h.wait_for(
        |frame| frame.contains("✓ done") && !frame.contains("run running"),
        Duration::from_secs(10),
    )?;
    {
        let frame = h.frame();
        let dump = frame.debug_dump();
        assert_real_pty_frame_geometry(frame, 140, 40);
        assert!(
            frame.contains("PTY-LIFECYCLE-TAIL"),
            "tail not followed:\n{dump}"
        );
        assert!(
            !frame.contains("PTY-LIFECYCLE-HEAD"),
            "long settled transcript did not exceed the viewport:\n{dump}"
        );
        assert!(
            frame.contains("To-do ·"),
            "active To-do vanished after settlement:\n{dump}"
        );
        let done_row = visible_row_with_text(frame, "✓ done").expect("done phase row");
        let done_color = foreground_at_text(frame, done_row, "done");
        assert_ne!(
            done_color,
            qa_harness::Color::Default,
            "done lost ANSI role"
        );
        assert_ne!(
            done_color,
            live_colors.expect("live colors").0,
            "done and live tool use collapsed to one ANSI role"
        );
        write_real_pty_evidence(
            "tool-lifecycle-settled-140x40",
            "size=140x40\nphase=done\ntranscript=settled\nfollow_tail=true",
            frame,
        )?;
    }

    assert!(
        scroll_until_with_motion(&mut h, ScrollDir::Up, "PTY-LIFECYCLE-HEAD"),
        "settled transcript head is not reviewable at 140x40:\n{}",
        h.frame().debug_dump()
    );
    for (cols, rows) in [(100_u16, 30_u16), (80, 24)] {
        resize_and_wait_for_composition(
            &mut h,
            rows,
            cols,
            |frame| frame.rows() == rows && frame.cols() == cols,
            KEY_TIMEOUT,
        )?;
        assert!(
            h.frame().contains("PTY-LIFECYCLE-HEAD")
                || scroll_until_with_motion(&mut h, ScrollDir::Up, "PTY-LIFECYCLE-HEAD"),
            "transcript head was lost after reflow to {cols}x{rows}:\n{}",
            h.frame().debug_dump()
        );
        assert_real_pty_frame_geometry(h.frame(), cols, rows);
    }
    assert!(
        scroll_until_with_motion(&mut h, ScrollDir::Up, "run done"),
        "settled real Bash card is not retained in the transcript:\n{}",
        h.frame().debug_dump()
    );
    assert!(
        !h.frame().contains("run running"),
        "settled Bash card reverted to live state:\n{}",
        h.frame().debug_dump()
    );

    // The ordinary shortened-output hint is a real transcript row at every
    // release geometry. Select it with terminal mouse bytes, then exercise the
    // shipped Alt/Option+V detail shortcut; screenshots remain genuine PTY
    // frames and never include a fabricated product surface.
    for (cols, rows) in [(80_u16, 24_u16), (100, 30), (140, 40)] {
        resize_and_wait_for_composition(
            &mut h,
            rows,
            cols,
            |frame| frame.rows() == rows && frame.cols() == cols,
            KEY_TIMEOUT,
        )?;
        let hint_visible = h.frame().contains("Output shortened")
            || scroll_until_with_motion(&mut h, ScrollDir::Up, "Output shortened")
            || scroll_until_with_motion(&mut h, ScrollDir::Down, "Output shortened");
        assert!(
            hint_visible,
            "shortened-output hint is not reviewable at {cols}x{rows}:\n{}",
            h.frame().debug_dump()
        );
        let (row, col) = h
            .frame()
            .find_text("Output shortened")
            .expect("visible selectable shortened-output hint");
        h.send(keys::mouse::click(row, col))?;
        h.send(keys::key::alt('v'))?;
        h.wait_for(
            |frame| {
                frame.contains("Raw detail — Bash")
                    && frame.contains("Raw detail for the selected item")
            },
            KEY_TIMEOUT,
        )?;
        let frame = h.frame();
        assert_real_pty_frame_geometry(frame, cols, rows);
        assert!(!frame.contains("/artifacts/"));
        assert!(!frame.contains(".codewhale/sessions"));
        assert!(!frame.contains(ws.home().to_string_lossy().as_ref()));
        write_real_pty_evidence(
            &format!("tool-lifecycle-output-detail-{cols}x{rows}"),
            &format!(
                "size={cols}x{rows}\nreal_pty=true\npreview=selected\nshortcut=Alt+V\npath_leak=false"
            ),
            frame,
        )?;
        h.send(keys::key::ch('q'))?;
        h.wait_for(|frame| !frame.contains("Raw detail — Bash"), KEY_TIMEOUT)?;
    }

    resize_and_wait_for_composition(
        &mut h,
        24,
        80,
        |frame| frame.rows() == 24 && frame.cols() == 80,
        KEY_TIMEOUT,
    )?;
    assert!(
        scroll_until_with_motion(&mut h, ScrollDir::Down, "PTY-LIFECYCLE-TAIL"),
        "follow-tail is not restorable at 80x24:\n{}",
        h.frame().debug_dump()
    );

    for (cols, rows) in [(100_u16, 30_u16), (140, 40)] {
        resize_and_wait_for_composition(
            &mut h,
            rows,
            cols,
            |frame| frame.rows() == rows && frame.cols() == cols,
            KEY_TIMEOUT,
        )?;
        assert!(
            h.frame().contains("PTY-LIFECYCLE-TAIL")
                || scroll_until_with_motion(&mut h, ScrollDir::Down, "PTY-LIFECYCLE-TAIL"),
            "follow-tail was lost after reflow to {cols}x{rows}:\n{}",
            h.frame().debug_dump()
        );
        let frame = h.frame();
        assert_real_pty_frame_geometry(frame, cols, rows);
        assert!(
            frame.contains("To-do ·"),
            "active To-do missing at {cols}x{rows}"
        );
    }

    let artifact_dir = std::fs::read_dir(ws.home().join(".codewhale/sessions"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("artifacts"))
        .find(|path| path.join("art_call_bash_pty.txt").is_file())
        .ok_or_else(|| anyhow::anyhow!("real PTY Bash evidence artifact was not retained"))?;
    let exact = std::fs::read(artifact_dir.join("art_call_bash_pty.txt"))?;
    assert!(
        String::from_utf8_lossy(&exact).contains("PTY-EVIDENCE-DEEP-SENTINEL"),
        "real PTY artifact lost its deep sentinel"
    );
    let metadata: serde_json::Value = serde_json::from_slice(&std::fs::read(
        artifact_dir.join("art_call_bash_pty.evidence.json"),
    )?)?;
    assert_eq!(metadata["handle"], "art_call_bash_pty");
    assert_eq!(metadata["call_id"], "call_bash_pty");
    assert_eq!(metadata["tool_name"], "Bash");
    assert_eq!(metadata["size_bytes"], exact.len() as u64);
    let digest = Sha256::digest(&exact)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        metadata["digest"], digest,
        "real PTY metadata digest must bind the exact bytes"
    );

    let _ = h.shutdown();
    server
        .join()
        .expect("tool lifecycle fixture server thread")?;
    Ok(())
}

/// The semantic phase strip is acceptance-tested against the shipped binary,
/// not a synthetic widget. A sealed loopback stream holds explicit private
/// reasoning, canonical File.read on a FIFO, and Bash on a sentinel long
/// enough for the real PTY to capture each truthful one-row label.
#[test]
fn semantic_activity_motion_crosses_reasoning_reading_and_tool_use_in_a_real_unix_pty()
-> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();

    #[derive(Clone, Copy)]
    struct Case {
        name: &'static str,
        theme: &'static str,
        motion_mode: &'static str,
        reduced_motion: bool,
        fancy_animations: bool,
        expect_motion: bool,
        static_marker: Option<char>,
        ascii_safe: bool,
        reasoning_size: (u16, u16),
        reading_size: (u16, u16),
        tool_size: (u16, u16),
    }

    let cases = [
        Case {
            name: "dark-motion",
            theme: "dark",
            motion_mode: "full",
            reduced_motion: false,
            fancy_animations: true,
            expect_motion: true,
            static_marker: None,
            ascii_safe: false,
            reasoning_size: (100, 32),
            reading_size: (50, 16),
            tool_size: (140, 40),
        },
        Case {
            name: "light-reduced",
            theme: "light",
            motion_mode: "reduced",
            reduced_motion: true,
            fancy_animations: false,
            expect_motion: false,
            static_marker: Some('⣤'),
            ascii_safe: false,
            reasoning_size: (100, 32),
            reading_size: (80, 24),
            tool_size: (100, 32),
        },
        Case {
            name: "dark-ascii",
            theme: "dark",
            motion_mode: "full",
            reduced_motion: false,
            fancy_animations: true,
            expect_motion: true,
            static_marker: None,
            ascii_safe: true,
            reasoning_size: (80, 24),
            reading_size: (80, 24),
            tool_size: (80, 24),
        },
        Case {
            name: "dark-still",
            theme: "dark",
            motion_mode: "still",
            reduced_motion: false,
            fancy_animations: false,
            expect_motion: false,
            static_marker: Some('›'),
            ascii_safe: false,
            reasoning_size: (100, 32),
            reading_size: (80, 24),
            tool_size: (100, 32),
        },
    ];

    for case in cases {
        const FIFO_NAME: &str = "semantic-motion-read.fifo";
        const REASONING_RELEASE: &str = "semantic-motion-reasoning.release";
        const BASH_RELEASE: &str = "semantic-motion-bash.release";

        let ws = make_sealed_workspace()?;
        let fifo_path = ws.workspace().join(FIFO_NAME);
        let mkfifo = Command::new("mkfifo").arg(&fifo_path).status()?;
        anyhow::ensure!(
            mkfifo.success(),
            "mkfifo failed for {}",
            fifo_path.display()
        );

        let reasoning_release = ws.workspace().join(REASONING_RELEASE);
        let (base_url, server) = spawn_semantic_activity_motion_fixture(
            reasoning_release.clone(),
            FIFO_NAME,
            BASH_RELEASE,
        )?;

        let codewhale_home = ws.home().join(".codewhale");
        let codex_home = ws.home().join(".codex");
        std::fs::create_dir_all(&codex_home)?;
        std::fs::write(
            codewhale_home.join("config.toml"),
            "allow_shell = true\nreasoning_effort = \"low\"\n\n[retry]\nenabled = false\n\n[update]\ncheck_for_updates = false\n\n[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n",
        )?;
        std::fs::write(
            codewhale_home.join("settings.toml"),
            format!(
                "theme = \"{}\"\nlocale = \"en\"\ndefault_mode = \"agent\"\npermission_posture = \"full-access\"\nshow_thinking = false\nlow_motion = {}\nfancy_animations = {}\ncomposer_border = true\n",
                case.theme, case.reduced_motion, case.fancy_animations,
            ),
        )?;
        std::fs::write(
            codex_home.join("models_cache.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "fetched_at": chrono::Utc::now(),
                "models": [{"slug": "deepseek-v4-pro", "priority": 1}]
            }))?,
        )?;

        let mut builder = Harness::builder(Harness::cargo_bin("codewhale-tui"))
            .cwd(ws.workspace())
            .clear_env()
            .seal_home(ws.home())
            .env("CODEWHALE_HOME", codewhale_home.to_string_lossy())
            .env(
                "DEEPSEEK_CONFIG_PATH",
                codewhale_home.join("config.toml").to_string_lossy(),
            )
            .env("CODEX_HOME", codex_home.to_string_lossy())
            .env("CODEWHALE_PROVIDER", "deepseek")
            .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
            .env("DEEPSEEK_BASE_URL", &base_url)
            .env("CODEWHALE_BASE_URL", &base_url)
            .env("DEEPSEEK_MODEL", "deepseek-v4-pro")
            .env("CODEWHALE_MODEL", "deepseek-v4-pro")
            .env("RUST_LOG", "warn")
            .args([
                "--workspace",
                ws.workspace().to_str().expect("utf-8 workspace path"),
                "--no-project-config",
                "--skip-onboarding",
                "--mouse-capture",
                "--yolo",
            ])
            .size(case.reasoning_size.1, case.reasoning_size.0);
        if case.reduced_motion {
            builder = builder.env("NO_ANIMATIONS", "1");
        }
        if case.ascii_safe {
            builder = builder.env("CODEWHALE_ASCII_SAFE", "1");
        }
        let mut h = builder.spawn()?;
        enter_launch_session(&mut h)?;

        let prompt = "show semantic activity with less chrome";
        h.paste(prompt)?;
        h.wait_for_text(prompt, KEY_TIMEOUT)?;
        h.send(keys::key::enter())?;
        h.wait_for(|frame| frame.contains("reasoning"), Duration::from_secs(15))?;

        let reasoning_marker = phase_marker_for_label(h.frame(), "reasoning");
        if !case.expect_motion {
            assert_eq!(
                Some(reasoning_marker),
                case.static_marker,
                "wrong semantic fallback marker in {}:\n{}",
                case.name,
                h.frame().debug_dump()
            );
            std::thread::sleep(Duration::from_millis(320));
            h.pump();
            assert_eq!(
                phase_marker_for_label(h.frame(), "reasoning"),
                reasoning_marker,
                "static reasoning marker moved in {}:\n{}",
                case.name,
                h.frame().debug_dump()
            );
        } else {
            let static_marker = if case.ascii_safe { '>' } else { '›' };
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut first_animated =
                (reasoning_marker != static_marker).then_some(reasoning_marker);
            while Instant::now() < deadline && first_animated.is_none() {
                std::thread::sleep(Duration::from_millis(80));
                h.pump();
                let marker = phase_marker_for_label(h.frame(), "reasoning");
                if marker != static_marker {
                    first_animated = Some(marker);
                }
            }
            let first_animated = first_animated.unwrap_or_else(|| {
                panic!(
                    "semantic reasoning marker never crossed its earned-motion delay in {}:\n{}",
                    case.name,
                    h.frame().debug_dump()
                )
            });

            let deadline = Instant::now() + Duration::from_secs(2);
            let mut advanced_after_delay = false;
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(80));
                h.pump();
                let marker = phase_marker_for_label(h.frame(), "reasoning");
                if marker != static_marker && marker != first_animated {
                    advanced_after_delay = true;
                    break;
                }
            }
            assert!(
                advanced_after_delay,
                "semantic reasoning marker froze after its earned-motion delay in {}:\n{}",
                case.name,
                h.frame().debug_dump()
            );
        }
        {
            let frame = h.frame();
            let dump = frame.debug_dump();
            assert_real_pty_frame_geometry(frame, case.reasoning_size.0, case.reasoning_size.1);
            assert!(
                !frame.contains("PRIVATE-MOTION-TRACE-MUST-STAY-HIDDEN"),
                "private reasoning leaked into the product UI:\n{dump}"
            );
            colored_foreground(frame, "reasoning");
            if case.ascii_safe {
                assert!(
                    frame.text().is_ascii(),
                    "ASCII-safe reasoning frame:\n{dump}"
                );
            }
            write_real_pty_evidence(
                &format!(
                    "semantic-{}-reasoning-{}x{}",
                    case.name, case.reasoning_size.0, case.reasoning_size.1
                ),
                &format!(
                    "theme={}\nphase=reasoning\nmotion_mode={}\nreduced_motion={}\nfancy_animations={}\nascii_safe={}\nprivate_reasoning_visible=false",
                    case.theme,
                    case.motion_mode,
                    case.reduced_motion,
                    case.fancy_animations,
                    case.ascii_safe
                ),
                frame,
            )?;
        }

        std::fs::write(&reasoning_release, "release\n")?;
        h.wait_for(|frame| frame.contains("reading"), Duration::from_secs(15))?;
        resize_and_wait_for_composition(
            &mut h,
            case.reading_size.1,
            case.reading_size.0,
            |frame| frame.contains("reading"),
            KEY_TIMEOUT,
        )?;
        {
            let frame = h.frame();
            let dump = frame.debug_dump();
            let row = visible_row_with_text(frame, "reading").expect("reading phase row");
            let row_text = frame.row(row);
            assert_real_pty_frame_geometry(frame, case.reading_size.0, case.reading_size.1);
            assert!(
                !frame.contains("PRIVATE-MOTION-TRACE-MUST-STAY-HIDDEN"),
                "private reasoning leaked during File.read:\n{dump}"
            );
            assert!(
                frame.contains("read running") && frame.contains("live: Reading"),
                "File.read transcript spacing collapsed:\n{dump}"
            );
            if case.reading_size.0 < 60 {
                assert!(
                    !row_text.contains('×') && !row_text.contains('s'),
                    "compact semantic row carried detail: {row_text:?}"
                );
            }
            colored_foreground(frame, "reading");
            if case.ascii_safe {
                assert!(frame.text().is_ascii(), "ASCII-safe reading frame:\n{dump}");
            }
            write_real_pty_evidence(
                &format!(
                    "semantic-{}-reading-{}x{}",
                    case.name, case.reading_size.0, case.reading_size.1
                ),
                &format!(
                    "theme={}\nphase=reading\nreal_tool=File.read\nsize={}x{}\nmotion_mode={}\nreduced_motion={}\nfancy_animations={}\nascii_safe={}\nprivate_reasoning_visible=false",
                    case.theme,
                    case.reading_size.0,
                    case.reading_size.1,
                    case.motion_mode,
                    case.reduced_motion,
                    case.fancy_animations,
                    case.ascii_safe
                ),
                frame,
            )?;
        }

        release_semantic_read_fifo(fifo_path)
            .join()
            .expect("FIFO release thread")?;
        h.wait_for(
            |frame| frame.contains("using tool") && frame.contains("run running"),
            Duration::from_secs(15),
        )?;
        resize_and_wait_for_composition(
            &mut h,
            case.tool_size.1,
            case.tool_size.0,
            |frame| frame.contains("using tool") && frame.contains("run running"),
            KEY_TIMEOUT,
        )?;
        {
            let frame = h.frame();
            let dump = frame.debug_dump();
            let row = visible_row_with_text(frame, "using tool").expect("tool phase row");
            let row_text = frame.row(row);
            assert_real_pty_frame_geometry(frame, case.tool_size.0, case.tool_size.1);
            assert!(
                frame.contains("read done")
                    && frame.contains("done: Reading")
                    && frame.contains("run running"),
                "completed/read and live/Bash transcript spacing collapsed:\n{dump}"
            );
            if case.tool_size.0 >= 60 {
                let count = if case.ascii_safe { "X1" } else { "×1" };
                assert!(
                    row_text.contains(count),
                    "bounded tool count missing: {row_text:?}"
                );
            }
            assert!(
                !row_text.contains("run ×1"),
                "tool verb repeated: {row_text:?}"
            );
            colored_foreground(frame, "using tool");
            if case.ascii_safe {
                assert!(frame.text().is_ascii(), "ASCII-safe tool frame:\n{dump}");
            }
            write_real_pty_evidence(
                &format!(
                    "semantic-{}-using-tool-{}x{}",
                    case.name, case.tool_size.0, case.tool_size.1
                ),
                &format!(
                    "theme={}\nphase=using-tool\nreal_tool=Bash.run\nsize={}x{}\nmotion_mode={}\nreduced_motion={}\nfancy_animations={}\nascii_safe={}\nprivate_reasoning_visible=false",
                    case.theme,
                    case.tool_size.0,
                    case.tool_size.1,
                    case.motion_mode,
                    case.reduced_motion,
                    case.fancy_animations,
                    case.ascii_safe
                ),
                frame,
            )?;
        }

        if case.expect_motion && !case.ascii_safe {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut first_animated = None;
            while Instant::now() < deadline && first_animated.is_none() {
                std::thread::sleep(Duration::from_millis(80));
                h.pump();
                if let Some(marker) =
                    maybe_transcript_marker_before_icon(h.frame(), "run running", "▶")
                    && marker != '›'
                {
                    first_animated = Some(marker);
                }
            }
            let first_animated = first_animated.unwrap_or_else(|| {
                panic!(
                    "full-motion transcript tool marker never crossed its earned-motion delay in {}:\n{}",
                    case.name,
                    h.frame().debug_dump()
                )
            });

            let deadline = Instant::now() + Duration::from_secs(2);
            let mut advanced_after_delay = false;
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(80));
                h.pump();
                if let Some(marker) =
                    maybe_transcript_marker_before_icon(h.frame(), "run running", "▶")
                    && marker != '›'
                    && marker != first_animated
                {
                    advanced_after_delay = true;
                    break;
                }
            }
            assert!(
                advanced_after_delay,
                "full-motion transcript tool marker froze after its earned-motion delay in {}:\n{}",
                case.name,
                h.frame().debug_dump()
            );
        } else if !case.ascii_safe {
            let initial_tool_marker =
                wait_for_transcript_marker_before_icon(&mut h, "run running", "▶", KEY_TIMEOUT)?;
            assert_eq!(
                Some(initial_tool_marker),
                case.static_marker,
                "wrong transcript fallback marker in {}:\n{}",
                case.name,
                h.frame().debug_dump()
            );
            std::thread::sleep(Duration::from_millis(320));
            h.pump();
            let marker_after_delay =
                wait_for_transcript_marker_before_icon(&mut h, "run running", "▶", KEY_TIMEOUT)?;
            assert_eq!(
                marker_after_delay,
                initial_tool_marker,
                "static transcript tool marker moved in {}:\n{}",
                case.name,
                h.frame().debug_dump()
            );
            resize_and_wait_for_composition(
                &mut h,
                case.tool_size.1,
                case.tool_size.0 + 1,
                |frame| frame.contains("using tool") && frame.contains("run running"),
                KEY_TIMEOUT,
            )?;
            resize_and_wait_for_composition(
                &mut h,
                case.tool_size.1,
                case.tool_size.0,
                |frame| frame.contains("using tool") && frame.contains("run running"),
                KEY_TIMEOUT,
            )?;
            let marker_after_resize =
                wait_for_transcript_marker_before_icon(&mut h, "run running", "▶", KEY_TIMEOUT)?;
            assert_eq!(
                marker_after_resize,
                initial_tool_marker,
                "state-change redraw moved a static transcript marker in {}:\n{}",
                case.name,
                h.frame().debug_dump()
            );
        }

        std::fs::write(ws.workspace().join(BASH_RELEASE), "release\n")?;
        h.wait_for_text("SEMANTIC-MOTION-DONE", Duration::from_secs(20))?;
        h.wait_for(
            |frame| frame.contains("done") && !frame.contains("run running"),
            Duration::from_secs(10),
        )?;
        let _ = h.shutdown();
        server
            .join()
            .expect("semantic activity fixture server thread")?;
    }

    Ok(())
}

/// SSE fixture for the transcript-rhythm probe: one turn that reasons, says
/// something, and runs a shell command; then a closing turn.
fn spawn_transcript_rhythm_fixture(
    shell_command: String,
) -> anyhow::Result<(String, std::thread::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;

    let reasoning = format!(
        "data: {}\n\n",
        serde_json::json!({
            "id": "chatcmpl-rhythm",
            "object": "chat.completion.chunk",
            "model": "deepseek-v4-pro",
            "choices": [{
                "index": 0,
                "delta": {"reasoning_content":
                    "PROBEREASONING the listing command is the one to run here."},
                "finish_reason": null
            }]
        })
    );
    let prose = format!(
        "data: {}\n\n",
        serde_json::json!({
            "id": "chatcmpl-rhythm",
            "object": "chat.completion.chunk",
            "model": "deepseek-v4-pro",
            "choices": [{
                "index": 0,
                "delta": {"content": "PROBEANSWERA Running the listing now."},
                "finish_reason": null
            }]
        })
    );
    let call = pty_tool_call_sse(
        "call_rhythm_probe",
        "Bash",
        serde_json::json!({
            "action": "run",
            "command": shell_command,
            "timeout_ms": 60_000
        }),
    );
    let first = format!("{reasoning}{prose}{call}");
    let second = pty_text_sse("PROBEANSWERB That is the full listing.");

    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut chat_index = 0usize;
        while chat_index < 2 && Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let Ok(request) = read_http_request(&mut stream) else {
                continue;
            };
            let request_line = request.lines().next().unwrap_or_default();
            if request_line.starts_with("GET ") && request_line.contains("/models") {
                let body = serde_json::json!({
                    "object": "list",
                    "data": [{"id": "deepseek-v4-pro", "object": "model"}]
                })
                .to_string();
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
                let _ = stream.flush();
                continue;
            }
            if !(request_line.starts_with("POST ") && request_line.contains("/chat/completions")) {
                continue;
            }
            let body = if chat_index == 0 { &first } else { &second };
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = stream.flush();
            chat_index += 1;
        }
    });
    Ok((format!("http://{address}"), handle))
}

/// Boot a probe session against the rhythm fixture and return the settled
/// frame rows plus its debug dump.
fn run_transcript_rhythm_probe(
    show_tool_details: bool,
    total_rows: usize,
) -> anyhow::Result<(Vec<String>, String, Vec<String>)> {
    let ws = make_sealed_workspace()?;
    let shell_command = format!(
        "for i in $(seq 0 {}); do printf 'row %02d plain content\\n' \"$i\"; done",
        total_rows - 1
    );
    let (base_url, server) = spawn_transcript_rhythm_fixture(shell_command)?;

    let codewhale_home = ws.home().join(".codewhale");
    let codex_home = ws.home().join(".codex");
    std::fs::create_dir_all(&codex_home)?;
    std::fs::write(
        codewhale_home.join("config.toml"),
        "allow_shell = true\n\n[retry]\nenabled = false\n\n[update]\ncheck_for_updates = false\n\n[notifications]\nmethod = \"off\"\ncompletion_sound = \"off\"\n",
    )?;
    std::fs::write(
        codewhale_home.join("settings.toml"),
        format!(
            // `show_thinking = true` is the shape the complaint was made
            // about: the owner could see the reasoning body, and it ran
            // straight into the answer underneath it.
            "locale = \"en\"\ndefault_mode = \"agent\"\npermission_posture = \"full-access\"\nshow_thinking = true\nshow_tool_details = {show_tool_details}\ntranscript_spacing = \"comfortable\"\ncomposer_border = true\n"
        ),
    )?;
    std::fs::write(
        codex_home.join("models_cache.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "fetched_at": chrono::Utc::now(),
            "models": [{"slug": "deepseek-v4-pro", "priority": 1}]
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
        .env("CODEWHALE_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "deepseek-local-test-key")
        .env("DEEPSEEK_BASE_URL", &base_url)
        .env("CODEWHALE_BASE_URL", &base_url)
        .env("DEEPSEEK_MODEL", "deepseek-v4-pro")
        .env("CODEWHALE_MODEL", "deepseek-v4-pro")
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
            "--yolo",
        ])
        // Tall enough that the whole probe turn lands on one frame; the
        // assertions are about the rows between blocks, not about scrolling.
        .size(64, 100)
        .spawn()?;
    enter_launch_session(&mut h)?;

    h.paste(TRANSCRIPT_RHYTHM_PROMPT)?;
    h.wait_for_text(TRANSCRIPT_RHYTHM_PROMPT, KEY_TIMEOUT)?;
    h.send(keys::key::enter())?;
    let dump = wait_for_frame_dump(
        &mut h,
        |frame| frame.contains("PROBEANSWERB"),
        Duration::from_secs(30),
    )?;
    let frame = h.frame();
    write_real_pty_evidence(
        &format!("transcript-rhythm-details-{show_tool_details}"),
        "size=64x100 spacing=comfortable show_thinking=true",
        frame,
    )?;
    if std::env::var_os("CODEWHALE_QA_PRINT_FRAME").is_some() {
        println!("--- show_tool_details={show_tool_details}\n{dump}");
    }
    let rows: Vec<String> = (0..frame.rows()).map(|y| frame.row(y)).collect();
    let painted: Vec<String> = rows.clone();
    let _ = h.shutdown();
    drop(server);
    Ok((rows, dump, painted))
}

const TRANSCRIPT_RHYTHM_PROMPT: &str = "list the rows";

/// Transcript vertical rhythm + live run-card budget, measured on real
/// terminal output rather than on a renderer unit test.
///
/// The owner's complaint had two halves and both are properties of painted
/// rows, so both are asserted on a parsed PTY frame:
///
/// * consecutive blocks ran together with no blank row — a reasoning block
///   flowing straight into the answer that followed it;
/// * the run cards showed so little of their output that even the truncated
///   view could not tell you what happened. A *successful* run showed its
///   header and nothing else at all.
///
/// The shell command here really runs (`--yolo`), so the card under assertion
/// is a real one carrying real output.
///
/// Set `CODEWHALE_QA_EVIDENCE_DIR` to capture the frame dumps,
/// `CODEWHALE_QA_PRINT_FRAME=1` to print them.
#[test]
fn transcript_blocks_are_separated_and_run_cards_show_real_output() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();
    // Twenty-four rows of unremarkable output: nothing an importance filter
    // would rescue, which is the case the head/tail split served worst.
    let total_rows = 24usize;

    // Leg A — shipped defaults (`show_tool_details = false`). This is what
    // almost every user sees, and it is the frame the complaint was about.
    let (rows, dump, painted) = run_transcript_rhythm_probe(false, total_rows)?;
    let painted_contains =
        |needle: &str, rows: &[String]| rows.iter().any(|row| row.contains(needle));
    let row_of = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("missing {needle} in frame:\n{dump}"))
    };
    let blank_between = |a: usize, b: usize| {
        rows[a.min(b) + 1..a.max(b)]
            .iter()
            .any(|row| row.trim().is_empty())
    };

    // 1. Reasoning must not run straight into the answer that follows it.
    //    This is the specific seam the owner pointed at.
    let reasoning = row_of("PROBEREASONING");
    let answer_a = row_of("PROBEANSWERA");
    assert!(
        answer_a > reasoning,
        "the answer should follow the reasoning:\n{dump}"
    );
    assert!(
        blank_between(reasoning, answer_a),
        "a reasoning block and the answer after it need a blank row between \
         them:\n{dump}"
    );

    // 2. Assistant prose must not run straight into the tool card.
    let card = row_of("row 00 plain content");
    assert!(
        blank_between(answer_a, card),
        "assistant prose and the tool card below it need a blank row:\n{dump}"
    );

    // 3. The user's own turn stays a visible seam.
    let user = row_of(TRANSCRIPT_RHYTHM_PROMPT);
    assert!(
        blank_between(user, reasoning),
        "the user turn needs a visible seam:\n{dump}"
    );

    // 4. Nowhere does a second separator row stack on the first. A scrolling
    //    terminal cannot afford a two-row gap and it reads as a hole.
    let last = row_of("PROBEANSWERB");
    assert!(
        !rows[user..=last]
            .windows(2)
            .any(|pair| pair[0].trim().is_empty() && pair[1].trim().is_empty()),
        "no two separator rows may stack:\n{dump}"
    );

    // 5. A *successful* run card used to paint its header and nothing else.
    //    On shipped defaults it must now carry real output rows.
    let shown = (0..total_rows)
        .filter(|i| painted_contains(&format!("row {i:02} plain content"), &painted))
        .count();
    assert!(
        shown >= 4,
        "a successful run card on shipped defaults painted {shown} output \
         rows; it used to paint none and must now show enough to tell what \
         happened:\n{dump}"
    );

    // Leg B — `show_tool_details = true`, where the card spends the full
    // output budget rather than the summary cap.
    let (_rows, detail_dump, detail_painted) = run_transcript_rhythm_probe(true, total_rows)?;
    let detailed = (0..total_rows)
        .filter(|i| painted_contains(&format!("row {i:02} plain content"), &detail_painted))
        .count();
    assert!(
        detailed > shown,
        "show_tool_details must reveal more than the summary card \
         ({detailed} vs {shown}):\n{detail_dump}"
    );
    assert!(
        detailed >= 6,
        "a detailed run card painted only {detailed} output rows:\n{detail_dump}"
    );

    Ok(())
}

/// Two of the owner's reported display defects, both measured on real
/// terminal output.
///
/// 1. **The composer looked like it was cutting text off.** It was not losing
///    anything — it broke lines on whatever grapheme crossed the margin, so a
///    wrapped sentence split mid-word (`…Write the file onl` / `y after…`),
///    which reads exactly like truncation. Assert that no wrapped composer
///    line ends inside a word and that every word survives.
///
/// 2. **A one-row toast was cut to uselessness.** Opening a second CodeWhale
///    in the same workspace loses the coordination flock and raises a sticky
///    warning. At a flat 40-column budget it painted `Delegated coordination
///    unavailable — an…`. Here a real second session is booted against the
///    same workspace and the strip must actually say what happened.
#[test]
fn composer_wraps_between_words_and_the_lock_toast_stays_legible() -> anyhow::Result<()> {
    let _guard = qa_pty_test_lock();

    // --- Leg 1: composer wrapping.
    let (ws, mut h) = boot_minimal()?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    let typed = "Mark inferences as inferences. A short PRD where each section decides something beats a long one that merely describes. Write the file only after the outline is agreed.";
    h.paste(typed)?;
    h.wait_for_text("outline is agreed", KEY_TIMEOUT)?;
    let composer_dump = h.frame().debug_dump();
    write_real_pty_evidence_dump("composer-wrap", "size=40x140", &composer_dump)?;
    if std::env::var_os("CODEWHALE_QA_PRINT_FRAME").is_some() {
        println!("--- composer wrap\n{composer_dump}");
    }

    let frame = h.frame();
    let rows: Vec<String> = (0..frame.rows()).map(|y| frame.row(y)).collect();
    // The composer rows are the ones carrying the typed text.
    let composer_rows: Vec<&String> = rows
        .iter()
        .filter(|row| {
            typed
                .split(' ')
                .any(|word| word.len() > 6 && row.contains(word))
        })
        .collect();
    assert!(
        composer_rows.len() > 1,
        "the probe text must wrap to more than one row:\n{composer_dump}"
    );
    // Every word of the input survives somewhere on the frame, whole.
    for word in typed.split(' ').filter(|word| word.len() > 3) {
        let word = word.trim_end_matches(['.', ',']);
        assert!(
            rows.iter().any(|row| row.contains(word)),
            "word {word:?} was split across the wrap and no row holds it \
             whole:\n{composer_dump}"
        );
    }
    let _ = h.shutdown();
    drop(ws);

    // --- Leg 2: the one-row status toast budget.
    //
    // The coordination-lock warning the owner hit is one instance of a
    // general defect: every sticky toast was truncated to a flat 40 columns
    // no matter how wide the terminal was. A failed `/load` raises a long
    // sticky warning through the same strip, and reproduces it in two
    // keystrokes without needing two sessions racing a flock. The lock
    // warning's own copy is pinned in
    // `tui::ui::tests::coordination_lock_loss_warns_only_for_a_foreign_owner`.
    let ws = make_sealed_workspace()?;
    let session_path = ws.workspace().join("broken-session.json");
    std::fs::write(
        &session_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "metadata": {
                "id": "pty-broken",
                "title": "Broken",
                "created_at": "2026-08-04T00:00:00Z",
                "updated_at": "2026-08-04T00:00:00Z",
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
            // A non-empty legacy Work view with no graph fails validation and
            // raises a long, explanatory warning — exactly the class of
            // message a 40-column budget destroys.
            "work_state": {
                "todos": {"items": [], "completion_pct": 0, "in_progress_id": null},
                "plan": {"objective": "", "items": []}
            }
        }))?,
    )?;
    let (_ws2, mut h) = spawn_minimal_with_env(ws, &[])?;
    h.wait_for_text(COMPOSER_READY_TEXT, BOOT_TIMEOUT)?;
    enter_launch_session(&mut h)?;
    h.send(keys::key::text(&format!(
        "/load {}",
        session_path.to_string_lossy()
    )))?;
    h.wait_for_idle(Duration::from_millis(150), Duration::from_secs(2))?;
    h.send(keys::key::enter())?;
    let toast_dump = wait_for_frame_dump(
        &mut h,
        |frame| frame.contains("Failed to restore session"),
        Duration::from_secs(10),
    )?;
    write_real_pty_evidence_dump("status-toast-budget", "size=40x140", &toast_dump)?;
    if std::env::var_os("CODEWHALE_QA_PRINT_FRAME").is_some() {
        println!("--- status toast\n{toast_dump}");
    }

    let frame = h.frame();
    let strip = (0..frame.rows())
        .map(|y| frame.row(y))
        .find(|row| row.contains("Failed to restore session"))
        .expect("the strip must carry the warning");
    let warning = strip
        .split_once("Failed to restore session")
        .map(|(_, tail)| format!("Failed to restore session{tail}"))
        .unwrap_or_default();
    let warning = warning
        .split("  ")
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    assert!(
        warning.chars().count() > 40,
        "the strip painted {} columns of a long warning on a 140-column \
         terminal; the flat 40-column budget was the defect:\n{toast_dump}",
        warning.chars().count()
    );
    assert!(
        warning.contains("Work Graph"),
        "the truncated warning must still reach the part that explains it: \
         {warning:?}\n{toast_dump}"
    );

    let _ = h.shutdown();
    Ok(())
}
