//! Shared animation frames for running-state UI chrome.
//!
//! Keep the braille spinner in one place so transcript tool cards, sidebars,
//! and any future running-job surfaces advance with the same cadence.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Braille "whale spout" frames used for running tools and background jobs.
///
/// The cycle climbs, crests, and falls instead of using the stock clock-wise
/// dots. At the shared repaint cadence it reads as a continuous spray plume.
pub(crate) const BRAILLE_SPINNER_FRAMES: [&str; 12] = [
    "\u{2840}", "\u{2844}", "\u{2846}", "\u{28C6}", "\u{28E6}", "\u{28F6}", "\u{28F2}", "\u{28B2}",
    "\u{2832}", "\u{2830}", "\u{2820}", "\u{2810}",
];

/// Match the product cadence clock so running glyphs advance with settle/surface.
///
/// Source of truth: [`crate::tui::motion::CADENCE_FRAME_MS`] (50ms × 48 ≈ 2.4s).
pub(crate) const BRAILLE_SPINNER_FRAME_MS: u64 = crate::tui::motion::CADENCE_FRAME_MS;

#[must_use]
pub(crate) fn braille_spinner_frame_for_elapsed_ms(
    elapsed_ms: u128,
    low_motion: bool,
) -> &'static str {
    if low_motion {
        return BRAILLE_SPINNER_FRAMES[0];
    }
    let idx = elapsed_ms
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
    let elapsed_ms = started_at.map_or_else(
        || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis())
        },
        |started| started.elapsed().as_millis(),
    );
    braille_spinner_frame_for_elapsed_ms(elapsed_ms, low_motion)
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
            BRAILLE_SPINNER_FRAMES[0]
        );
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(u128::from(BRAILLE_SPINNER_FRAME_MS) - 1, false),
            BRAILLE_SPINNER_FRAMES[0]
        );
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(u128::from(BRAILLE_SPINNER_FRAME_MS), false),
            BRAILLE_SPINNER_FRAMES[1]
        );
    }

    #[test]
    fn braille_spinner_respects_low_motion() {
        assert_eq!(
            braille_spinner_frame_for_elapsed_ms(u128::from(BRAILLE_SPINNER_FRAME_MS) * 3, true),
            BRAILLE_SPINNER_FRAMES[0]
        );
    }
}
