//! Illuminated "hot tail" for active prose streaming.
//!
//! Renders the newest ~8–16 graphemes brighter (and optionally bold) while a
//! response is live. Settled text uses the normal palette. Boundaries are
//! tracked on grapheme clusters, never byte indices.
//!
//! Activate only while responding; resolve the entire block to settled styling
//! when the stream completes. Reduced motion freezes the breathing luminance
//! cycle but keeps the static hot highlight.

use ratatui::style::{Color, Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;

use crate::tui::ocean;

/// Default hot-tail length in graphemes.
pub const HOT_TAIL_GRAPHEMES: usize = 12;

/// Split `text` into (settled, hot) grapheme spans for streaming render.
///
/// Returns `(settled, hot)` where `hot` is the trailing up-to-`n` graphemes
/// when `active` is true; otherwise the whole string is settled.
#[must_use]
pub fn split_hot_tail(text: &str, active: bool, n: usize) -> (&str, &str) {
    if !active || n == 0 || text.is_empty() {
        return (text, "");
    }
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    if graphemes.len() <= n {
        return ("", text);
    }
    let split_at = graphemes.len() - n;
    let mut byte = 0usize;
    for g in graphemes.iter().take(split_at) {
        byte += g.len();
    }
    (&text[..byte], &text[byte..])
}

/// Breathing luminance factor on a slow ~1.5 s cycle. Fixed under reduced motion.
#[must_use]
pub fn breath_luminance(elapsed_ms: u128, reduced_motion: bool) -> f32 {
    if reduced_motion {
        return 1.12;
    }
    let period = 1_500u128;
    let phase = (elapsed_ms % period) as f32 / period as f32;
    let s = (phase * std::f32::consts::TAU).sin();
    // 1.05 … 1.22
    1.135 + s * 0.085
}

/// Style for the hot tail of an active streaming block.
#[must_use]
pub fn hot_tail_style(base_fg: Color, elapsed_ms: u128, reduced_motion: bool) -> Style {
    let scale = breath_luminance(elapsed_ms, reduced_motion);
    let fg = ocean::scale_color(base_fg, scale);
    Style::default().fg(fg).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_hot_tail_on_grapheme_boundary() {
        let text = "hello 世界!";
        let (settled, hot) = split_hot_tail(text, true, 3);
        assert_eq!(hot.graphemes(true).count(), 3);
        assert_eq!(format!("{settled}{hot}"), text);
    }

    #[test]
    fn inactive_stream_has_no_hot_tail() {
        let (settled, hot) = split_hot_tail("abcdef", false, 12);
        assert_eq!(settled, "abcdef");
        assert!(hot.is_empty());
    }
}
