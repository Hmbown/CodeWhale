use super::*;
use ratatui::text::Span;

#[test]
fn ambient_min_dimensions_allow_small_windows() {
    const {
        assert!(AMBIENT_MIN_WIDTH < 68);
        assert!(AMBIENT_MIN_HEIGHT < 15);
    }
}

#[test]
fn occupied_text_bounds_skips_string_join() {
    let line = Line::from(vec![Span::raw("  hello  "), Span::raw("world  ")]);
    let (start, end) = occupied_text_bounds(&line).expect("bounds");
    assert_eq!(start, 2);
    assert!(end > start);
}

#[test]
fn whale_cameo_is_brief() {
    assert_eq!(whale_cameo_phase(0), WhaleCameoPhase::Breach);
    assert_eq!(whale_cameo_phase(500), WhaleCameoPhase::Spout);
    assert_eq!(whale_cameo_phase(1_200), WhaleCameoPhase::Fluke);
    assert_eq!(whale_cameo_phase(2_000), WhaleCameoPhase::Submerge);
    assert_eq!(whale_cameo_phase(3_000), WhaleCameoPhase::Hidden);
}

#[test]
fn density_scales_with_area() {
    assert_eq!(
        LifeDensity::from_area(Rect::new(0, 0, 40, 10)),
        LifeDensity::Sparse
    );
    assert_eq!(
        LifeDensity::from_area(Rect::new(0, 0, 100, 30)),
        LifeDensity::Rich
    );
}

#[test]
fn fish_school_uses_one_silhouette_family() {
    // Never mix lone `>` with full fish bodies; the lead just gains an
    // eye within the same family.
    assert_eq!(fish_body(true, false), "><>");
    assert_eq!(fish_body(false, false), "<><");
    assert_eq!(fish_body(true, true), "><o>");
    assert_eq!(fish_body(false, true), "<o><");
}

fn frame_at(t: u128) -> FrameMarks {
    let area = Rect::new(0, 0, 100, 30);
    let mut stats = AmbientFrameStats::default();
    build_frame_marks(
        area,
        t,
        LifeDensity::from_area(area),
        AmbientCursor::default(),
        WhaleCameo::default(),
        &mut stats,
    )
}

#[test]
fn fish_always_swim_the_way_they_face() {
    // Wrap-around construction: within one crossing, a right-facing
    // school only ever moves right, and a left-facing school only left.
    let area_travel = 100u128 + 21; // width + wedge span (see constants)
    let cycle_ms = area_travel * SCHOOL_CELL_MS;
    for cycle in 0u128..6 {
        // The school clock carries a half-cycle head start, so sampling
        // at cycle*cycle_ms lands mid-crossing of `cycle`.
        let t1 = cycle * cycle_ms;
        let t2 = t1 + SCHOOL_CELL_MS * 3;
        let lead = |t: u128| {
            frame_at(t)
                .marks
                .into_iter()
                .find(|mark| mark.glyph.contains('o'))
        };
        let (Some(a), Some(b)) = (lead(t1), lead(t2)) else {
            continue; // school off-screen at this sample — fine
        };
        let expect_right = school_swims_right(cycle);
        if expect_right {
            assert_eq!(a.glyph, "><o>", "cycle {cycle} facing");
            assert!(b.x >= a.x, "cycle {cycle}: right-facing fish moved left");
        } else {
            assert_eq!(a.glyph, "<o><", "cycle {cycle} facing");
            assert!(b.x <= a.x, "cycle {cycle}: left-facing fish moved right");
        }
    }
    // Both directions must actually occur across nearby cycles.
    let dirs: Vec<bool> = (0u128..12).map(school_swims_right).collect();
    assert!(
        dirs.iter().any(|d| *d) && dirs.iter().any(|d| !*d),
        "{dirs:?}"
    );
}

#[test]
fn water_holds_only_fish_bubbles_and_jellyfish() {
    // Seaweed and bio-dust are gone (2026-07-23): every mark is a fish
    // body, a bubble glyph, or a jellyfish part.
    for t in [0u128, 7_500, 33_000, 61_000, 120_000] {
        for mark in frame_at(t).marks {
            let ok = matches!(mark.glyph, "><>" | "<><" | "><o>" | "<o><")
                || matches!(mark.glyph, "·" | "˚" | "°")
                || JELLY_DOME_TOP_FRAMES.contains(&mark.glyph)
                || JELLY_DOME_SKIRT_FRAMES.contains(&mark.glyph)
                || JELLY_DOME_TOP_COMPACT.contains(&mark.glyph)
                || JELLY_DOME_SKIRT_COMPACT.contains(&mark.glyph)
                || JELLY_TENTACLE_FRAMES.contains(&mark.glyph);
            assert!(ok, "unexpected ambient glyph {:?} at t={t}", mark.glyph);
        }
    }
}

#[test]
fn ambient_glyphs_are_ascii_or_have_fallbacks() {
    // Every glyph the water can paint is either pure ASCII (fish and
    // the whole jellyfish silhouette, by construction) or carries a
    // glyphs::ascii_fallback entry (bubbles, whale cameo) so
    // CODEWHALE_ASCII_SAFE=1 covers the whole field.
    let mut jellyfish: Vec<&str> = Vec::new();
    jellyfish.extend(JELLY_DOME_TOP_FRAMES);
    jellyfish.extend(JELLY_DOME_SKIRT_FRAMES);
    jellyfish.extend(JELLY_DOME_TOP_COMPACT);
    jellyfish.extend(JELLY_DOME_SKIRT_COMPACT);
    jellyfish.extend(JELLY_TENTACLE_FRAMES);
    for glyph in jellyfish {
        assert!(glyph.is_ascii(), "jellyfish glyph {glyph:?} must be ASCII");
    }
    for glyph in ["><>", "<><", "><o>", "<o><"] {
        assert!(glyph.is_ascii(), "fish glyph {glyph:?} must be ASCII");
    }
    for glyph in ["·", "˚", "°", "≈≈>", "≈", "～"] {
        assert!(
            glyph.is_ascii() || crate::tui::glyphs::ascii_fallback(glyph).is_some(),
            "ambient glyph {glyph:?} lacks an ASCII fallback"
        );
    }
}

#[test]
fn jellyfish_reads_as_dome_with_lagging_tentacles() {
    // Find a frame where a full jelly is on-screen and assert its
    // structure: a two-row dome at least four cells wide, exactly three
    // tentacle columns one row below the skirt, and both dome and
    // tentacles holding their brightness floors.
    let mut seen = false;
    for probe in 0..240u128 {
        let t = probe * 500;
        let frame = frame_at(t);
        let Some(top) = frame
            .marks
            .iter()
            .find(|mark| JELLY_DOME_TOP_FRAMES.contains(&mark.glyph))
        else {
            continue;
        };
        let dome_w = UnicodeWidthStr::width(top.glyph) as u16;
        assert!(dome_w >= 4, "rich dome too narrow: {:?}", top.glyph);
        let Some(skirt) = frame.marks.iter().find(|mark| {
            JELLY_DOME_SKIRT_FRAMES.contains(&mark.glyph) && mark.x == top.x && mark.y == top.y + 1
        }) else {
            panic!("visible jellyfish dome lost its skirt: {frame:?}");
        };
        let tentacles: Vec<&AmbientMark> = frame
            .marks
            .iter()
            .filter(|mark| {
                JELLY_TENTACLE_FRAMES.contains(&mark.glyph)
                    && mark.y == skirt.y + 1
                    && mark.x > top.x
                    && mark.x < top.x + dome_w
            })
            .collect();
        assert_eq!(
            tentacles.len(),
            3,
            "visible jellyfish must keep all tentacles: {frame:?}"
        );
        let dome_glow = top.brightness.expect("dome pulses");
        assert!(dome_glow >= JELLY_BRIGHTNESS_FLOOR - f32::EPSILON);
        for tentacle in &tentacles {
            let glow = tentacle.brightness.expect("tentacle pulses");
            assert!(glow >= JELLY_BRIGHTNESS_FLOOR - f32::EPSILON);
        }
        seen = true;
        break;
    }
    assert!(seen, "no complete jellyfish found in 120s of frames");
}

#[test]
fn jellyfish_tentacles_sway_out_of_phase_and_dome_pulses() {
    // Over a sweep: the dome shows both pulse frames within a few
    // JELLY_PULSE_MS periods, each tentacle column cycles its sway
    // frames, and the trio is not always in lockstep (per-column phase
    // offset — the lag is what sells "jellyfish").
    let mut dome_frames = std::collections::BTreeSet::new();
    let mut column_frames: [std::collections::BTreeSet<&str>; 3] = Default::default();
    let mut saw_desync = false;
    for probe in 0..480u128 {
        let frame = frame_at(probe * 100);
        let Some(top) = frame
            .marks
            .iter()
            .find(|mark| JELLY_DOME_TOP_FRAMES.contains(&mark.glyph))
        else {
            continue;
        };
        dome_frames.insert(top.glyph);
        let mut trio = [""; 3];
        let mut found = 0usize;
        for (col, slot) in trio.iter_mut().enumerate() {
            if let Some(tentacle) = frame.marks.iter().find(|mark| {
                JELLY_TENTACLE_FRAMES.contains(&mark.glyph)
                    && mark.y == top.y + 2
                    && mark.x == top.x + 1 + col as u16
            }) {
                *slot = tentacle.glyph;
                column_frames[col].insert(tentacle.glyph);
                found += 1;
            }
        }
        if found == 3 && !(trio[0] == trio[1] && trio[1] == trio[2]) {
            saw_desync = true;
        }
    }
    assert_eq!(
        dome_frames.len(),
        JELLY_DOME_TOP_FRAMES.len(),
        "dome pulse should show every frame: {dome_frames:?}"
    );
    for (col, set) in column_frames.iter().enumerate() {
        assert!(set.len() > 1, "tentacle column {col} never swayed");
    }
    assert!(saw_desync, "tentacle columns strobed in lockstep");
}

#[test]
fn sparse_water_gets_a_compact_jellyfish() {
    // Narrow fallback: the Sparse tier swaps the full 5-cell dome for a
    // 3-cell one with two tentacles — a different silhouette, not just
    // fewer jellies.
    let area = Rect::new(0, 0, 48, 12);
    let mut saw_compact = false;
    let mut saw_full = false;
    let mut saw_two_tentacles = false;
    for probe in 0..240u128 {
        let mut stats = AmbientFrameStats::default();
        let frame = build_frame_marks(
            area,
            probe * 500,
            LifeDensity::from_area(area),
            AmbientCursor::default(),
            WhaleCameo::default(),
            &mut stats,
        );
        for mark in &frame.marks {
            saw_compact |= JELLY_DOME_TOP_COMPACT.contains(&mark.glyph);
            saw_full |= JELLY_DOME_TOP_FRAMES.contains(&mark.glyph);
        }
        let Some(top) = frame
            .marks
            .iter()
            .find(|mark| JELLY_DOME_TOP_COMPACT.contains(&mark.glyph))
        else {
            continue;
        };
        let tentacles = frame
            .marks
            .iter()
            .filter(|mark| JELLY_TENTACLE_FRAMES.contains(&mark.glyph) && mark.y == top.y + 2)
            .count();
        assert_eq!(
            tentacles, 2,
            "visible compact jelly must keep both tentacles: {frame:?}"
        );
        saw_two_tentacles = true;
    }
    assert!(saw_compact, "sparse water never showed the compact dome");
    assert!(!saw_full, "sparse water used the full-size dome");
    assert!(saw_two_tentacles, "compact jelly lost its tentacles");
}

#[test]
fn animated_tentacle_frames_never_collapse_to_punctuation_dots() {
    assert!(
        JELLY_TENTACLE_FRAMES
            .iter()
            .all(|glyph| matches!(*glyph, "|" | "/" | "\\")),
        "every animation frame must retain a legible tentacle stroke"
    );
}

#[test]
fn motion_is_a_deterministic_function_of_elapsed_time() {
    // v0.9.4: positions always ride the monotonic clock (presence fades the
    // marks in/out instead of parking them), so a fixed t must produce the
    // exact same complete jellyfish: dome, skirt, and tentacles visible.
    let area = Rect::new(0, 0, 100, 30);
    let build = || {
        let mut stats = AmbientFrameStats::default();
        build_frame_marks(
            area,
            0,
            LifeDensity::from_area(area),
            AmbientCursor::default(),
            WhaleCameo::default(),
            &mut stats,
        )
    };
    let first = build();
    let second = build();
    let pose = |frame: &FrameMarks| {
        frame
            .marks
            .iter()
            .map(|mark| (mark.glyph, mark.x, mark.y))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        pose(&first),
        pose(&second),
        "motion must be a pure function of t"
    );
    let top = first
        .marks
        .iter()
        .find(|mark| JELLY_DOME_TOP_FRAMES.contains(&mark.glyph))
        .expect("jellyfish dome at t=0");
    assert!(
        first
            .marks
            .iter()
            .any(|mark| { JELLY_DOME_SKIRT_FRAMES.contains(&mark.glyph) && mark.y == top.y + 1 }),
        "jellyfish lost its skirt at t=0"
    );
    let tentacles = first
        .marks
        .iter()
        .filter(|mark| JELLY_TENTACLE_FRAMES.contains(&mark.glyph) && mark.y == top.y + 2)
        .count();
    assert!(tentacles >= 3, "jellyfish lost its tentacles at t=0");
}

#[test]
fn glow_helpers_stay_bounded_with_floors() {
    for t in (0u128..12_000).step_by(97) {
        let w = wave01(t, FISH_WAVE_MS, 0);
        assert!((0.0..=1.0).contains(&w), "wave01 out of range: {w}");
        let g = glint01(t, 2_600, 600, BUBBLE_BRIGHTNESS_FLOOR, 0);
        assert!(
            (BUBBLE_BRIGHTNESS_FLOOR..=1.0).contains(&g),
            "glint01 lost its floor: {g}"
        );
    }
}

#[test]
fn frame_stats_account_for_every_mark() {
    let area = Rect::new(0, 0, 100, 30);
    let mut buf = Buffer::empty(area);
    let stats = render_ambient_life(
        area,
        &mut buf,
        (Color::Cyan, Color::Blue),
        &[],
        12_000,
        1.0,
        AmbientCursor::default(),
        WhaleCameo::default(),
    );
    assert_eq!(
        stats.marks_built,
        stats.marks_painted + stats.marks_skipped_text + stats.marks_clipped,
        "every built mark is painted, text-skipped, or clipped: {stats:?}"
    );
    assert!(stats.marks_painted > 0, "empty water should paint life");
    // cells_written counts each cell of a multi-cell glyph (and counts
    // a shared cell once per overlapping mark), so the honest bound is
    // per painted mark — at most the widest glyph (the 5-cell dome) —
    // rather than per area cell.
    assert!(stats.cells_written >= stats.marks_painted);
    assert!(
        stats.cells_written <= stats.marks_painted * 5,
        "cells_written out of proportion: {stats:?}"
    );
}

#[test]
fn frame_stats_stay_within_the_render_budget() {
    // Worst case on the largest Rich field with the whale cameo active:
    // 7 fish + 2 jellies x 5 parts + 2 bubbles + 2 cameo cells. Anything
    // more is a leak in the O(1)-per-frame budget.
    let area = Rect::new(0, 0, 160, 40);
    let mut buf = Buffer::empty(area);
    let whale = WhaleCameo {
        elapsed_ms: Some(500), // Spout: cameo glyph plus spray
        anchor_x: 80,
        anchor_y: 26,
    };
    let stats = render_ambient_life(
        area,
        &mut buf,
        (Color::Cyan, Color::Blue),
        &[],
        12_000,
        1.0,
        AmbientCursor::default(),
        whale,
    );
    assert!(
        stats.marks_built <= MAX_FRAME_MARKS,
        "frame budget blown: {stats:?}"
    );
    assert_eq!(
        stats.marks_built,
        stats.marks_painted + stats.marks_skipped_text + stats.marks_clipped,
        "every built mark is accounted for: {stats:?}"
    );
}

#[test]
fn frame_stats_never_overwrite_text() {
    // Property: on a text-covered field, colliding marks are charged to
    // marks_skipped_text and no transcript cell is overwritten.
    let area = Rect::new(0, 0, 100, 30);
    let mut buf = Buffer::empty(area);
    let lines: Vec<Line<'static>> = (0..usize::from(area.height))
        .map(|i| {
            Line::from(Span::raw(format!(
                "transcript row {i:02} occupies the water"
            )))
        })
        .collect();
    for (i, line) in lines.iter().enumerate() {
        buf.set_line(area.x, area.y + i as u16, line, area.width);
    }
    let stats = render_ambient_life(
        area,
        &mut buf,
        (Color::Cyan, Color::Blue),
        &lines,
        12_000,
        1.0,
        AmbientCursor::default(),
        WhaleCameo::default(),
    );
    assert!(
        stats.marks_skipped_text > 0,
        "text-covered water should skip some marks: {stats:?}"
    );
    assert_eq!(
        stats.marks_built,
        stats.marks_painted + stats.marks_skipped_text + stats.marks_clipped,
        "every built mark is accounted for: {stats:?}"
    );
    for (i, line) in lines.iter().enumerate() {
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        for (x, ch) in text.chars().enumerate() {
            assert_eq!(
                buf[(area.x + x as u16, area.y + i as u16)].symbol(),
                ch.to_string(),
                "ambient life overwrote transcript cell ({x},{i})"
            );
        }
    }
}

fn full_jellyfish_frame(x: u16, y: u16) -> FrameMarks {
    let mark = |x, y, glyph, depth| AmbientMark {
        x,
        y,
        glyph,
        jellyfish: Some(0),
        depth,
        style_mod: None,
        brightness: None,
    };
    FrameMarks {
        marks: vec![
            mark(x, y, JELLY_DOME_TOP_FRAMES[0], Depth::Midground),
            mark(x, y + 1, JELLY_DOME_SKIRT_FRAMES[0], Depth::Midground),
            mark(x + 1, y + 2, "|", Depth::Background),
            mark(x + 2, y + 2, "/", Depth::Background),
            mark(x + 3, y + 2, "\\", Depth::Background),
        ],
    }
}

fn paint_fixture(
    area: Rect,
    lines: &[Line<'static>],
    frame: &FrameMarks,
) -> (Buffer, AmbientFrameStats) {
    let mut buf = Buffer::empty(area);
    for (row, line) in lines.iter().enumerate() {
        buf.set_line(area.x, area.y + row as u16, line, area.width);
    }
    let mut stats = AmbientFrameStats {
        marks_built: frame.marks.len() as u32,
        ..AmbientFrameStats::default()
    };
    paint_marks(
        area,
        &mut buf,
        (Color::Cyan, Color::Blue),
        lines,
        frame,
        1.0,
        &mut stats,
    );
    (buf, stats)
}

#[test]
fn jellyfish_relocates_as_one_nearest_silhouette() {
    let area = Rect::new(0, 0, 24, 8);
    let mut lines = vec![Line::default(); usize::from(area.height)];
    lines[2] = Line::from(Span::raw("          X"));
    let (buf, stats) = paint_fixture(area, &lines, &full_jellyfish_frame(10, 2));

    assert_eq!(buf[(10, 2)].symbol(), "X");
    assert_eq!(buf[(12, 2)].symbol(), ".");
    assert_eq!(buf[(12, 3)].symbol(), "\\");
    assert_eq!(buf[(13, 4)].symbol(), "|");
    assert_eq!(buf[(14, 4)].symbol(), "/");
    assert_eq!(buf[(15, 4)].symbol(), "\\");
    assert_eq!(stats.marks_painted, 5);
    assert_eq!(stats.marks_skipped_text, 0);
    assert_eq!(stats.marks_clipped, 0);
    assert_eq!(stats.cells_written, 13);
}

#[test]
fn jellyfish_uses_the_nearest_protected_edge_at_x33() {
    let area = Rect::new(0, 0, 100, 8);
    let mut lines = vec![Line::default(); usize::from(area.height)];
    lines[2] = Line::from(Span::raw("X".repeat(32)));
    let (buf, stats) = paint_fixture(area, &lines, &full_jellyfish_frame(16, 2));

    assert_eq!(buf[(31, 2)].symbol(), "X");
    assert_eq!(buf[(32, 2)].symbol(), " ");
    assert_eq!(buf[(33, 2)].symbol(), ".");
    assert_eq!(buf[(33, 3)].symbol(), "\\");
    assert_eq!(buf[(34, 4)].symbol(), "|");
    assert_eq!(stats.marks_painted, 5);
    assert_eq!(stats.marks_skipped_text, 0);
    assert_eq!(stats.marks_clipped, 0);
}

#[test]
fn sparse_jellyfish_row_keeps_a_safe_gap_over_text() {
    let area = Rect::new(0, 0, 12, 6);
    let mut lines = vec![Line::default(); usize::from(area.height)];
    lines[2] = Line::from(Span::raw("   X"));
    let mark = |x| AmbientMark {
        x,
        y: 2,
        glyph: "|",
        jellyfish: Some(0),
        depth: Depth::Background,
        style_mod: None,
        brightness: None,
    };
    let frame = FrameMarks {
        marks: vec![mark(0), mark(6)],
    };
    let (buf, stats) = paint_fixture(area, &lines, &frame);

    assert_eq!(buf[(0, 2)].symbol(), "|");
    assert_eq!(buf[(3, 2)].symbol(), "X");
    assert_eq!(buf[(6, 2)].symbol(), "|");
    assert_eq!(stats.marks_painted, 2);
}

#[test]
fn fully_blocked_jellyfish_is_suppressed_and_accounted() {
    let area = Rect::new(0, 0, 24, 8);
    let mut lines = vec![Line::default(); usize::from(area.height)];
    for line in lines.iter_mut().take(5).skip(2) {
        *line = Line::from(Span::raw("X".repeat(24)));
    }
    let (buf, stats) = paint_fixture(area, &lines, &full_jellyfish_frame(10, 2));

    assert_eq!(stats.marks_painted, 0);
    assert_eq!(stats.marks_skipped_text, 5);
    assert_eq!(stats.marks_clipped, 0);
    assert_eq!(stats.cells_written, 0);
    assert_eq!(stats.marks_built, stats.marks_skipped_text);
    for row in 2..=4 {
        for column in 0..area.width {
            assert_eq!(buf[(column, row)].symbol(), "X");
        }
    }
}

#[test]
fn too_wide_jellyfish_is_truthfully_clipped_as_a_group() {
    let area = Rect::new(0, 0, 4, 8);
    let lines = vec![Line::default(); usize::from(area.height)];
    let (_, stats) = paint_fixture(area, &lines, &full_jellyfish_frame(0, 2));

    assert_eq!(stats.marks_painted, 0);
    assert_eq!(stats.marks_skipped_text, 0);
    assert_eq!(stats.marks_clipped, 5);
    assert_eq!(stats.marks_built, stats.marks_clipped);
}
