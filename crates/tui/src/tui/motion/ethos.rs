//! Chef's-choice motion ethos for the underwater TUI.
//!
//! Infuses Omarchy (DHH / Hyprland) timing — arrivals land, exits get out of
//! the way, surfaces pop from ~87% rather than growing from a vanishing point
//! — without copying Hyprland's look. Codewhale keeps Blue Stage water, the
//! whale, gold current, and ombre depth. Workspace/page slide theater stays
//! off: spatial memory beats animated travel.
//!
//! Reduced/Still skip decorative treatments via [`super::MotionPolicy`].

use super::MotionPolicy;

/// Arrivals. Things land; they do not ease-in from a crawl.
#[must_use]
pub fn ease_out_quint(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(5)
}

/// Exits are faster than entries and closer to linear — get out of the way.
#[must_use]
#[allow(dead_code)] // chef's-choice exit curve; picker dismiss waits on a redraw driver
pub fn ease_out_exit(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t)
}

/// Almost-linear fade window. No 400 ms theatrical dissolves.
#[allow(dead_code)] // chef's-choice fade window; hosts should ask here rather than invent
pub const FADE_MS: u128 = 160;

/// Picker/menu surface pop-in. Snappy settle, not cinematic.
pub const SURFACE_POP_MS: u128 = 180;

/// Exits complete sooner than entries.
#[allow(dead_code)] // chef's-choice exit window; picker dismiss waits on a redraw driver
pub const SURFACE_EXIT_MS: u128 = 120;

/// Surfaces pop from this scale, never from 0.
pub const SURFACE_POP_FROM: f32 = 0.87;

/// Authored empty-state whale surface window. Keep this duration; change the
/// curve, not the length, so arrival stays a beat rather than a splash.
pub const WELCOME_SURFACE_MS: u128 = 640;

/// Receipt stagger (one-shot). Do not lengthen toward cinematic.
pub const RECEIPT_STAGGER_MS: u128 = 70;

/// Fish flee-and-return arc. Do not lengthen toward cinematic.
pub const FISH_FLEE_MS: u128 = 800;

/// Mix of linear and a whisper of ease-out so short fades do not look robotic.
#[must_use]
#[allow(dead_code)] // chef's-choice fade; hosts should ask here rather than invent
pub fn fade_opacity(elapsed_ms: u128, policy: MotionPolicy) -> f32 {
    if !policy.allows_decorative() {
        return 1.0;
    }
    almost_linear(elapsed_ms, FADE_MS)
}

#[must_use]
#[allow(dead_code)] // used by fade_opacity; kept as the shared almost-linear mix
fn almost_linear(elapsed_ms: u128, duration_ms: u128) -> f32 {
    if duration_ms == 0 {
        return 1.0;
    }
    let t = (elapsed_ms as f32 / duration_ms as f32).clamp(0.0, 1.0);
    0.85 * t + 0.15 * ease_out_exit(t)
}

/// 0.87 → 1.0 over [`SURFACE_POP_MS`] with ease-out-quint. Reduced/Still land
/// at 1.0 immediately so the picker never grows from a vanishing point.
#[must_use]
#[allow(dead_code)] // picker DIM host API; live redraw driver is a follow-up
pub fn surface_pop(elapsed_ms: u128, policy: MotionPolicy) -> f32 {
    if !policy.allows_decorative() || elapsed_ms >= SURFACE_POP_MS {
        return 1.0;
    }
    let t = ease_out_quint(elapsed_ms as f32 / SURFACE_POP_MS as f32);
    SURFACE_POP_FROM + (1.0 - SURFACE_POP_FROM) * t
}

/// 1.0 → 0.0 over [`SURFACE_EXIT_MS`]. Faster than the matching pop-in.
#[must_use]
#[allow(dead_code)] // picker DIM host API; live redraw driver is a follow-up
pub fn surface_exit(elapsed_ms: u128, policy: MotionPolicy) -> f32 {
    if !policy.allows_decorative() || elapsed_ms >= SURFACE_EXIT_MS {
        return 0.0;
    }
    1.0 - ease_out_exit(elapsed_ms as f32 / SURFACE_EXIT_MS as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::motion::MotionPolicy;

    fn full() -> MotionPolicy {
        MotionPolicy::from_settings(false, true, false)
    }

    fn reduced() -> MotionPolicy {
        MotionPolicy::from_settings(true, true, false)
    }

    fn still() -> MotionPolicy {
        MotionPolicy::from_settings(false, false, false)
    }

    #[test]
    fn arrivals_land_instead_of_crawling_in() {
        let early = ease_out_quint(0.2);
        let raised_cosine_early = 0.5 * (1.0 - (std::f32::consts::PI * 0.2).cos());
        assert!(
            early > raised_cosine_early + 0.4,
            "ease-out-quint must be well ahead of a raised-cosine crawl at t=0.2: {early} vs {raised_cosine_early}"
        );
        assert!(
            (ease_out_quint(0.5) - 0.96875).abs() < 0.001,
            "mid-arrival should already have landed"
        );
        assert_eq!(ease_out_quint(0.0), 0.0);
        assert_eq!(ease_out_quint(1.0), 1.0);
    }

    #[test]
    fn exits_are_faster_than_entries_and_closer_to_linear() {
        const { assert!(SURFACE_EXIT_MS < SURFACE_POP_MS) };
        let t = 0.4;
        let entry = ease_out_quint(t);
        let exit = ease_out_exit(t);
        assert!(
            (exit - t).abs() < (entry - t).abs(),
            "exit curve should hug linear more than quint: exit={exit} entry={entry} t={t}"
        );
        let entry_remaining = SURFACE_POP_MS - 80;
        let exit_remaining = SURFACE_EXIT_MS.saturating_sub(80);
        assert!(
            exit_remaining < entry_remaining,
            "an exit started at the same instant finishes first"
        );
    }

    #[test]
    fn surfaces_pop_from_nearly_full_scale_never_from_zero() {
        assert!((SURFACE_POP_FROM - 0.87).abs() < f32::EPSILON);
        let start = surface_pop(0, full());
        assert!(
            (start - SURFACE_POP_FROM).abs() < 0.001,
            "first frame must already occupy most of the final surface: {start}"
        );
        assert!(start > 0.8);
        assert_eq!(surface_pop(SURFACE_POP_MS, full()), 1.0);
        assert_eq!(surface_pop(SURFACE_POP_MS + 40, full()), 1.0);
    }

    #[test]
    fn fade_is_short_and_almost_linear() {
        assert!((150..=180).contains(&FADE_MS));
        let mid = fade_opacity(FADE_MS / 2, full());
        assert!(
            (mid - 0.5).abs() < 0.08,
            "a 160ms fade should be close to linear at the midpoint: {mid}"
        );
        assert_eq!(fade_opacity(FADE_MS, full()), 1.0);
    }

    #[test]
    fn reduced_and_still_skip_decorative_pop_and_fade() {
        for policy in [reduced(), still()] {
            assert_eq!(surface_pop(0, policy), 1.0);
            assert_eq!(surface_exit(0, policy), 0.0);
            assert_eq!(fade_opacity(0, policy), 1.0);
        }
    }

    #[test]
    fn authored_one_shots_stay_snappy() {
        assert_eq!(WELCOME_SURFACE_MS, 640);
        assert_eq!(RECEIPT_STAGGER_MS, 70);
        assert_eq!(FISH_FLEE_MS, 800);
    }
}
