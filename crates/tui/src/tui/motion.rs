//! Ocean motion grammar — receipt settle + completion surface.
//!
//! Pure timing helpers ported from `tui-fix/cw-underwater-take.html`:
//! - Receipt settle: ~400ms ease, ~60ms burst stagger, gated on `!low_motion`
//! - Completion surface: one-shot ~800ms field lighten on working→done,
//!   gated on `!low_motion && fancy_animations`
//!
//! Cadence matches the braille spinner (50ms frames, 2.4s cycle). This module
//! is intentionally free of App/History coupling so unit tests can drive the
//! real math without a live TUI.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Shared frame cadence with [`super::spinner::BRAILLE_SPINNER_FRAME_MS`].
pub const CADENCE_FRAME_MS: u64 = 50;

/// Receipt settle duration (~1/6 of 2.4s cadence).
pub const RECEIPT_SETTLE_MS: u64 = 400;

/// Stagger between receipts that land in a burst.
pub const RECEIPT_STAGGER_MS: u64 = 60;

/// One-shot completion surface duration (~1/3 of cadence).
pub const COMPLETION_SURFACE_MS: u64 = 800;

/// Peak brightness multiplier at the midpoint of the completion surface.
pub const COMPLETION_SURFACE_PEAK: f32 = 0.12;

/// Floor brightness for a settling receipt at t=0 (never fully invisible).
const SETTLE_DIM_FLOOR: f32 = 0.38;

/// Ease approximating cubic-bezier(0.32, 0.72, 0, 1) — organic settle curve.
#[must_use]
pub fn settle_ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    // Smoothstep-ish with a slightly front-loaded settle (matches HTML ease).
    let s = t * t * (3.0 - 2.0 * t);
    // Bias toward faster early motion (0.72-ish second control point).
    s * s * (3.0 - 2.0 * s).mul_add(0.35, 0.65).clamp(0.0, 1.0)
}

/// Progress of a receipt settle animation in `0.0..=1.0`.
///
/// `stagger_index` is the burst order (0 for the first cell in a burst).
/// When `low_motion` is true, always returns `1.0` (settled immediately).
#[must_use]
pub fn receipt_settle_progress(elapsed_ms: u128, stagger_index: u32, low_motion: bool) -> f32 {
    if low_motion {
        return 1.0;
    }
    let delay = u128::from(stagger_index) * u128::from(RECEIPT_STAGGER_MS);
    if elapsed_ms < delay {
        return 0.0;
    }
    let local = elapsed_ms - delay;
    if local >= u128::from(RECEIPT_SETTLE_MS) {
        return 1.0;
    }
    settle_ease(local as f32 / RECEIPT_SETTLE_MS as f32)
}

/// Whether a cell is still mid-settle (needs redraw + cache bust).
#[must_use]
pub fn is_receipt_settling(elapsed_ms: u128, stagger_index: u32, low_motion: bool) -> bool {
    receipt_settle_progress(elapsed_ms, stagger_index, low_motion) < 1.0
}

/// Discrete frame key used to bust transcript cache while settling.
///
/// Returns `0` once settled or under low motion so settled cells keep a
/// stable revision.
#[must_use]
pub fn settle_frame_key(elapsed_ms: u128, stagger_index: u32, low_motion: bool) -> u64 {
    if !is_receipt_settling(elapsed_ms, stagger_index, low_motion) {
        return 0;
    }
    let delay = u128::from(stagger_index) * u128::from(RECEIPT_STAGGER_MS);
    let local = elapsed_ms.saturating_sub(delay);
    u64::try_from(local / u128::from(CADENCE_FRAME_MS)).unwrap_or(0) + 1
}

/// Brightness boost for the completion surface in `0.0..=COMPLETION_SURFACE_PEAK`.
///
/// Returns `None` when idle / gated off / finished. Peak at mid-duration then
/// eases back to 0 — one breath outward, no loop.
#[must_use]
pub fn completion_surface_boost(
    elapsed_ms: u128,
    low_motion: bool,
    fancy_animations: bool,
) -> Option<f32> {
    if low_motion || !fancy_animations {
        return None;
    }
    if elapsed_ms >= u128::from(COMPLETION_SURFACE_MS) {
        return None;
    }
    let t = elapsed_ms as f32 / COMPLETION_SURFACE_MS as f32;
    // Triangle envelope: 0 → 1 → 0 across the duration.
    let envelope = if t <= 0.5 {
        t * 2.0
    } else {
        (1.0 - t) * 2.0
    };
    Some(envelope * COMPLETION_SURFACE_PEAK)
}

/// Whether the completion surface is still animating.
#[must_use]
pub fn is_completion_surface_active(
    elapsed_ms: u128,
    low_motion: bool,
    fancy_animations: bool,
) -> bool {
    completion_surface_boost(elapsed_ms, low_motion, fancy_animations).is_some()
}

/// Lerp an RGB color toward white by `amount` (0 = unchanged, 1 = white).
#[must_use]
pub fn lighten_color(color: Color, amount: f32) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    match color {
        Color::Rgb(r, g, b) => {
            let f = |c: u8| {
                let lifted = f32::from(c) + (255.0 - f32::from(c)) * amount;
                lifted.round().clamp(0.0, 255.0) as u8
            };
            Color::Rgb(f(r), f(g), f(b))
        }
        other => other,
    }
}

/// Scale an RGB foreground toward the dim floor by settle progress.
///
/// `progress` 0 = dimmest (just appeared), 1 = full ink.
#[must_use]
pub fn settle_color(color: Color, progress: f32) -> Color {
    let progress = progress.clamp(0.0, 1.0);
    let factor = SETTLE_DIM_FLOOR + (1.0 - SETTLE_DIM_FLOOR) * progress;
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(
            (f32::from(r) * factor).round().clamp(0.0, 255.0) as u8,
            (f32::from(g) * factor).round().clamp(0.0, 255.0) as u8,
            (f32::from(b) * factor).round().clamp(0.0, 255.0) as u8,
        ),
        other => other,
    }
}

/// Apply settle dimming to all spans in a line set. No-op when progress ≥ 1.
pub fn apply_receipt_settle(lines: &mut [Line<'static>], progress: f32) {
    if progress >= 1.0 {
        return;
    }
    for line in lines.iter_mut() {
        let spans = std::mem::take(&mut line.spans);
        line.spans = spans
            .into_iter()
            .map(|span| {
                let style = dim_style(span.style, progress);
                Span::styled(span.content, style)
            })
            .collect();
    }
}

fn dim_style(style: Style, progress: f32) -> Style {
    let mut out = style;
    if let Some(fg) = style.fg {
        out = out.fg(settle_color(fg, progress));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_progress_reaches_full_after_duration() {
        assert_eq!(receipt_settle_progress(0, 0, false), 0.0);
        assert!(receipt_settle_progress(200, 0, false) > 0.0);
        assert!(receipt_settle_progress(200, 0, false) < 1.0);
        assert_eq!(
            receipt_settle_progress(u128::from(RECEIPT_SETTLE_MS), 0, false),
            1.0
        );
        assert_eq!(
            receipt_settle_progress(u128::from(RECEIPT_SETTLE_MS) + 50, 0, false),
            1.0
        );
    }

    #[test]
    fn settle_respects_low_motion() {
        assert_eq!(receipt_settle_progress(0, 0, true), 1.0);
        assert!(!is_receipt_settling(0, 3, true));
        assert_eq!(settle_frame_key(0, 0, true), 0);
    }

    #[test]
    fn settle_stagger_delays_later_receipts() {
        // Second cell in a burst waits ~60ms before starting its ease.
        assert_eq!(receipt_settle_progress(0, 1, false), 0.0);
        assert_eq!(
            receipt_settle_progress(u128::from(RECEIPT_STAGGER_MS) - 1, 1, false),
            0.0
        );
        assert!(
            receipt_settle_progress(u128::from(RECEIPT_STAGGER_MS) + 1, 1, false) > 0.0,
            "staggered cell should start after delay"
        );
        // First cell is already partway through while second is still delayed.
        let first = receipt_settle_progress(u128::from(RECEIPT_STAGGER_MS), 0, false);
        let second = receipt_settle_progress(u128::from(RECEIPT_STAGGER_MS), 1, false);
        assert!(first > second);
    }

    #[test]
    fn settle_frame_key_advances_then_zeros() {
        assert_eq!(settle_frame_key(0, 0, false), 1);
        assert_eq!(
            settle_frame_key(u128::from(CADENCE_FRAME_MS), 0, false),
            2
        );
        assert_eq!(
            settle_frame_key(u128::from(RECEIPT_SETTLE_MS), 0, false),
            0
        );
    }

    #[test]
    fn completion_surface_is_one_shot_envelope() {
        assert!(completion_surface_boost(0, false, true).is_some());
        let mid = completion_surface_boost(u128::from(COMPLETION_SURFACE_MS) / 2, false, true)
            .expect("mid");
        let early = completion_surface_boost(50, false, true).expect("early");
        let late = completion_surface_boost(u128::from(COMPLETION_SURFACE_MS) - 50, false, true)
            .expect("late");
        assert!(mid > early);
        assert!(mid > late);
        assert!(
            (mid - COMPLETION_SURFACE_PEAK).abs() < 0.001,
            "peak should hit COMPLETION_SURFACE_PEAK, got {mid}"
        );
        assert!(
            completion_surface_boost(u128::from(COMPLETION_SURFACE_MS), false, true).is_none()
        );
    }

    #[test]
    fn completion_surface_gated_on_motion_flags() {
        assert!(completion_surface_boost(0, true, true).is_none());
        assert!(completion_surface_boost(0, false, false).is_none());
        assert!(completion_surface_boost(0, true, false).is_none());
    }

    #[test]
    fn apply_receipt_settle_dims_rgb_then_restores() {
        let mut lines = vec![Line::from(Span::styled(
            "✓ read",
            Style::default().fg(Color::Rgb(200, 200, 200)),
        ))];
        apply_receipt_settle(&mut lines, 0.0);
        match lines[0].spans[0].style.fg {
            Some(Color::Rgb(r, g, b)) => {
                assert!(r < 200 && g < 200 && b < 200, "should dim at progress 0");
            }
            other => panic!("expected RGB after settle, got {other:?}"),
        }

        let mut full = vec![Line::from(Span::styled(
            "✓ read",
            Style::default().fg(Color::Rgb(200, 200, 200)),
        ))];
        apply_receipt_settle(&mut full, 1.0);
        assert_eq!(full[0].spans[0].style.fg, Some(Color::Rgb(200, 200, 200)));
    }

    #[test]
    fn lighten_color_moves_toward_white() {
        let base = Color::Rgb(10, 20, 30);
        let lit = lighten_color(base, 0.12);
        match lit {
            Color::Rgb(r, g, b) => {
                assert!(r > 10 && g > 20 && b > 30);
            }
            other => panic!("expected RGB, got {other:?}"),
        }
        assert_eq!(lighten_color(base, 0.0), base);
    }
}
