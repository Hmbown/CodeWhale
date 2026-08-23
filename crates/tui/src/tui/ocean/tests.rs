use super::*;

fn distance(a: Color, b: Color) -> u16 {
    let (ar, ag, ab) = rgb(a).expect("RGB color");
    let (br, bg, bb) = rgb(b).expect("RGB color");
    ar.abs_diff(br) as u16 + ag.abs_diff(bg) as u16 + ab.abs_diff(bb) as u16
}

fn relative_luminance(value: Color) -> f64 {
    let (r, g, b) = rgb(value).expect("contrast colors must be RGB");
    let linearize = |component: u8| {
        let srgb = f64::from(component) / 255.0;
        if srgb <= 0.04045 {
            srgb / 12.92
        } else {
            ((srgb + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linearize(r) + 0.7152 * linearize(g) + 0.0722 * linearize(b)
}

fn contrast_ratio(foreground: Color, background: Color) -> f64 {
    let foreground = relative_luminance(foreground);
    let background = relative_luminance(background);
    let (lighter, darker) = if foreground >= background {
        (foreground, background)
    } else {
        (background, foreground)
    };
    (lighter + 0.05) / (darker + 0.05)
}

#[test]
fn whale_ramp_is_perceptibly_deep_not_merely_non_equal() {
    let ramp = OceanRamp::for_theme(&crate::palette::UI_THEME).expect("RGB theme");
    assert_eq!(ramp.surface, Color::Rgb(0x10, 0x2a, 0x45));
    assert_eq!(ramp.middle, Color::Rgb(0x0a, 0x1e, 0x33));
    assert_eq!(ramp.deep, Color::Rgb(0x06, 0x13, 0x20));
    assert!(
        distance(ramp.surface, ramp.deep) >= 32,
        "the selected underwater treatment must read at a glance"
    );
    assert_ne!(ramp.color_at(0, 20), ramp.color_at(19, 20));
}

#[test]
fn whale_column_stays_blue_and_gently_banded_at_full_screen_depth() {
    let theme = crate::palette::UI_THEME;
    let ramp = OceanRamp::for_theme(&theme).expect("RGB theme");
    let mut previous = ramp.color_at(0, 80);

    for row in 0..80 {
        let current = ramp.color_at(row, 80);
        let (red, green, blue) = rgb(current).expect("RGB ocean color");
        assert!(
            blue > green && green > red,
            "row {row} lost the authored blue-ocean ordering: {current:?}"
        );
        assert!(
            relative_luminance(current) > relative_luminance(theme.surface_bg),
            "row {row} fell back into the near-black shell field"
        );
        assert!(
            distance(previous, current) <= 4,
            "row {row} introduced a hard depth seam"
        );
        previous = current;
    }

    assert_eq!(ramp.color_at(0, 80), ramp.surface);
    assert_eq!(ramp.color_at(79, 80), ramp.deep);
}

#[test]
fn whale_ocean_keeps_text_and_semantic_roles_readable() {
    let theme = crate::palette::UI_THEME;
    let ramp = OceanRamp::for_theme(&theme).expect("RGB theme");
    let foregrounds = [
        ("body", theme.text_body),
        ("soft", theme.text_soft),
        ("muted", theme.text_muted),
        ("hint", theme.text_hint),
        ("action", theme.accent_primary),
        ("live", theme.status_working),
        ("human", theme.accent_action),
        ("warning", theme.warning),
        ("danger", theme.error_fg),
        ("act mode", theme.mode_agent),
        ("plan mode", theme.mode_plan),
        ("operate", theme.mode_operate),
        ("full-access mode", theme.mode_yolo),
        ("success", theme.success),
    ];

    for (background_name, background) in [
        ("ocean surface", ramp.surface),
        ("ocean middle", ramp.middle),
        ("ocean deep", ramp.deep),
    ] {
        for (foreground_name, foreground) in foregrounds {
            let ratio = contrast_ratio(foreground, background);
            assert!(
                ratio >= 4.5,
                "Whale {foreground_name} on {background_name} contrast {ratio:.2} is below 4.50"
            );
        }
    }
}

#[test]
fn light_theme_stays_light_enough_for_light_theme_text() {
    let ramp = OceanRamp::for_theme(&crate::palette::LIGHT_UI_THEME).expect("RGB theme");
    assert_eq!(ramp.surface, Color::Rgb(0xff, 0xfd, 0xf8));
    assert_eq!(ramp.middle, Color::Rgb(0xf4, 0xf7, 0xfb));
    assert_eq!(ramp.deep, Color::Rgb(0xf0, 0xf4, 0xf9));
    let (r, g, b) = rgb(ramp.deep).expect("RGB color");
    assert!(u16::from(r) + u16::from(g) + u16::from(b) > 420);
}

#[test]
fn light_ocean_and_selection_keep_text_and_semantic_roles_readable() {
    let theme = crate::palette::LIGHT_UI_THEME;
    let ramp = OceanRamp::for_theme(&theme).expect("RGB theme");
    let foregrounds = [
        ("body", theme.text_body),
        ("soft", theme.text_soft),
        ("muted", theme.text_muted),
        ("hint", theme.text_hint),
        ("action", theme.accent_primary),
        ("live", theme.status_working),
        ("human", theme.accent_action),
        ("warning", theme.warning),
        ("danger", theme.error_fg),
        ("act mode", theme.mode_agent),
        ("plan mode", theme.mode_plan),
        ("operate", theme.mode_operate),
        ("full-access mode", theme.mode_yolo),
        ("success", theme.success),
        ("user", crate::palette::LIGHT_USER_BODY),
    ];
    let backgrounds = [
        ("ocean surface", ramp.surface),
        ("ocean middle", ramp.middle),
        ("ocean deep", ramp.deep),
        ("selection", theme.selection_bg),
    ];

    for (background_name, background) in backgrounds {
        for (foreground_name, foreground) in foregrounds {
            let ratio = contrast_ratio(foreground, background);
            assert!(
                ratio >= 4.5,
                "light {foreground_name} on {background_name} contrast {ratio:.2} is below 4.50"
            );
        }
    }
}

#[test]
fn whale_custom_background_uses_the_configured_surface() {
    let custom = Color::Rgb(0x12, 0x1a, 0x2d);
    let theme = crate::palette::UI_THEME.with_background_color(custom);
    let ramp = OceanRamp::for_theme(&theme).expect("custom backgrounds retain ombre");

    assert_ne!(ramp.surface, Color::Rgb(0x0e, 0x17, 0x29));
    assert_ne!(ramp.surface, ramp.deep);
}

#[test]
fn inherited_terminal_background_reports_no_ramp() {
    let mut theme = crate::palette::UI_THEME;
    theme.surface_bg = Color::Reset;
    assert_eq!(OceanRamp::for_theme(&theme), None);
}

#[test]
fn solarized_light_preserves_its_canonical_base3_background() {
    let theme = crate::palette::SOLARIZED_LIGHT_UI_THEME;

    assert_eq!(theme.surface_bg, Color::Rgb(0xfd, 0xf6, 0xe3));
    assert_eq!(OceanRamp::for_theme(&theme), None);
}

#[test]
fn solarized_light_custom_background_preserves_ombre() {
    let custom = Color::Rgb(0x1a, 0x1b, 0x26);
    let theme = crate::palette::SOLARIZED_LIGHT_UI_THEME.with_background_color(custom);
    let ramp = OceanRamp::for_theme(&theme).expect("custom backgrounds retain ombre");

    assert_ne!(ramp.surface, custom);
    assert_ne!(ramp.surface, ramp.deep);
}

#[test]
fn every_shipped_theme_has_an_intentional_ocean_treatment() {
    use crate::palette::{SELECTABLE_THEMES, ThemeId};

    for id in SELECTABLE_THEMES {
        let ramp = OceanRamp::for_theme(&id.ui_theme());
        if matches!(id, ThemeId::Terminal | ThemeId::SolarizedLight) {
            assert_eq!(
                ramp,
                None,
                "{} must keep its canonical background",
                id.name()
            );
        } else {
            let ramp = ramp.unwrap_or_else(|| panic!("{} has no ocean ramp", id.name()));
            assert_ne!(
                ramp.surface,
                ramp.deep,
                "{} lost underwater depth",
                id.name()
            );
        }
    }
}

#[test]
fn treatment_parses_saved_values_and_defaults_to_ombre() {
    assert_eq!(OceanTreatment::parse("flat"), OceanTreatment::Flat);
    assert_eq!(OceanTreatment::parse(" FLAT "), OceanTreatment::Flat);
    assert_eq!(OceanTreatment::parse("ombre"), OceanTreatment::Ombre);
    assert_eq!(OceanTreatment::parse("kelp"), OceanTreatment::Ombre);
    assert_eq!(OceanTreatment::parse(""), OceanTreatment::Ombre);
    // Migration shim: settings saved by pre-0.9.4 builds may still carry the
    // removed classic shell; they load as the default ombre treatment.
    assert_eq!(OceanTreatment::parse("classic"), OceanTreatment::Ombre);
}

#[test]
fn every_underwater_treatment_keeps_ambient_life() {
    // The classic shell was the only treatment that stilled ambient life; with
    // it removed there is no per-treatment ambient-life flag left to test.
    // What remains worth pinning: both live treatments stay distinct so the
    // flat/ombre choice keeps its meaning.
    assert_ne!(OceanTreatment::Ombre, OceanTreatment::Flat);
    assert!(OceanTreatment::Ombre.is_ombre());
    assert!(OceanTreatment::Flat.is_flat());
}

#[test]
fn ambient_ink_matches_sunk_sky_shades_and_survives_reset_surfaces() {
    // RGB themes: fish wear two sunk sky shades; seafoam remains live-work ink.
    let theme = crate::palette::UI_THEME;
    let ramp = OceanRamp::for_theme(&theme).expect("RGB theme");
    let baseline = crate::tui::ambient_life::AmbientActivity::Baseline;
    let (primary, secondary) = ambient_inks_for_activity(&theme, baseline);
    assert_ne!(primary, ramp.ambient);
    assert_ne!(primary, secondary);
    assert_ne!(primary, theme.accent_secondary);

    // Terminal-owned surfaces have no RGB base; the raw secondary accent
    // lets the terminal's own palette color the life.
    let terminal = crate::palette::TERMINAL_UI_THEME;
    assert_eq!(
        ambient_inks_for_activity(&terminal, baseline),
        (terminal.info, terminal.info)
    );
}

#[test]
fn ambient_ink_reads_the_activity_at_a_glance() {
    use crate::tui::ambient_life::AmbientActivity;
    let theme = crate::palette::UI_THEME;
    let baseline = ambient_inks_for_activity(&theme, AmbientActivity::Baseline);
    let reasoning = ambient_inks_for_activity(&theme, AmbientActivity::Reasoning);
    let tools = ambient_inks_for_activity(&theme, AmbientActivity::Tools);
    let subagents = ambient_inks_for_activity(&theme, AmbientActivity::Subagents);

    // Each activity wears its own water: dim deep for reasoning, bright
    // current for tools, seafoam for orchestration.
    assert_ne!(reasoning, baseline);
    assert_ne!(tools, baseline);
    assert_ne!(subagents, baseline);
    assert_ne!(subagents, tools);
    // Verifying keeps the metered baseline treatment.
    assert_eq!(
        ambient_inks_for_activity(&theme, AmbientActivity::Verifying),
        baseline
    );
}

#[test]
fn attention_phases_tint_the_water_even_when_life_has_settled() {
    let viewport = Rect::new(0, 0, 80, 24);
    let ramp = OceanRamp::for_theme(&crate::palette::UI_THEME).expect("RGB theme");
    // presence 0 + animated false is the fully settled, reduced-motion case —
    // exactly where the old treatment went neutral and a blocked session was
    // indistinguishable from an idle one across the room.
    let waiting = OceanColumn::new(ramp, viewport, 0, None, ShellPhase::Approval, false, 0);
    let failed = OceanColumn::new(ramp, viewport, 0, None, ShellPhase::Failed, false, 0);
    let idle = OceanColumn::new(ramp, viewport, 0, None, ShellPhase::Idle, false, 0);

    assert_ne!(waiting.color_at_y(0), idle.color_at_y(0));
    assert_ne!(failed.color_at_y(0), idle.color_at_y(0));
    assert_ne!(waiting.color_at_y(0), failed.color_at_y(0));

    // The tint is steady across time and motion settings alike.
    let later = OceanColumn::new(ramp, viewport, 700, None, ShellPhase::Approval, true, 0);
    assert_eq!(waiting.color_at_y(0), later.color_at_y(0));
}

#[test]
fn shimmer_is_subtle_and_concentrated_near_the_surface() {
    let ramp = OceanRamp::for_theme(&crate::palette::UI_THEME).expect("RGB theme");
    let surface_a = ramp.color_at_phase(0, 20, 0, ShellPhase::Idle);
    let surface_b = ramp.color_at_phase(0, 20, 22_500, ShellPhase::Idle);
    let deep_a = ramp.color_at_phase(19, 20, 0, ShellPhase::Idle);
    let deep_b = ramp.color_at_phase(19, 20, 22_500, ShellPhase::Idle);

    let surface_shift = distance(surface_a, surface_b);
    assert!(
        (1..=8).contains(&surface_shift),
        "surface shift was {surface_shift}"
    );
    assert_eq!(
        deep_a, deep_b,
        "the floor should stay perceptually anchored"
    );
}

#[test]
fn attention_phases_carry_their_own_water_and_work_phases_have_distinct_depth_bias() {
    let ramp = OceanRamp::for_theme(&crate::palette::UI_THEME).expect("RGB theme");
    // Attention tints are steady — the color itself is the signal, and a
    // slow breath read as flicker rather than intent — but never neutral:
    // each attention phase differs from the plain water.
    for phase in [
        ShellPhase::Waiting,
        ShellPhase::Approval,
        ShellPhase::Failed,
    ] {
        assert_eq!(
            ramp.color_at_phase(4, 20, 0, phase),
            ramp.color_at_phase(4, 20, 45_000, phase)
        );
        assert_ne!(ramp.color_at_phase(4, 20, 0, phase), ramp.color_at(4, 20));
    }
    assert_ne!(
        ramp.color_at_phase(10, 20, 22_500, ShellPhase::Working),
        ramp.color_at_phase(10, 20, 22_500, ShellPhase::Verifying)
    );
}

#[test]
fn tall_columns_darken_continuously_without_an_anchor_shelf() {
    // The old two-segment ramp met at 0.42 with zero color velocity on both
    // sides: on a tall window that shelf read as a horizontal seam. The
    // Bézier column must keep moving through the former anchor zone.
    let ramp = OceanRamp::for_theme(&crate::palette::UI_THEME).expect("RGB theme");
    let height = 120;
    let anchor = 50; // ~0.42 of 120
    let above = ramp.color_at(anchor - 6, height);
    let at = ramp.color_at(anchor, height);
    let below = ramp.color_at(anchor + 6, height);
    assert_ne!(above, at, "water must still darken entering the old anchor");
    assert_ne!(at, below, "water must still darken leaving the old anchor");
    assert_eq!(ramp.color_at(0, height), ramp.surface);
    assert_eq!(ramp.color_at(height - 1, height), ramp.deep);
}

#[test]
fn completion_breath_peaks_once_then_settles() {
    let ramp = OceanRamp::for_theme(&crate::palette::UI_THEME).expect("RGB theme");
    let start = ramp.color_at_completion(0, 20, 0);
    let peak = ramp.color_at_completion(0, 20, 320);
    let settled = ramp.color_at_completion(0, 20, 800);
    assert_ne!(start, peak);
    assert_ne!(peak, settled);
    assert_eq!(settled, ramp.color_at(0, 20));
}

#[test]
fn cache_fingerprint_changes_when_only_ramp_colors_change() {
    let viewport = Rect::new(3, 5, 80, 24);
    let first_ramp = OceanRamp {
        surface: Color::Rgb(1, 2, 3),
        middle: Color::Rgb(4, 5, 6),
        deep: Color::Rgb(7, 8, 9),
        ambient: Color::Rgb(10, 11, 12),
        attention: Color::Rgb(240, 180, 60),
        failure: Color::Rgb(220, 80, 80),
    };
    let second_ramp = OceanRamp {
        surface: Color::Rgb(21, 22, 23),
        middle: Color::Rgb(24, 25, 26),
        deep: Color::Rgb(27, 28, 29),
        ambient: Color::Rgb(30, 31, 32),
        attention: Color::Rgb(240, 180, 60),
        failure: Color::Rgb(220, 80, 80),
    };
    let first = OceanColumn::new(
        first_ramp,
        viewport,
        22_500,
        None,
        ShellPhase::Working,
        true,
        1000,
    );
    let second = OceanColumn::new(
        second_ramp,
        viewport,
        22_500,
        None,
        ShellPhase::Working,
        true,
        1000,
    );

    assert_ne!(first.color_at_y(viewport.y), second.color_at_y(viewport.y));
    assert_ne!(
        first.ramp_fingerprint(),
        second.ramp_fingerprint(),
        "visibly different palettes must not reuse the same row-color cache"
    );
}

#[test]
fn each_ramp_color_participates_in_the_typed_cache_identity() {
    let viewport = Rect::new(3, 5, 80, 24);
    let ramp = OceanRamp {
        surface: Color::Rgb(1, 2, 3),
        middle: Color::Rgb(4, 5, 6),
        deep: Color::Rgb(7, 8, 9),
        ambient: Color::Rgb(10, 11, 12),
        attention: Color::Rgb(240, 180, 60),
        failure: Color::Rgb(220, 80, 80),
    };
    let baseline = OceanColumn::new(
        ramp,
        viewport,
        22_500,
        None,
        ShellPhase::Working,
        true,
        1000,
    );
    let alternatives = [
        OceanRamp {
            surface: Color::Rgb(101, 2, 3),
            ..ramp
        },
        OceanRamp {
            middle: Color::Rgb(104, 5, 6),
            ..ramp
        },
        OceanRamp {
            deep: Color::Rgb(107, 8, 9),
            ..ramp
        },
        OceanRamp {
            ambient: Color::Rgb(110, 11, 12),
            ..ramp
        },
        OceanRamp {
            attention: Color::Rgb(255, 200, 90),
            ..ramp
        },
        OceanRamp {
            failure: Color::Rgb(255, 90, 90),
            ..ramp
        },
    ];

    for alternative in alternatives {
        let changed = OceanColumn::new(
            alternative,
            viewport,
            22_500,
            None,
            ShellPhase::Working,
            true,
            1000,
        );
        assert_ne!(
            baseline.ramp_cache_identity(),
            changed.ramp_cache_identity()
        );
        assert_ne!(baseline.ramp_fingerprint(), changed.ramp_fingerprint());
    }
}

#[test]
fn identical_semantic_cache_inputs_have_identical_identity() {
    let ramp = OceanRamp::for_theme(&crate::palette::UI_THEME).expect("RGB theme");
    let viewport = Rect::new(3, 5, 80, 24);
    let first = OceanColumn::new(
        ramp,
        viewport,
        22_500,
        None,
        ShellPhase::Working,
        true,
        1000,
    );
    let second = OceanColumn::new(
        ramp,
        viewport,
        22_500,
        None,
        ShellPhase::Working,
        true,
        1000,
    );

    assert_eq!(first.ramp_cache_identity(), second.ramp_cache_identity());
    assert_eq!(first.ramp_fingerprint(), second.ramp_fingerprint());
}

#[test]
fn split_shell_surfaces_share_one_absolute_row_column() {
    let theme = crate::palette::UI_THEME;
    let ramp = OceanRamp::for_theme(&theme).expect("RGB theme");
    let viewport = Rect::new(0, 0, 12, 12);
    let header = Rect::new(0, 0, 12, 2);
    let composer = Rect::new(0, 10, 12, 2);
    let mut buf = Buffer::empty(viewport);
    for y in header.top()..header.bottom() {
        for x in header.left()..header.right() {
            buf[(x, y)].set_bg(theme.header_bg);
        }
    }
    for y in composer.top()..composer.bottom() {
        for x in composer.left()..composer.right() {
            buf[(x, y)].set_bg(theme.composer_bg);
        }
    }
    buf[(4, 10)].set_bg(theme.selection_bg);

    let column = OceanColumn::new(ramp, viewport, 0, None, ShellPhase::Idle, false, 0);
    column.paint_matching(header, &mut buf, theme.header_bg);
    column.paint_matching(composer, &mut buf, theme.composer_bg);

    assert_eq!(buf[(0, 0)].bg, ramp.color_at(0, 12));
    assert_eq!(buf[(0, 11)].bg, ramp.color_at(11, 12));
    assert_ne!(buf[(0, 1)].bg, buf[(0, 10)].bg);
    assert_eq!(
        buf[(4, 10)].bg,
        theme.selection_bg,
        "semantic surfaces must survive the shell ombre pass"
    );
}

#[test]
fn full_viewport_water_column_reaches_both_terminal_edges() {
    let theme = crate::palette::UI_THEME;
    let ramp = OceanRamp::for_theme(&theme).expect("RGB theme");
    let viewport = Rect::new(0, 0, 120, 32);
    let mut buf = Buffer::empty(viewport);
    for y in viewport.top()..viewport.bottom() {
        for x in viewport.left()..viewport.right() {
            buf[(x, y)].set_bg(theme.surface_bg);
        }
    }
    buf[(60, 16)].set_bg(theme.selection_bg);

    let column = OceanColumn::new(ramp, viewport, 0, None, ShellPhase::Idle, false, 0);
    column.paint_matching(viewport, &mut buf, theme.surface_bg);

    for y in viewport.top()..viewport.bottom() {
        let expected = ramp.color_at(y, viewport.height);
        assert_eq!(buf[(viewport.left(), y)].bg, expected);
        assert_eq!(buf[(viewport.right() - 1, y)].bg, expected);
    }
    assert_eq!(
        buf[(60, 16)].bg,
        theme.selection_bg,
        "semantic surfaces must remain protected inside the full-width water column"
    );
}

// ---- v0.9.4: life presence eases the animated/static boundary ----

#[test]
fn life_presence_is_pure_and_bounded() {
    // Same inputs -> same output; presence never leaves 0..=1.
    let inputs = [
        (None, None, false, false, false),
        (None, None, true, false, false),
        (Some(0), Some(0), true, false, false),
        (Some(500), Some(10_000), false, false, false),
        (Some(2_000), None, true, false, false),
        (None, Some(30_000), true, true, false),
    ];
    for (completion, turn, animated, browsing, empty) in inputs {
        let a = life_presence(completion, turn, animated, browsing, empty);
        let b = life_presence(completion, turn, animated, browsing, empty);
        assert_eq!(a, b, "life_presence must be a pure function of its inputs");
        assert!(
            (0.0..=1.0).contains(&a),
            "presence must stay in 0..=1, got {a}"
        );
    }
}

#[test]
fn life_presence_holds_full_through_completion_breath_then_settles() {
    // During the breath the water keeps full life so the settle flourish is
    // accompanied by swimming fish, then presence eases out.
    assert_eq!(life_presence(Some(0), None, false, false, false), 1.0);
    assert_eq!(
        life_presence(Some(COMPLETION_BREATH_MS - 1), None, false, false, false),
        1.0
    );
    assert_eq!(
        life_presence(Some(COMPLETION_BREATH_MS), None, false, false, false),
        1.0,
        "presence holds at the breath boundary before easing"
    );
    let mid = life_presence(
        Some(COMPLETION_BREATH_MS + SETTLE_MS / 2),
        None,
        false,
        false,
        false,
    );
    assert!(
        mid > 0.0 && mid < 1.0,
        "presence must be mid-fade during the settle window, got {mid}"
    );
    assert_eq!(
        life_presence(
            Some(COMPLETION_BREATH_MS + SETTLE_MS),
            None,
            false,
            false,
            false
        ),
        0.0,
        "presence reaches zero at the end of the settle window"
    );
}

#[test]
fn life_presence_ramps_in_from_turn_anchor_with_bounded_velocity() {
    // Working/Verifying ramps in over RAMP_MS; the ramp is monotone and
    // zero-velocity at both ends (smoothstep), so bursty streams ease in.
    let at = |ms: u128| life_presence(None, Some(ms), true, false, false);
    assert_eq!(at(0), 0.0);
    assert_eq!(at(RAMP_MS / 2), 0.5, "smoothstep midpoint is 0.5");
    assert_eq!(at(RAMP_MS), 1.0);
    assert_eq!(at(RAMP_MS * 10), 1.0);
    // Monotone non-decreasing across the ramp.
    let mut prev = 0.0f32;
    for step in 1..=16u128 {
        let ms = step * RAMP_MS / 16;
        let value = at(ms);
        assert!(value >= prev, "presence must not regress while ramping");
        prev = value;
    }
}

#[test]
fn life_presence_user_driven_states_are_immediate() {
    // Browsing history and the pristine empty state are deliberate user
    // surfaces: full presence with no ramp.
    assert_eq!(life_presence(None, Some(0), true, true, false), 1.0);
    assert_eq!(life_presence(None, Some(0), true, false, true), 1.0);
    // Fully static contexts are exactly zero.
    assert_eq!(life_presence(None, None, false, false, false), 0.0);
    assert_eq!(life_presence(None, Some(50_000), false, false, false), 0.0);
}
