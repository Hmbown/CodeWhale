//! Status-indicator frame resolution for the header chip cluster.
//!
//! The classic-shell `HeaderWidget` was removed with the classic shell in
//! 0.9.4 (rail unification); what remains here is the frame picker shared
//! by the underwater header.

use std::time::Instant;

/// Milliseconds between status-indicator frame advances. The original
/// `deepseek_squiggle` (v0.3.5 → v0.8.x) used 420 ms; the dot replacement
/// used the same cadence. Keep both at 420 ms so the visual rhythm matches
/// what long-time users remember.
#[allow(dead_code)] // the echolocation animation family (spec §5e, 420 ms cadence) reuses
// this picker when its landing slice animates the merged-footer chip;
// the shell renders the still frame until then (2026-08-29).
const STATUS_INDICATOR_FRAME_MS: u128 = 420;

/// Geometric replacement frames shipped between v0.8.x and v0.8.29.
/// Every frame is one cell wide so the provider/model label never shifts
/// while the animation advances.
#[allow(dead_code)] // the echolocation animation family (spec §5e, 420 ms cadence) reuses
// this picker when its landing slice animates the merged-footer chip;
// the shell renders the still frame until then (2026-08-29).
const STATUS_INDICATOR_DOT_FRAMES: &[&str] = &["◍", "◉", "◌", "◌", "◉", "◍"];

/// Resolve the current status-indicator frame to render in the header
/// chip cluster.
///
/// `turn_started_at = None` (no active turn) returns the first frame so the
/// chip is *visible* but not animating — it's a chip, not a spinner. As
/// soon as a turn starts, the elapsed time keys the cycle.
///
/// `mode` accepts the canonical names `"cw"`, `"dots"`, `"off"`. The default
/// is the plain `Codewhale` wordmark: identity is stated once in the chrome,
/// while live state uses the existing semantic rows and receipts. Legacy
/// `"whale"` values (still present in persisted settings) normalize to that
/// wordmark; unknown values do too. `"off"` returns `None` so the caller can
/// hide the chip outright.
#[must_use]
#[allow(dead_code)] // the echolocation animation family (spec §5e, 420 ms cadence) reuses
// this picker when its landing slice animates the merged-footer chip;
// the shell renders the still frame until then (2026-08-29).
pub fn header_status_indicator_frame(
    turn_started_at: Option<Instant>,
    mode: &str,
) -> Option<&'static str> {
    let frames: &[&str] = match mode.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "hidden" | "false" => return None,
        "dots" | "dot" => STATUS_INDICATOR_DOT_FRAMES,
        // Canonical mark, legacy whale opt-ins, and unknown values all land
        // on the static wordmark so the header never reintroduces an emoji
        // chip beside the operational chrome.
        _ => return Some("codewhale"),
    };
    let elapsed_ms = turn_started_at
        .map(|t| t.elapsed().as_millis())
        .unwrap_or(0);
    let idx = (elapsed_ms / STATUS_INDICATOR_FRAME_MS) as usize % frames.len();
    Some(frames[idx])
}

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_whale_indicator_settings_normalize_to_the_wordmark() {
        // The whale emoji chip is retired from the header (2026-07-23):
        // persisted `status_indicator = "whale"` opt-ins render the plain
        // wordmark instead, idle or mid-turn.
        for legacy in ["whale", "🐳", "🐋"] {
            assert_eq!(
                super::header_status_indicator_frame(None, legacy),
                Some("codewhale"),
                "legacy mode {legacy:?} must normalize to the cw mark"
            );
            assert_eq!(
                super::header_status_indicator_frame(Some(std::time::Instant::now()), legacy),
                Some("codewhale"),
                "legacy mode {legacy:?} must stay static mid-turn"
            );
        }
    }

    #[test]
    fn cw_indicator_is_static_wordmark() {
        assert_eq!(
            super::header_status_indicator_frame(None, "cw"),
            Some("codewhale")
        );
        assert_eq!(
            super::header_status_indicator_frame(Some(std::time::Instant::now()), "cw"),
            Some("codewhale")
        );
    }

    #[test]
    fn dots_indicator_uses_geometric_frames() {
        let frame = super::header_status_indicator_frame(None, "dots");
        assert_eq!(frame, Some("\u{25CD}"));
    }

    #[test]
    fn off_indicator_returns_none_so_chip_is_hidden() {
        assert!(super::header_status_indicator_frame(None, "off").is_none());
        // Aliases mirror the parser in Settings.
        assert!(super::header_status_indicator_frame(None, "none").is_none());
        assert!(super::header_status_indicator_frame(None, "hidden").is_none());
        assert!(super::header_status_indicator_frame(None, "false").is_none());
    }

    #[test]
    fn unknown_indicator_mode_defaults_to_cw() {
        let frame = super::header_status_indicator_frame(None, "wahel-typo");
        assert_eq!(frame, Some("codewhale"));
    }

    #[test]
    fn whale_glyphs_have_narrow_ascii_fallbacks() {
        assert_eq!(crate::tui::glyphs::ascii_fallback("🐳"), Some("w"));
        assert_eq!(crate::tui::glyphs::ascii_fallback("🐋"), Some("w"));
    }
}
