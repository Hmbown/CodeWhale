//! One-shot primary-display refresh probe with fail-closed defaults.
//!
//! Adapted from Grok's host display-refresh probe: measure once per process,
//! clamp to sane bounds, and never panic into the render loop. SSH / missing
//! FFI paths fall back to the fixed [`crate::tui::frame_rate_limiter`] defaults.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Inclusive lower bound for accepted refresh rates.
pub const MIN_HZ: u32 = 30;
/// Inclusive upper bound for accepted refresh rates.
pub const MAX_HZ: u32 = 240;
/// Safe fallback when probing is skipped or fails (≈8 fps / 120 ms underwater
/// atmosphere — not the draw-rate cap). Calmed from the historical 80 ms for
/// v0.9.4: the field still breathes, but the ambient cadence no longer feels
/// restless next to real content.
pub const FALLBACK_ANIMATION_MS: u64 = 120;
/// Absolute floor for adaptive animation intervals (≈ 4 fps).
pub const MIN_ANIMATION_HZ: u32 = 4;
/// Absolute ceiling for adaptive animation intervals (≈ 30 fps).
pub const MAX_ANIMATION_HZ: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayRefreshSource {
    None,
    // Only constructed inside `#[cfg(target_os = "macos")]` in `probe_inner`
    // below; non-macOS builds never build a value of this variant.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    MacosCoreGraphics,
    EnvOverride,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRefreshProbeResult {
    pub hz: Option<u32>,
    pub source: DisplayRefreshSource,
    /// Stable skip/error token when `hz` is `None`.
    pub skip_reason: &'static str,
    pub duration_ms: u64,
}

impl DisplayRefreshProbeResult {
    #[must_use]
    pub fn outcome(self) -> &'static str {
        if self.hz.is_some() {
            "ok"
        } else if self.skip_reason == "error" {
            "error"
        } else {
            "skipped"
        }
    }
}

// Keep outcome() public for diagnostics even when the TUI only logs the probe
// struct fields today.
const _: fn(DisplayRefreshProbeResult) -> &'static str = DisplayRefreshProbeResult::outcome;

/// Once per process. Infallible.
pub fn probe_display_refresh() -> DisplayRefreshProbeResult {
    static CACHE: OnceLock<DisplayRefreshProbeResult> = OnceLock::new();
    *CACHE.get_or_init(probe_uncached)
}

fn probe_uncached() -> DisplayRefreshProbeResult {
    let start = Instant::now();
    let (hz, source, skip_reason) = probe_inner();
    DisplayRefreshProbeResult {
        hz,
        source,
        skip_reason,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

fn probe_inner() -> (Option<u32>, DisplayRefreshSource, &'static str) {
    if let Some(hz) = env_override_hz() {
        return match accept_hz(hz) {
            Some(hz) => (Some(hz), DisplayRefreshSource::EnvOverride, ""),
            None => (None, DisplayRefreshSource::EnvOverride, "out_of_range"),
        };
    }
    if is_remote_session() {
        return (None, DisplayRefreshSource::None, "ssh");
    }
    #[cfg(target_os = "macos")]
    {
        match probe_macos() {
            Ok(hz) => match accept_hz(hz) {
                Some(hz) => (Some(hz), DisplayRefreshSource::MacosCoreGraphics, ""),
                None => (
                    None,
                    DisplayRefreshSource::MacosCoreGraphics,
                    "out_of_range",
                ),
            },
            Err(reason) => (None, DisplayRefreshSource::MacosCoreGraphics, reason),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        (None, DisplayRefreshSource::None, "unsupported")
    }
}

fn env_override_hz() -> Option<u32> {
    let raw = std::env::var("CODEWHALE_DISPLAY_HZ").ok()?;
    raw.trim().parse().ok()
}

fn is_remote_session() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_CLIENT").is_some()
        || std::env::var_os("SSH_TTY").is_some()
}

fn accept_hz(hz: u32) -> Option<u32> {
    if (MIN_HZ..=MAX_HZ).contains(&hz) {
        Some(hz)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn probe_macos() -> Result<u32, &'static str> {
    // CoreGraphics is available on macOS; use a minimal safe FFI for the main
    // display mode. Fail closed on any error.
    //
    // CGDisplayModeGetRefreshRate returns 0 for some virtual displays — treat
    // that as skipped rather than forcing a zero cadence.
    unsafe extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayCopyDisplayMode(display: u32) -> *mut std::ffi::c_void;
        fn CGDisplayModeGetRefreshRate(mode: *mut std::ffi::c_void) -> f64;
        fn CGDisplayModeRelease(mode: *mut std::ffi::c_void);
    }
    unsafe {
        let display = CGMainDisplayID();
        let mode = CGDisplayCopyDisplayMode(display);
        if mode.is_null() {
            return Err("no_mode");
        }
        let rate = CGDisplayModeGetRefreshRate(mode);
        CGDisplayModeRelease(mode);
        if !rate.is_finite() || rate <= 0.0 {
            return Err("zero_rate");
        }
        let hz = rate.round() as u32;
        if hz == 0 {
            return Err("zero_rate");
        }
        Ok(hz)
    }
}

/// Convert a measured display Hz into a bounded animation interval.
///
/// Policy: target roughly `display_hz / 8` for atmosphere (calm, not steppy
/// on high-Hz panels), clamped to [`MIN_ANIMATION_HZ`]..=[`MAX_ANIMATION_HZ`].
/// Missing measurement falls back to [`FALLBACK_ANIMATION_MS`] (≈8 fps /
/// 120 ms atmosphere). `low_motion` always wins (2.4s).
#[must_use]
pub fn animation_interval_for_hz(display_hz: Option<u32>, low_motion: bool) -> Duration {
    if low_motion {
        return Duration::from_millis(2_400);
    }
    match display_hz {
        // No measurement, or a standard 60 Hz panel: keep the 120 ms
        // atmosphere cadence so low-Hz hosts do not feel steppier.
        None | Some(0..=60) => Duration::from_millis(FALLBACK_ANIMATION_MS),
        // High-Hz panels: ~1/8 of refresh, bounded so we never thrash or stall.
        Some(hz) => {
            let target = (hz / 8).clamp(MIN_ANIMATION_HZ, MAX_ANIMATION_HZ);
            let ms = (1000u32 / target.max(1)).max(1);
            // Never slower than the fallback (only raise cadence).
            Duration::from_millis(u64::from(ms).min(FALLBACK_ANIMATION_MS))
        }
    }
}

/// Convenience: probe once and return the animation interval for the current
/// motion policy. Safe to call every frame — probe is OnceLock-cached.
#[must_use]
pub fn adaptive_animation_interval_ms(low_motion: bool) -> u64 {
    let probe = probe_display_refresh();
    animation_interval_for_hz(probe.hz, low_motion).as_millis() as u64
}

/// Map measured Hz into a frame-rate limiter minimum interval, never exceeding
/// the historical 120 FPS draw cap and never undercutting low-motion 30 FPS.
#[must_use]
pub fn draw_min_interval_for_hz(display_hz: Option<u32>, low_motion: bool) -> Duration {
    use super::frame_rate_limiter::{LOW_MOTION_MIN_FRAME_INTERVAL, MIN_FRAME_INTERVAL};
    if low_motion {
        return LOW_MOTION_MIN_FRAME_INTERVAL;
    }
    let Some(hz) = display_hz else {
        return MIN_FRAME_INTERVAL;
    };
    // Cap draw rate at min(display_hz, 120). Never faster than MIN_FRAME_INTERVAL.
    let capped = hz.clamp(30, 120);
    let nanos = 1_000_000_000u64 / u64::from(capped);
    Duration::from_nanos(nanos).max(MIN_FRAME_INTERVAL)
}

/// Content-driven draw cadence: atmosphere rate when only ambience moves;
/// full rate for stream / selection / input / hover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawCadenceTier {
    /// Only ambient life / ocean breath — use atmosphere interval.
    Atmosphere,
    /// Streaming, selection, input, or interactive hover — full draw cap.
    Interactive,
}

/// Choose the draw min-interval for the current content tier.
#[must_use]
pub fn content_driven_draw_interval(
    tier: DrawCadenceTier,
    display_hz: Option<u32>,
    low_motion: bool,
) -> Duration {
    match tier {
        DrawCadenceTier::Atmosphere => animation_interval_for_hz(display_hz, low_motion),
        DrawCadenceTier::Interactive => draw_min_interval_for_hz(display_hz, low_motion),
    }
}

/// Infer cadence tier from coarse app activity signals.
#[must_use]
pub fn cadence_tier_from_signals(
    streaming_or_loading: bool,
    selection_active: bool,
    input_nonempty: bool,
    pointer_hover_active: bool,
) -> DrawCadenceTier {
    if streaming_or_loading || selection_active || input_nonempty || pointer_hover_active {
        DrawCadenceTier::Interactive
    } else {
        DrawCadenceTier::Atmosphere
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::frame_rate_limiter::{LOW_MOTION_MIN_FRAME_INTERVAL, MIN_FRAME_INTERVAL};

    #[test]
    fn falls_back_to_default_when_probe_has_no_hz() {
        let interval = animation_interval_for_hz(None, false);
        assert_eq!(interval, Duration::from_millis(FALLBACK_ANIMATION_MS));
        assert_eq!(FALLBACK_ANIMATION_MS, 120);
    }

    #[test]
    fn low_motion_wins_over_measured_hz() {
        let interval = animation_interval_for_hz(Some(144), true);
        assert_eq!(interval, Duration::from_millis(2_400));
    }

    #[test]
    fn high_hz_display_raises_cadence_but_stays_bounded() {
        let interval = animation_interval_for_hz(Some(144), false);
        // 144/8 = 18 → ~55ms: calmer than the old 1/5 divisor, still smooth.
        assert!(interval >= Duration::from_millis(33));
        assert!(interval <= Duration::from_millis(250));
    }

    #[test]
    fn sixty_hz_keeps_historical_atmosphere_cadence() {
        let interval = animation_interval_for_hz(Some(60), false);
        // Standard panels stay on the 120 ms atmosphere floor.
        assert_eq!(interval, Duration::from_millis(FALLBACK_ANIMATION_MS));
    }

    #[test]
    fn accept_hz_rejects_out_of_range() {
        assert_eq!(accept_hz(10), None);
        assert_eq!(accept_hz(60), Some(60));
        assert_eq!(accept_hz(500), None);
    }

    #[test]
    fn draw_cap_never_exceeds_historical_min_interval() {
        let interval = draw_min_interval_for_hz(Some(240), false);
        assert!(interval >= MIN_FRAME_INTERVAL);
    }

    #[test]
    fn draw_cap_respects_low_motion() {
        assert_eq!(
            draw_min_interval_for_hz(Some(144), true),
            LOW_MOTION_MIN_FRAME_INTERVAL
        );
    }

    #[test]
    fn probe_is_infallible_and_cached() {
        let a = probe_display_refresh();
        let b = probe_display_refresh();
        assert_eq!(a, b);
        // Either measured or skipped — never panics.
        assert!(a.outcome() == "ok" || a.outcome() == "skipped" || a.outcome() == "error");
    }
}
