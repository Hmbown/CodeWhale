//! Automated terminal / key / modal acceptance matrix for #3758.
//!
//! #3758 asked for a multi-terminal QA pass. Most of that report is
//! automatable: terminal geometry, `TERM`/`COLORTERM` capability tiers, the
//! Unicode→ASCII fallback, every paste shape a real terminal can deliver,
//! IME-style commits, mouse/resize/focus events, and every modal open/close
//! path. Those cells run here, provider-free, in real pseudo-terminals.
//!
//! What deliberately does **not** live here: literal physical observations in
//! iTerm2 / Terminal.app / WezTerm / a real SSH session. A PTY reproduces the
//! byte protocol, not the emulator, so those rows stay `UNRUN` in
//! `docs/releases/v0.9.2-terminal-matrix.md` rather than being claimed by
//! proxy.
//!
//! Conventions this file holds to, because the alternative is a matrix that
//! looks green and proves nothing:
//!
//! - **No sleep as correctness.** Every wait is a bounded poll on a real
//!   signal (rendered text, view-stack trace record, process exit) and fails
//!   with the frame *and* the terminal-mode ledger. The only fixed sleeps are
//!   the paste-burst settle windows, which are part of the behaviour under
//!   test, not a substitute for synchronisation.
//! - **No duplicate shortcut table.** Chord coverage is scraped from the help
//!   overlay the product actually renders from
//!   `crate::tui::keybindings::KEYBINDINGS`; the catalog's own invariants are
//!   pinned by unit tests next to it.
//! - **Terminal modes are checked on every exit path**, from the raw control
//!   stream rather than the screen.

#![cfg(unix)]

#[path = "support/qa_harness/mod.rs"]
mod qa_harness;

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use qa_harness::harness::{Harness, SealedWorkspace, make_sealed_workspace};
use qa_harness::modes::{MODES_THAT_MUST_NOT_LEAK, mode};
use qa_harness::view_log::{self, VIEW_STACK_RUST_LOG};
use qa_harness::{Frame, keys};

const BOOT_TIMEOUT: Duration = Duration::from_secs(20);
const KEY_TIMEOUT: Duration = Duration::from_secs(6);
const MODAL_TIMEOUT: Duration = Duration::from_secs(8);
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
/// The paste-burst detector suppresses a trailing Enter for ~120 ms after a
/// burst. Waiting past it is part of the contract under test, not a stand-in
/// for synchronisation.
const PASTE_GUARD_SETTLE: Duration = Duration::from_millis(180);
const COMPOSER_READY_TEXT: &str = "Write a task";

/// PTY scenarios each boot a real binary and contend for CPU; run them one at
/// a time so a wait budget measures the product, not the runner.
static TERMINAL_MATRIX_LOCK: Mutex<()> = Mutex::new(());

fn matrix_lock() -> MutexGuard<'static, ()> {
    TERMINAL_MATRIX_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// One column of the capability matrix: what the terminal claims to be.
#[derive(Debug, Clone, Copy)]
struct TerminalProfile {
    name: &'static str,
    term: &'static str,
    /// Empty means "the variable is present but says nothing", which is how a
    /// terminal that does not advertise truecolor actually behaves. The
    /// harness always sets `COLORTERM`, so this is the honest way to model
    /// its absence.
    colorterm: &'static str,
    extra_env: &'static [(&'static str, &'static str)],
    /// Whether 24-bit SGR is permitted to reach the terminal on this profile.
    truecolor_allowed: bool,
}

const CAPABILITY_PROFILES: &[TerminalProfile] = &[
    TerminalProfile {
        name: "xterm-256color + COLORTERM=truecolor",
        term: "xterm-256color",
        colorterm: "truecolor",
        extra_env: &[],
        truecolor_allowed: true,
    },
    TerminalProfile {
        name: "xterm-256color, no truecolor claim",
        term: "xterm-256color",
        colorterm: "",
        extra_env: &[],
        truecolor_allowed: false,
    },
    TerminalProfile {
        name: "xterm (unknown tier)",
        term: "xterm",
        colorterm: "",
        extra_env: &[],
        truecolor_allowed: false,
    },
    TerminalProfile {
        name: "screen-256color (tmux-style)",
        term: "screen-256color",
        colorterm: "",
        extra_env: &[],
        truecolor_allowed: false,
    },
    TerminalProfile {
        name: "NO_COLOR present",
        term: "xterm-256color",
        colorterm: "truecolor",
        extra_env: &[("NO_COLOR", "1")],
        // NO_COLOR is not honored by the palette today. This row proves the
        // TUI still boots and paints with it set; it deliberately does not
        // claim color suppression, which is not a v0.9.2 contract.
        truecolor_allowed: true,
    },
    TerminalProfile {
        name: "CODEWHALE_ASCII_SAFE=1",
        term: "xterm-256color",
        colorterm: "truecolor",
        extra_env: &[("CODEWHALE_ASCII_SAFE", "1")],
        truecolor_allowed: true,
    },
];

/// Terminal geometries the release supports, from the smallest pane a user
/// realistically splits down to a full-screen 4K terminal.
const SIZE_MATRIX: &[(u16, u16, &str)] = &[
    (24, 80, "classic 80x24"),
    (40, 120, "laptop 120x40"),
    (20, 60, "narrow split 60x20"),
    (14, 48, "tiny pane 48x14"),
    (50, 200, "wide 200x50"),
];

/// Decorative glyphs the ASCII-safe tier promises to narrow. Each one is in
/// `crate::tui::glyphs::ascii_fallback`'s explicit table, so this asserts a
/// published mapping rather than a guessed Unicode range.
const DECORATIVE_GLYPHS: &[char] = &[
    '─', '│', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼', '╭', '╮', '╰', '╯', '█', '▌', '▐', '▶',
    '◀', '●', '○', '■', '◆', '…',
];

fn spawn(
    ws: &SealedWorkspace,
    profile: TerminalProfile,
    rows: u16,
    cols: u16,
    rust_log: &str,
) -> Result<Harness> {
    let mut builder = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        // A stub key skips onboarding; a refused loopback base URL guarantees
        // no request can escape the box. Nothing in this file needs a
        // provider — the rows that need a running turn live in
        // `release_runtime_qa.rs` against a loopback mock.
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", rust_log)
        .env("TERM", profile.term)
        .env("COLORTERM", profile.colorterm)
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--no-project-config",
            "--skip-onboarding",
        ])
        .size(rows, cols);
    for (key, value) in profile.extra_env {
        builder = builder.env(*key, *value);
    }
    builder.spawn()
}

fn default_profile() -> TerminalProfile {
    CAPABILITY_PROFILES[0]
}

fn boot(ws: &SealedWorkspace, rows: u16, cols: u16) -> Result<Harness> {
    let mut harness = spawn(ws, default_profile(), rows, cols, "warn")?;
    expect_text(&mut harness, COMPOSER_READY_TEXT, BOOT_TIMEOUT, "boot")?;
    Ok(harness)
}

/// Bounded wait for rendered text that fails with the frame *and* the
/// terminal-mode ledger, so a CI timeout is diagnosable without a rerun.
fn expect_text(
    harness: &mut Harness,
    needle: &str,
    timeout: Duration,
    context: &str,
) -> Result<()> {
    match harness.wait_for_text(needle, timeout) {
        Ok(()) => Ok(()),
        Err(err) => {
            let modes = harness.terminal_modes().debug_dump();
            Err(anyhow!("{context}: {err:#}\n{modes}"))
        }
    }
}

fn expect_frame<F>(
    harness: &mut Harness,
    predicate: F,
    timeout: Duration,
    context: &str,
) -> Result<()>
where
    F: FnMut(&Frame) -> bool,
{
    match harness.wait_for(predicate, timeout) {
        Ok(()) => Ok(()),
        Err(err) => {
            let modes = harness.terminal_modes().debug_dump();
            Err(anyhow!("{context}: {err:#}\n{modes}"))
        }
    }
}

/// Type a slash command and run it, waiting on the echo rather than sleeping.
fn run_command(harness: &mut Harness, command: &str) -> Result<()> {
    harness.send(keys::key::text(command))?;
    expect_text(harness, command, KEY_TIMEOUT, &format!("echo of {command}"))?;
    std::thread::sleep(PASTE_GUARD_SETTLE);
    harness.pump();
    harness.send(keys::key::enter())?;
    Ok(())
}

fn assert_no_leaked_modes(harness: &Harness, context: &str) {
    let ledger = harness.terminal_modes();
    let leaked = ledger.leaked_modes();
    assert!(
        leaked.is_empty(),
        "{context}: terminal modes left enabled after exit: {leaked:?}\n{}",
        ledger.debug_dump()
    );
    assert!(
        ledger.keyboard_pops() >= ledger.keyboard_pushes(),
        "{context}: keyboard enhancement stack not unwound ({} pushed, {} popped) — this is the \
         `^[[>5u` shell pollution from #1583\n{}",
        ledger.keyboard_pushes(),
        ledger.keyboard_pops(),
        ledger.debug_dump()
    );
    if ledger.was_ever_enabled(mode::ALT_SCREEN) {
        assert_eq!(
            ledger.state(mode::CURSOR_VISIBLE),
            Some(true),
            "{context}: cursor left hidden on the restored primary screen\n{}",
            ledger.debug_dump()
        );
    }
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Every supported terminal size keeps the composer reachable and paints
/// inside the viewport. Driven as live resizes on one process because that is
/// also the resize contract: a user dragging a pane must never be left with a
/// blank or overflowing frame.
#[test]
fn size_matrix_keeps_the_composer_visible_and_inside_the_viewport() -> Result<()> {
    let _guard = matrix_lock();
    let ws = make_sealed_workspace()?;
    let mut harness = boot(&ws, 40, 120)?;

    for (rows, cols, label) in SIZE_MATRIX {
        let transcript_before_resize = harness.transcript().len();
        harness.resize(*rows, *cols)?;
        let deadline = Instant::now() + qa_harness::harness::ci_scaled(KEY_TIMEOUT);
        loop {
            harness.pump();
            let saw_post_resize_output = harness.transcript().len() > transcript_before_resize;
            let frame = harness.frame();
            if saw_post_resize_output
                && frame.rows() == *rows
                && frame.cols() == *cols
                && frame.any_visible_text()
                && frame.contains(COMPOSER_READY_TEXT)
            {
                break;
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "painted composer after resize to {label}: timed out\n{}",
                    harness.diagnostics()
                ));
            }
            std::thread::sleep(Duration::from_millis(40));
        }

        let frame = harness.frame();
        let dump = frame.debug_dump();
        assert!(
            frame.any_visible_text(),
            "{label}: viewport went blank after resize:\n{dump}"
        );
        assert!(
            frame.max_row_width() <= usize::from(*cols),
            "{label}: a row overflowed {cols} columns:\n{dump}"
        );
        let (cursor_row, cursor_col) = frame.cursor();
        assert!(
            cursor_row < *rows && cursor_col < *cols,
            "{label}: cursor left the viewport at {cursor_row}x{cursor_col}:\n{dump}"
        );
    }

    // Typing must still land in the composer at the last (widest) size, so a
    // resize storm cannot silently detach input.
    harness.send(keys::key::text("post-resize input"))?;
    expect_text(
        &mut harness,
        "post-resize input",
        KEY_TIMEOUT,
        "composer input after the full resize sweep",
    )?;

    let _ = harness.shutdown();
    Ok(())
}

// ---------------------------------------------------------------------------
// Capability tiers
// ---------------------------------------------------------------------------

/// Boot under each `TERM`/`COLORTERM` tier and prove the palette honored it.
/// The assertion reads parsed ANSI out of the PTY, not the renderer's
/// intent — a terminal that only advertises 256 colors must never receive
/// `38;2` truecolor SGR (#2494 item 3).
#[test]
fn capability_matrix_honors_the_advertised_color_tier() -> Result<()> {
    let _guard = matrix_lock();

    for profile in CAPABILITY_PROFILES {
        let ws = make_sealed_workspace()?;
        let mut harness = spawn(&ws, *profile, 40, 120, "warn")?;
        expect_text(
            &mut harness,
            COMPOSER_READY_TEXT,
            BOOT_TIMEOUT,
            &format!("boot under {}", profile.name),
        )?;

        let frame = harness.frame();
        let dump = frame.debug_dump();
        assert!(
            frame.any_visible_text(),
            "{}: booted to a blank screen:\n{dump}",
            profile.name
        );
        if !profile.truecolor_allowed {
            assert!(
                !frame.any_truecolor_cell(),
                "{}: 24-bit SGR reached a terminal that never advertised it:\n{dump}",
                profile.name
            );
        }

        let _ = harness.shutdown();
    }

    Ok(())
}

/// `CODEWHALE_ASCII_SAFE=1` must narrow every CodeWhale-authored decorative
/// glyph, and the default tier must actually differ — otherwise the fallback
/// is untested and the "ASCII terminals are supported" claim is unbacked.
#[test]
fn ascii_safe_tier_removes_decorative_glyphs_the_default_tier_paints() -> Result<()> {
    let _guard = matrix_lock();

    let rich_ws = make_sealed_workspace()?;
    let mut rich = spawn(&rich_ws, default_profile(), 40, 120, "warn")?;
    expect_text(&mut rich, COMPOSER_READY_TEXT, BOOT_TIMEOUT, "rich boot")?;
    let rich_painted = rich.frame().painted_chars();
    let rich_dump = rich.debug_dump();
    let _ = rich.shutdown();

    let rich_decorative: Vec<char> = DECORATIVE_GLYPHS
        .iter()
        .copied()
        .filter(|glyph| rich_painted.contains(glyph))
        .collect();
    assert!(
        !rich_decorative.is_empty(),
        "default tier painted no decorative glyph, so the ASCII fallback below \
         would pass vacuously:\n{rich_dump}"
    );

    let ascii_profile = CAPABILITY_PROFILES
        .iter()
        .find(|profile| profile.name == "CODEWHALE_ASCII_SAFE=1")
        .copied()
        .expect("ascii-safe profile is declared in the capability matrix");
    let ascii_ws = make_sealed_workspace()?;
    let mut ascii = spawn(&ascii_ws, ascii_profile, 40, 120, "warn")?;
    expect_text(
        &mut ascii,
        COMPOSER_READY_TEXT,
        BOOT_TIMEOUT,
        "ascii-safe boot",
    )?;
    let ascii_painted = ascii.frame().painted_chars();
    let ascii_dump = ascii.debug_dump();
    let _ = ascii.shutdown();

    let leaked: Vec<char> = DECORATIVE_GLYPHS
        .iter()
        .copied()
        .filter(|glyph| ascii_painted.contains(glyph))
        .collect();
    assert!(
        leaked.is_empty(),
        "ASCII-safe tier still painted {leaked:?}:\n{ascii_dump}"
    );
    let braille: Vec<char> = ascii_painted
        .iter()
        .copied()
        .filter(|ch| ('\u{2800}'..='\u{28FF}').contains(ch))
        .collect();
    assert!(
        braille.is_empty(),
        "ASCII-safe tier still painted braille state markers {braille:?}:\n{ascii_dump}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Paste / IME
// ---------------------------------------------------------------------------

/// Whether the frame shows a turn that actually left the composer. Boot is
/// provider-free against a refused loopback port, so any dispatch surfaces as
/// a connection failure rather than a reply.
fn frame_shows_dispatched_turn(frame: &Frame) -> bool {
    frame.contains("Turn failed") || frame.contains("Connection refused") || frame.contains("error")
}

/// Every paste shape a real terminal can deliver — bracketed, raw, multiline,
/// CJK, and a very large payload — must land in the composer and must not
/// auto-submit. #1073 and the v0.9.2 real-PTY paste trace are both regressions
/// of exactly this cell.
#[test]
fn paste_matrix_lands_in_the_composer_without_autosubmitting() -> Result<()> {
    let _guard = matrix_lock();

    // Each case is (label, payload, bracketed). The marker each payload ends
    // with is what the assertion looks for, so a silently truncated paste
    // fails instead of passing on its prefix.
    let cases: &[(&str, String, bool)] = &[
        (
            "bracketed single line",
            "matrix-bracketed-single-END".to_string(),
            true,
        ),
        (
            "bracketed multiline",
            "matrix-line-one\nmatrix-line-two\nmatrix-bracketed-multi-END".to_string(),
            true,
        ),
        (
            "bracketed with trailing newline",
            "matrix-bracketed-trailing-END\n".to_string(),
            true,
        ),
        (
            "bracketed CJK + wide glyphs",
            "你好世界 マトリクス 매트릭스 matrix-cjk-END".to_string(),
            true,
        ),
        (
            "raw unbracketed multiline",
            "matrix-raw-one\nmatrix-raw-two\nmatrix-raw-END".to_string(),
            false,
        ),
        (
            "large bracketed payload",
            format!("{}matrix-large-END", "abcdefghij ".repeat(180)),
            true,
        ),
    ];

    for (label, payload, bracketed) in cases {
        let ws = make_sealed_workspace()?;
        let mut harness = boot(&ws, 40, 120)?;

        if *bracketed {
            harness.paste(payload)?;
        } else {
            harness.paste_unbracketed(payload)?;
        }

        let marker = payload
            .trim_end()
            .rsplit(['\n', ' '])
            .next()
            .expect("every payload ends with a marker token")
            .to_string();
        expect_text(
            &mut harness,
            &marker,
            KEY_TIMEOUT,
            &format!("{label}: pasted tail never reached the composer"),
        )?;

        // Give any trailing-newline-driven submit the chance to happen before
        // asserting that it did not. This window is the paste-burst guard
        // itself, not a synchronisation crutch.
        std::thread::sleep(PASTE_GUARD_SETTLE);
        harness.pump();
        let frame = harness.frame();
        let dump = frame.debug_dump();
        assert!(
            !frame_shows_dispatched_turn(frame),
            "{label}: paste auto-submitted; nothing may dispatch without an explicit Enter:\n{dump}"
        );

        let _ = harness.shutdown();
    }

    Ok(())
}

/// An IME delivers each committed character as its own event with human-scale
/// gaps. That is typing, not a paste: the Enter that follows a committed CJK
/// sentence must submit rather than being absorbed as a paste's trailing
/// newline. Covers the lone-commit short-window path from the v0.9.2 paste
/// trace as well as a multi-character commit.
#[test]
fn ime_style_commits_are_typing_and_the_following_enter_submits() -> Result<()> {
    let _guard = matrix_lock();
    let ws = make_sealed_workspace()?;
    let mut harness = boot(&ws, 40, 120)?;

    // Multi-character commit, one candidate at a time.
    for ch in "行列テスト".chars() {
        harness.send(keys::key::ch(ch))?;
        std::thread::sleep(Duration::from_millis(60));
    }
    expect_text(
        &mut harness,
        "行列テスト",
        KEY_TIMEOUT,
        "IME commits never echoed",
    )?;
    let frame = harness.frame();
    let dump = frame.debug_dump();
    assert!(
        !frame_shows_dispatched_turn(frame),
        "IME composition must not dispatch on its own:\n{dump}"
    );

    // A lone trailing commit followed by Enter is the exact shape that used to
    // re-arm the paste-burst window and swallow the send.
    harness.send(keys::key::ch('了'))?;
    std::thread::sleep(Duration::from_millis(50));
    harness.send(keys::key::enter())?;

    expect_frame(
        &mut harness,
        frame_shows_dispatched_turn,
        Duration::from_secs(15),
        "IME-typed message never submitted on Enter",
    )?;

    let _ = harness.shutdown();
    Ok(())
}

// ---------------------------------------------------------------------------
// Mouse / resize / focus
// ---------------------------------------------------------------------------

/// Mouse, resize and focus events must never be decoded as text, and
/// `FocusGained` must re-establish the terminal modes the emulator may have
/// dropped while the window was in the background.
#[test]
fn mouse_resize_and_focus_events_never_reach_the_composer_as_text() -> Result<()> {
    let _guard = matrix_lock();
    let ws = make_sealed_workspace()?;
    let mut harness = boot(&ws, 40, 120)?;

    harness.send(keys::key::text("focus-sentinel"))?;
    expect_text(
        &mut harness,
        "focus-sentinel",
        KEY_TIMEOUT,
        "sentinel draft",
    )?;

    harness.send(keys::mouse::click(10, 20))?;
    harness.send(keys::mouse::wheel_up(10, 20))?;
    harness.send(keys::mouse::wheel_down(10, 20))?;
    harness.send(keys::mouse::drag(12, 24))?;
    harness.send(keys::focus::lost())?;
    harness.resize(30, 100)?;
    harness.send(keys::focus::gained())?;
    harness.resize(40, 120)?;

    expect_text(
        &mut harness,
        "focus-sentinel",
        KEY_TIMEOUT,
        "draft after the mouse/focus/resize storm",
    )?;
    let frame = harness.frame();
    let dump = frame.debug_dump();
    for residue in ["[<0;", "[<64;", "[<65;", "[<32;", "\u{1b}[I", "\u{1b}[O"] {
        assert!(
            !frame.contains(residue),
            "control sequence {residue:?} was painted as text:\n{dump}"
        );
    }
    assert!(
        !frame_shows_dispatched_turn(frame),
        "a mouse or focus event dispatched a turn:\n{dump}"
    );

    // FocusGained runs `recover_terminal_modes`, so focus reporting and
    // bracketed paste must be *on* again while the process is still alive.
    let ledger = harness.terminal_modes();
    assert_eq!(
        ledger.state(mode::FOCUS),
        Some(true),
        "focus reporting was not re-established after FocusGained\n{}",
        ledger.debug_dump()
    );
    assert_eq!(
        ledger.state(mode::BRACKETED_PASTE),
        Some(true),
        "bracketed paste was not re-established after FocusGained\n{}",
        ledger.debug_dump()
    );

    let _ = harness.shutdown();
    Ok(())
}

// ---------------------------------------------------------------------------
// Modals
// ---------------------------------------------------------------------------

/// How a modal is opened. Chords are the ones the help overlay advertises;
/// commands are the ones the keybinding catalog documents as the guaranteed
/// path for terminals that cannot encode the chord.
enum Opener {
    Chord(&'static str, Vec<u8>),
    Command(&'static str),
}

impl Opener {
    fn label(&self) -> &'static str {
        match self {
            Self::Chord(label, _) => label,
            Self::Command(command) => command,
        }
    }
}

/// Every modal that can be opened deterministically without a provider must
/// push exactly one view, and Esc must pop exactly that view and return the
/// stack to empty. Evidence is the product's own `view_stack` trace records,
/// not "the frame looked different" — a replaced modal and a closed modal
/// render the same way.
#[test]
fn every_provider_free_modal_opens_and_escape_returns_to_the_composer() -> Result<()> {
    let _guard = matrix_lock();
    let ws = make_sealed_workspace()?;
    let mut harness = spawn(&ws, default_profile(), 44, 140, VIEW_STACK_RUST_LOG)?;
    expect_text(&mut harness, COMPOSER_READY_TEXT, BOOT_TIMEOUT, "boot")?;

    let openers = [
        (Opener::Chord("F1", keys::key::f1()), "Help"),
        (
            Opener::Chord("Ctrl+K", keys::key::ctrl('k')),
            "CommandPalette",
        ),
        (Opener::Command("/context"), "ContextInspector"),
        (Opener::Command("/transcript"), "LiveTranscript"),
        (Opener::Command("/theme"), "ThemePicker"),
        (Opener::Command("/skills"), "SkillsManager"),
    ];

    let mut transition_cursor = view_log::read_events(ws.home()).unwrap_or_default().len();
    for (opener, kind) in &openers {
        match opener {
            Opener::Chord(_, bytes) => harness.send(bytes.clone())?,
            Opener::Command(command) => run_command(&mut harness, command)?,
        }
        let (opened, last) =
            view_log::wait_for_event_after(ws.home(), transition_cursor, MODAL_TIMEOUT, |event| {
                event.is_open() && event.kind == *kind
            })
            .map_err(|err| anyhow!("{} did not open {kind}: {err:#}", opener.label()))?;
        transition_cursor = opened.len();
        assert_eq!(
            last.depth,
            1,
            "{} opened {kind} on top of a stack that was never unwound",
            opener.label()
        );

        harness.send(keys::key::esc())?;
        let (closed, last) =
            view_log::wait_for_event_after(ws.home(), transition_cursor, MODAL_TIMEOUT, |event| {
                event.is_close() && event.kind == *kind
            })
            .map_err(|err| anyhow!("Esc did not close {kind}: {err:#}"))?;
        transition_cursor = closed.len();
        assert_eq!(
            last.depth, 0,
            "Esc left the view stack at depth {} after closing {kind}",
            last.depth
        );

        expect_text(
            &mut harness,
            COMPOSER_READY_TEXT,
            MODAL_TIMEOUT,
            &format!("composer after closing {kind}"),
        )?;
    }

    // The composer must be typable again after the whole sweep — a modal that
    // closed but kept input focus is the failure this row exists to catch.
    harness.send(keys::key::text("post-modal input"))?;
    expect_text(
        &mut harness,
        "post-modal input",
        KEY_TIMEOUT,
        "composer input after the modal sweep",
    )?;

    let _ = harness.shutdown();
    Ok(())
}

/// The help overlay is the only place the product advertises chords, and it
/// renders straight from `KEYBINDINGS`. Scraping it keeps this suite from
/// growing a second shortcut table that can drift: if a chord stops being
/// advertised, this row fails rather than silently testing a stale contract.
#[test]
fn help_overlay_advertises_the_chords_this_matrix_drives() -> Result<()> {
    let _guard = matrix_lock();
    let ws = make_sealed_workspace()?;
    let mut harness = boot(&ws, 44, 140)?;

    harness.send(keys::key::f1())?;
    expect_frame(
        &mut harness,
        |frame| frame.contains("Ctrl+K") || frame.contains("F1"),
        MODAL_TIMEOUT,
        "help overlay never opened",
    )?;

    // The overlay lists more rows than fit on any terminal, so each chord is
    // brought into view through the overlay's own substring filter instead of
    // being asserted against whatever happened to be scrolled into frame.
    // Chords this file drives, plus the command fallbacks the catalog
    // documents as guaranteed. macOS renders Alt chords with `⌥`, so only
    // spellings that are stable on every platform are asserted here.
    for advertised in ["F1", "Ctrl+K", "Enter", "/context", "/transcript"] {
        harness.send(keys::key::text(advertised))?;
        expect_text(
            &mut harness,
            advertised,
            KEY_TIMEOUT,
            &format!("help overlay no longer advertises {advertised}, but this matrix drives it"),
        )?;
        harness.send(keys::key::backspaces(advertised.chars().count()))?;
        expect_frame(
            &mut harness,
            |frame| frame.contains("Ctrl+C") && frame.contains("Esc"),
            KEY_TIMEOUT,
            "help filter never cleared",
        )?;
    }

    // #440 / #3758: Ctrl+G and Ctrl+S stash a draft, and nothing else. If
    // either ever appears beside send/queue/steer copy the advertised action
    // is ambiguous and the running-turn contract stops being teachable.
    for stash_chord in ["Ctrl+G", "Ctrl+S"] {
        harness.send(keys::key::text(stash_chord))?;
        expect_text(
            &mut harness,
            stash_chord,
            KEY_TIMEOUT,
            &format!("{stash_chord} is not advertised at all"),
        )?;
        let frame = harness.frame();
        let text = frame.text();
        for line in text.lines() {
            if !line.contains(stash_chord) {
                continue;
            }
            let lowered = line.to_ascii_lowercase();
            for forbidden in ["send", "queue", "steer", "submit"] {
                assert!(
                    !lowered.contains(forbidden),
                    "{stash_chord} must advertise exactly one action (stash), got {line:?}"
                );
            }
        }
        harness.send(keys::key::backspaces(stash_chord.chars().count()))?;
    }

    harness.send(keys::key::esc())?;
    expect_text(
        &mut harness,
        COMPOSER_READY_TEXT,
        MODAL_TIMEOUT,
        "composer after closing help",
    )?;

    let _ = harness.shutdown();
    Ok(())
}

// ---------------------------------------------------------------------------
// Terminal-mode restoration
// ---------------------------------------------------------------------------

/// Every exit path must hand the terminal back the way it found it. Checked
/// from the raw control stream, because a leaked alternate screen or a leaked
/// kitty keyboard flag is invisible on the rendered frame and only shows up in
/// the user's shell afterwards (#1583, #2494).
#[test]
fn terminal_modes_are_restored_on_every_exit_path() -> Result<()> {
    let _guard = matrix_lock();

    // Ctrl+D on an empty composer: the cooperative exit.
    {
        let ws = make_sealed_workspace()?;
        let mut harness = boot(&ws, 40, 120)?;
        harness.send(keys::key::ctrl_d())?;
        let status = harness.wait_for_exit(EXIT_TIMEOUT);
        harness.pump();
        assert_eq!(
            status,
            Some(0),
            "Ctrl+D on an empty composer must exit cleanly\n{}",
            harness.diagnostics()
        );
        assert_no_leaked_modes(&harness, "Ctrl+D exit");
    }

    // SIGINT: the signal handler's emergency restore path.
    {
        let ws = make_sealed_workspace()?;
        let mut harness = boot(&ws, 40, 120)?;
        let pid = harness.pid().ok_or_else(|| anyhow!("no child pid"))?;
        let signalled = std::process::Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .status()?;
        assert!(signalled.success(), "could not signal the TUI child");
        harness.wait_for_exit(EXIT_TIMEOUT);
        harness.pump();
        assert_no_leaked_modes(&harness, "SIGINT exit");
    }

    // A terminal that was never given the alternate screen still must not be
    // left with mouse capture or bracketed paste on.
    {
        let ws = make_sealed_workspace()?;
        let mut harness = spawn(&ws, CAPABILITY_PROFILES[2], 24, 80, "warn")?;
        expect_text(
            &mut harness,
            COMPOSER_READY_TEXT,
            BOOT_TIMEOUT,
            "boot on the unknown-tier terminal",
        )?;
        harness.send(keys::key::ctrl_d())?;
        harness.wait_for_exit(EXIT_TIMEOUT);
        harness.pump();
        assert_no_leaked_modes(&harness, "unknown-tier Ctrl+D exit");
    }

    Ok(())
}

/// Guard on the guard: `MODES_THAT_MUST_NOT_LEAK` has to actually cover the
/// modes the TUI turns on, or `assert_no_leaked_modes` passes vacuously.
#[test]
fn the_leak_guard_covers_the_modes_the_tui_enables() -> Result<()> {
    let _guard = matrix_lock();
    let ws = make_sealed_workspace()?;
    let mut harness = boot(&ws, 40, 120)?;
    // Mouse capture is established at startup; touch the input path so the
    // sample is taken after the TUI has fully settled its modes.
    harness.send(keys::focus::gained())?;
    expect_text(
        &mut harness,
        COMPOSER_READY_TEXT,
        KEY_TIMEOUT,
        "composer before sampling modes",
    )?;

    let ledger = harness.terminal_modes();
    let covered: Vec<u16> = MODES_THAT_MUST_NOT_LEAK
        .iter()
        .map(|(number, _)| *number)
        .filter(|number| ledger.was_ever_enabled(*number))
        .collect();
    assert!(
        covered.contains(&mode::ALT_SCREEN)
            && covered.contains(&mode::BRACKETED_PASTE)
            && covered.contains(&mode::FOCUS),
        "the running TUI did not enable the modes the exit guard checks; \
         covered={covered:?}\n{}",
        ledger.debug_dump()
    );

    let _ = harness.shutdown();
    Ok(())
}
