//! OSC 8 hyperlink emission and stripping.
//!
//! Modern terminals (iTerm2, Terminal.app 13+, Ghostty, Kitty, WezTerm,
//! Alacritty, recent gnome-terminal/konsole) make a substring clickable when
//! it is wrapped in:
//!
//! ```text
//! \x1b]8;;TARGET\x1b\\LABEL\x1b]8;;\x1b\\
//! ```
//!
//! Terminals that don't understand the sequence simply render the visible
//! `LABEL` and ignore the escape. So emitting OSC 8 is a strict UX upgrade for
//! supporting terminals and a no-op for the rest.
//!
//! The TUI emits these inside `Span::content` strings so the existing
//! ratatui pipeline carries them through. The tradeoff is that the clipboard
//! / selection extraction path must strip the codes before handing text to the
//! user — that's what [`strip_into`] is for.

use std::sync::atomic::{AtomicBool, Ordering};

const OSC8_PREFIX: &str = "\x1b]8;;";
const OSC8_TERMINATOR: &str = "\x1b\\";
const OSC8_CLOSE: &str = "\x1b]8;;\x1b\\";

/// A contiguous run of cells on one terminal row that share a hyperlink target.
#[derive(Debug, Clone)]
pub struct LinkRegion {
    pub row: u16,
    pub col_start: u16,
    pub col_end: u16,
    pub target: String,
}

/// Write an OSC 8 hyperlink open sequence for `target` to `w`.
pub fn write_osc8_open(w: &mut impl std::io::Write, target: &str) -> std::io::Result<()> {
    w.write_all(OSC8_PREFIX.as_bytes())?;
    w.write_all(target.as_bytes())?;
    w.write_all(OSC8_TERMINATOR.as_bytes())
}

/// Write an OSC 8 hyperlink close sequence to `w`.
pub fn write_osc8_close(w: &mut impl std::io::Write) -> std::io::Result<()> {
    w.write_all(OSC8_CLOSE.as_bytes())
}

/// Process-wide enable flag. `true` by default. Set once at app init from
/// `[ui] osc8_links` (when present) and read by the renderer.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Set the process-wide OSC 8 enable flag. Intended to be called once at
/// startup; subsequent calls take effect immediately.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether OSC 8 hyperlink emission is currently enabled.
#[must_use]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

// --- Thread-local link region accumulator (#3029) ---

use std::cell::RefCell;

thread_local! {
    /// Link regions collected during the current render frame.
    /// Populated by the render closure after scanning the ratatui buffer;
    /// consumed and cleared by `ColorCompatBackend::draw()`.
    pub static FRAME_LINKS: RefCell<Vec<LinkRegion>> = const { RefCell::new(Vec::new()) };
}

/// Replace the thread-local frame link buffer with `links`.
#[allow(dead_code)] // called from render closure (future integration)
pub fn set_frame_links(links: Vec<LinkRegion>) {
    FRAME_LINKS.with(|cell| {
        *cell.borrow_mut() = links;
    });
}

/// Take the thread-local frame links, leaving an empty vec behind.
pub fn take_frame_links() -> Vec<LinkRegion> {
    FRAME_LINKS.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

/// Wrap `label` so it links to `target` in OSC 8-aware terminals. The returned
/// string contains the full `\x1b]8;;TARGET\x1b\LABEL\x1b]8;;\x1b\` payload.
///
/// Does **not** check [`enabled()`]; callers wanting the runtime gate should
/// branch on it before calling this. That keeps the helper test-friendly.
#[must_use]
pub fn wrap_link(target: &str, label: &str) -> String {
    let mut out = String::with_capacity(target.len() + label.len() + 12);
    out.push_str(OSC8_PREFIX);
    out.push_str(target);
    out.push_str(OSC8_TERMINATOR);
    out.push_str(label);
    out.push_str(OSC8_PREFIX);
    out.push_str(OSC8_TERMINATOR);
    out
}

/// Strip every ANSI escape sequence from `s` into `out`, preserving only the
/// visible characters. ratatui's buffer drops the leading `ESC` byte but
/// happily paints every other byte of an escape (`[`, `0`, `;`, `m`, OSC
/// payloads, etc.) into a buffer cell, drifting columns. Tool stdout that
/// includes ANSI (e.g. `gh`/`git` with color forced on, anything run through
/// a PTY) must be sanitized before it enters the transcript.
///
/// Handles CSI (`ESC [ … final`), OSC (`ESC ] … BEL` or `ESC \`), DCS, SOS,
/// PM, APC, and standalone two-byte ESC sequences. OSC 8 hyperlink wrappers
/// (`ESC ] 8 ; … BEL` / `ESC \`) are stripped along with the rest.
pub fn strip_ansi_into(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            match next {
                // CSI: ESC [ ... <final byte 0x40..=0x7E>
                b'[' => {
                    let mut j = i + 2;
                    while j < bytes.len() {
                        let b = bytes[j];
                        if (0x40..=0x7e).contains(&b) {
                            j += 1;
                            break;
                        }
                        j += 1;
                    }
                    i = j;
                    continue;
                }
                // OSC / DCS / SOS / PM / APC: ESC ] | P | X | ^ | _ ... ST(ESC \) or BEL
                b']' | b'P' | b'X' | b'^' | b'_' => {
                    let mut j = i + 2;
                    while j < bytes.len() {
                        if bytes[j] == 0x07 {
                            j += 1;
                            break;
                        }
                        if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                            j += 2;
                            break;
                        }
                        j += 1;
                    }
                    i = j;
                    continue;
                }
                // Standalone two-byte ESC sequence (RIS, charset selection, etc.)
                _ => {
                    i += 2;
                    continue;
                }
            }
        }
        // Strip lone control bytes that ratatui would otherwise drop (and which
        // mean nothing in transcript output) but keep \n, \r, \t as legitimate
        // formatting.
        let b = bytes[i];
        if b < 0x80 {
            if b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t' {
                i += 1;
                continue;
            }
            out.push(b as char);
            i += 1;
        } else {
            // UTF-8 multi-byte sequence: copy the whole code point intact.
            // Pushing `b as char` would mis-decode it as Latin-1 and mangle
            // non-ASCII text (CJK, accented Latin, emoji, …).
            let len = utf8_seq_len(b);
            let end = (i + len).min(bytes.len());
            if let Ok(chunk) = std::str::from_utf8(&bytes[i..end]) {
                out.push_str(chunk);
            }
            i = end;
        }
    }
}

/// Length in bytes of the UTF-8 sequence that starts with `lead`. Falls back
/// to `1` for continuation bytes / invalid leads so callers always make
/// forward progress.
fn utf8_seq_len(lead: u8) -> usize {
    if lead < 0xc0 {
        1
    } else if lead < 0xe0 {
        2
    } else if lead < 0xf0 {
        3
    } else {
        4
    }
}

/// Strip OSC 8 escape sequences from `s` into `out`, preserving the visible
/// label text. Other escapes (color, style) pass through untouched. The
/// implementation handles both the standard `ESC \` and the lone `BEL`
/// terminators that some emitters use.
pub fn strip_into(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for the OSC 8 prefix `ESC ] 8 ;`
        if i + 4 <= bytes.len()
            && bytes[i] == 0x1b
            && bytes[i + 1] == b']'
            && bytes[i + 2] == b'8'
            && bytes[i + 3] == b';'
        {
            // Skip until the string terminator (ESC \) or BEL.
            let mut j = i + 4;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    j += 1;
                    break;
                }
                if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                    j += 2;
                    break;
                }
                j += 1;
            }
            i = j;
            continue;
        }
        let b = bytes[i];
        if b < 0x80 {
            out.push(b as char);
            i += 1;
        } else {
            let len = utf8_seq_len(b);
            let end = (i + len).min(bytes.len());
            if let Ok(chunk) = std::str::from_utf8(&bytes[i..end]) {
                out.push_str(chunk);
            }
            i = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize tests that read or write the `ENABLED` flag so they don't
    /// race each other under cargo's default parallel test runner.
    static FLAG_GUARD: Mutex<()> = Mutex::new(());

    fn strip(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        strip_into(s, &mut out);
        out
    }

    #[test]
    fn wrap_link_shape_is_osc_8_compliant() {
        let wrapped = wrap_link("https://example.com", "click me");
        assert_eq!(
            wrapped,
            "\x1b]8;;https://example.com\x1b\\click me\x1b]8;;\x1b\\"
        );
    }

    #[test]
    fn strip_removes_wrapper_keeps_label() {
        let wrapped = wrap_link("https://example.com", "click me");
        assert_eq!(strip(&wrapped), "click me");
    }

    #[test]
    fn strip_handles_bel_terminator() {
        let wrapped = "\x1b]8;;https://example.com\x07click me\x1b]8;;\x07";
        assert_eq!(strip(wrapped), "click me");
    }

    #[test]
    fn strip_passes_through_text_with_no_escapes() {
        let plain = "no escapes here";
        assert_eq!(strip(plain), plain);
    }

    #[test]
    fn strip_preserves_non_osc_8_escapes() {
        // Color escape stays in place; only OSC 8 wrappers are removed.
        let mixed = format!(
            "\x1b[31mred\x1b[0m {wrapped}",
            wrapped = wrap_link("https://example.com", "click")
        );
        assert_eq!(strip(&mixed), "\x1b[31mred\x1b[0m click");
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        strip_ansi_into(s, &mut out);
        out
    }

    #[test]
    fn strip_ansi_removes_csi_sgr_and_keeps_text() {
        let coloured = "526   \x1b[1;32mOPEN\x1b[0m  bug fix";
        assert_eq!(strip_ansi(coloured), "526   OPEN  bug fix");
    }

    #[test]
    fn strip_ansi_removes_osc_8_wrapper() {
        let wrapped = wrap_link("https://example.com", "click");
        assert_eq!(strip_ansi(&wrapped), "click");
    }

    #[test]
    fn strip_ansi_preserves_newlines_tabs_and_cr() {
        let s = "a\nb\tc\rd";
        assert_eq!(strip_ansi(s), "a\nb\tc\rd");
    }

    #[test]
    fn strip_ansi_drops_lone_control_bytes() {
        // Bare BEL or other C0 control bytes that aren't \n/\r/\t are dropped
        // so they can't paint as visible cells.
        let s = "a\x07b\x01c";
        assert_eq!(strip_ansi(s), "abc");
    }

    #[test]
    fn strip_ansi_preserves_utf8_multibyte_chars() {
        // CJK, accented Latin, and emoji must survive the strip without being
        // re-decoded as Latin-1 (which would explode 你 -> ä½ ).
        let s = "Phase 1: 第一步 README é 🚀";
        assert_eq!(strip_ansi(s), "Phase 1: 第一步 README é 🚀");

        let coloured = "\x1b[1;32m第一步\x1b[0m done";
        assert_eq!(strip_ansi(coloured), "第一步 done");
    }

    #[test]
    fn strip_preserves_utf8_multibyte_chars() {
        let wrapped = wrap_link("https://example.com", "点击我");
        assert_eq!(strip(&wrapped), "点击我");
    }

    #[test]
    fn enabled_is_true_by_default_when_untouched() {
        // Hold the flag guard so we observe the initial state, not a value
        // mid-flight from `set_enabled_round_trips`. The flag *defaults* to
        // true at static init and tests in this module are the only writers.
        let _g = FLAG_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        assert!(enabled());
    }

    #[test]
    fn set_enabled_round_trips() {
        let _g = FLAG_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prior = enabled();
        set_enabled(false);
        assert!(!enabled());
        set_enabled(true);
        assert!(enabled());
        set_enabled(prior);
    }
}
