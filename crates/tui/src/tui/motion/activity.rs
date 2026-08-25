//! Activity-shaped motion waveforms.
//!
//! Cadence numbers live with [`crate::tui::ambient_life::ActivityMotion`] so
//! the ocean, phase strip, and streaming caret share one table. This module
//! owns the pure functions those cadences drive: a layout-stable caret pulse
//! and the slower attention breath used while the shell is waiting on you.

/// Streaming-caret brightness in `[floor, 1.0]`. The glyph stays on screen
/// for the whole stream so adjacent text never shifts; only the ink breathes.
#[must_use]
pub fn caret_brightness(elapsed_ms: u128, period_ms: u128) -> f32 {
    wave01(elapsed_ms, period_ms, 0.35)
}

/// Waiting-on-you diamond breath. Trough stays readable (never black).
#[must_use]
pub fn attention_brightness(elapsed_ms: u128, period_ms: u128) -> f32 {
    wave01(elapsed_ms, period_ms, 0.40)
}

fn wave01(elapsed_ms: u128, period_ms: u128, floor: f32) -> f32 {
    let floor = floor.clamp(0.0, 1.0);
    let period = period_ms.max(1);
    let frac = (elapsed_ms % period) as f64 / period as f64;
    let s = (frac * std::f64::consts::PI).sin();
    let wave = (s * s) as f32;
    floor + (1.0 - floor) * wave
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_brightness_stays_bounded_and_moves() {
        let period = 900u128;
        let samples: Vec<f32> = (0..=period)
            .step_by(30)
            .map(|ms| caret_brightness(ms, period))
            .collect();
        assert!(
            samples.iter().all(|value| (0.35..=1.0).contains(value)),
            "caret brightness left its envelope: {samples:?}"
        );
        assert!(
            samples
                .windows(2)
                .any(|pair| (pair[1] - pair[0]).abs() > 0.01),
            "caret brightness must still travel"
        );
        assert!(
            (caret_brightness(0, period) - 0.35).abs() < 1e-5,
            "sin² trough is the authored floor"
        );
        let peak = caret_brightness(period / 2, period);
        assert!(
            (peak - 1.0).abs() < 1e-5,
            "sin² crest is full ink, got {peak}"
        );
    }

    #[test]
    fn attention_brightness_is_calmer_than_the_caret_floor() {
        assert!(attention_brightness(0, 1_300) > caret_brightness(0, 1_300));
        assert!((attention_brightness(650, 1_300) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn zero_period_does_not_panic() {
        assert!(caret_brightness(12, 0) >= 0.35);
        assert!(attention_brightness(12, 0) >= 0.40);
    }
}
