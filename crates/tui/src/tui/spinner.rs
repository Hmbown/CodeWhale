//! Shared animation frames for running-state UI chrome.
//!
//! Keep the braille spinner in one place so transcript tool cards, sidebars,
//! and any future running-job surfaces advance with the same cadence.
//!
//! Motion *policy* (whether to animate at all) lives in
//! [`crate::tui::motion::MotionPolicy`]. Callers that already have a policy
//! should prefer [`crate::tui::motion::MotionPolicy::spinner_glyph`]; the
//! helpers here remain the shared frame table + elapsed-time index.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Braille bubble frames used for running tools and background jobs. Dots fill
/// upward, then release. Eight distinct states at eight hertz keep the motion
/// continuous without turning the one-cell marker into a high-frequency
/// spinner.
pub(crate) const BRAILLE_SPINNER_FRAMES: [&str; 8] = ["⠀", "⢀", "⣀", "⣄", "⣤", "⣦", "⣶", "⣿"];
pub(crate) const VERIFY_TICK_FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// A motion marker is earned only after work survives the eye's quick-event
/// window. Faster work should simply land as a receipt.
pub(crate) const LIVE_MARKER_DELAY_MS: u64 = 400;
pub(crate) const LIVE_STATIC_MARKER: &str = "›";
pub(crate) const BRAILLE_SPINNER_STILL_FRAME: &str = "⣤";

/// Eight stepped states per second. This is deliberately slower than the
/// underwater field's ~12fps caustic cadence: the marker communicates active
/// work, while the field stays subordinate atmosphere.
pub(crate) const BRAILLE_SPINNER_FRAME_MS: u64 = 125;

#[must_use]
pub(crate) fn braille_spinner_frame_for_elapsed_ms(
    elapsed_ms: u128,
    low_motion: bool,
) -> &'static str {
    if low_motion {
        return BRAILLE_SPINNER_STILL_FRAME;
    }
    if elapsed_ms < u128::from(LIVE_MARKER_DELAY_MS) {
        return LIVE_STATIC_MARKER;
    }
    let idx = elapsed_ms
        .saturating_sub(u128::from(LIVE_MARKER_DELAY_MS))
        .checked_div(u128::from(BRAILLE_SPINNER_FRAME_MS))
        .map_or(0, |frame| frame % BRAILLE_SPINNER_FRAMES.len() as u128);
    BRAILLE_SPINNER_FRAMES[usize::try_from(idx).unwrap_or_default()]
}

#[must_use]
pub(crate) fn braille_spinner_frame_for_duration_ms(
    duration_ms: u64,
    low_motion: bool,
) -> &'static str {
    braille_spinner_frame_for_elapsed_ms(u128::from(duration_ms), low_motion)
}

#[must_use]
pub(crate) fn braille_spinner_frame(started_at: Option<Instant>, low_motion: bool) -> &'static str {
    braille_spinner_frame_for_elapsed_ms(marker_elapsed_ms(started_at), low_motion)
}

#[must_use]
pub(crate) fn verification_tick_frame(
    started_at: Option<Instant>,
    low_motion: bool,
) -> &'static str {
    if low_motion {
        return VERIFY_TICK_FRAMES[4];
    }
    let elapsed_ms = marker_elapsed_ms(started_at);
    if elapsed_ms < u128::from(LIVE_MARKER_DELAY_MS) {
        return LIVE_STATIC_MARKER;
    }
    let idx = elapsed_ms
        .saturating_sub(u128::from(LIVE_MARKER_DELAY_MS))
        .checked_div(u128::from(BRAILLE_SPINNER_FRAME_MS))
        .map_or(0, |frame| frame % VERIFY_TICK_FRAMES.len() as u128);
    VERIFY_TICK_FRAMES[usize::try_from(idx).unwrap_or_default()]
}

fn marker_elapsed_ms(started_at: Option<Instant>) -> u128 {
    started_at.map_or_else(
        || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis())
        },
        |started| started.elapsed().as_millis(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braille_spinner_advances_at_shared_cadence() {
        // Assert cadence behavior against the frame table rather than specific
        // glyphs so the whale-spout pattern can be retuned without churn here.
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(0, false),
            LIVE_STATIC_MARKER
        );
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(u128::from(LIVE_MARKER_DELAY_MS) - 1, false),
            LIVE_STATIC_MARKER
        );
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(u128::from(LIVE_MARKER_DELAY_MS), false),
            BRAILLE_SPINNER_FRAMES[0]
        );
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(
                u128::from(LIVE_MARKER_DELAY_MS + BRAILLE_SPINNER_FRAME_MS),
                false,
            ),
            BRAILLE_SPINNER_FRAMES[1]
        );
    }

    #[test]
    fn active_marker_uses_a_stable_eight_hertz_wall_clock() {
        assert_eq!(BRAILLE_SPINNER_FRAME_MS, 125);
        for (index, frame) in BRAILLE_SPINNER_FRAMES.iter().enumerate() {
            assert_eq!(
                braille_spinner_frame_for_elapsed_ms(
                    u128::from(LIVE_MARKER_DELAY_MS)
                        + u128::from(BRAILLE_SPINNER_FRAME_MS) * index as u128,
                    false,
                ),
                *frame
            );
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(*frame),
                1,
                "active marker frames must never shift adjacent text"
            );
        }
    }

    #[test]
    fn braille_spinner_respects_low_motion() {
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(u128::from(BRAILLE_SPINNER_FRAME_MS) * 3, true),
            BRAILLE_SPINNER_STILL_FRAME
        );
    }

    #[test]
    fn verification_tick_is_distinct_and_freezes_legibly() {
        let start = Instant::now() - std::time::Duration::from_millis(LIVE_MARKER_DELAY_MS);
        assert_eq!(
            verification_tick_frame(Some(start), false),
            VERIFY_TICK_FRAMES[0]
        );
        assert_eq!(
            verification_tick_frame(Some(start), true),
            VERIFY_TICK_FRAMES[4]
        );
        assert_ne!(VERIFY_TICK_FRAMES[0], BRAILLE_SPINNER_FRAMES[0]);
    }
}
