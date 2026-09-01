//! The Codewhale mark — the identity silhouette, as cells.
//!
//! Side-view whale facing left, `>` prompt-eye, upward fluke. The old
//! symmetric fluke read as a fish at 16px; this mark cannot be mirrored
//! about its vertical axis.
//!
//! ASCII rungs are drawn, not transliterated — `glyphs::ascii_fallback` maps
//! every block glyph to `#`, which would flatten the whale to a blob.

use ratatui::{buffer::Buffer, layout::Rect, style::Color, style::Style};

/// Rungs of the mark's scale ladder, each sampled at its own size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkSize {
    /// 9x4 — the smallest rung that still reads as a side-view whale.
    Small,
    /// 13x6 — the default hero mark.
    Medium,
    /// 19x8 — full hero mark for tall stages.
    Large,
}

const SMALL_ROWS: [&str; 4] = [
    "▄█████>  ", //
    "████████▄",
    "▀██▀ ▀██ ",
    "  ▀▀▀    ",
];

const MEDIUM_ROWS: [&str; 6] = [
    "  ▄██████>   ", //
    "▄██████████▄ ",
    "█████████████",
    "▀███▀   ▀██▀ ",
    "   ▀▀▀▀▀     ",
    "      ▀      ",
];

const LARGE_ROWS: [&str; 8] = [
    "   ▄████████>      ", //
    "▄████████████████▄ ",
    "███████████████████",
    "▀████▀     ▀████▀  ",
    "    ▀▀▀▀▀▀▀        ",
    "       ▀▀▀         ",
    "        ▀          ",
    "                   ",
];

/// `'` and `,` carry the half-cell the block glyphs carried, so lobes slope.
const SMALL_ASCII: [&str; 4] = [
    ",#####>  ", //
    "########,",
    "'##' '## ",
    "  '''    ",
];

const MEDIUM_ASCII: [&str; 6] = [
    "  ,######>   ", //
    ",##########, ",
    "#############",
    "'###'   '##' ",
    "   '''''     ",
    "      '      ",
];

const LARGE_ASCII: [&str; 8] = [
    "   ,########>      ", //
    ",################, ",
    "###################",
    "'####'     '####'  ",
    "    '''''''        ",
    "       '''         ",
    "        '          ",
    "                   ",
];

impl MarkSize {
    /// Cell footprint as `(cols, rows)`.
    #[must_use]
    pub const fn cells(self) -> (u16, u16) {
        match self {
            Self::Small => (9, 4),
            Self::Medium => (13, 6),
            Self::Large => (19, 8),
        }
    }

    /// The rows for this rung.
    #[must_use]
    pub const fn rows(self, ascii_safe: bool) -> &'static [&'static str] {
        match (self, ascii_safe) {
            (Self::Small, false) => &SMALL_ROWS,
            (Self::Medium, false) => &MEDIUM_ROWS,
            (Self::Large, false) => &LARGE_ROWS,
            (Self::Small, true) => &SMALL_ASCII,
            (Self::Medium, true) => &MEDIUM_ASCII,
            (Self::Large, true) => &LARGE_ASCII,
        }
    }

    /// Largest rung fitting `area` with `reserve_rows` left beneath it.
    /// `None` → type alone; a clipped fluke reads as a different animal.
    #[must_use]
    pub fn for_area(area: Rect, reserve_rows: u16) -> Option<Self> {
        let usable_h = area.height.saturating_sub(reserve_rows);
        [Self::Large, Self::Medium, Self::Small]
            .into_iter()
            .find(|size| {
                let (w, h) = size.cells();
                area.width >= w && usable_h >= h
            })
    }
}

/// Blend `from` toward `to` (0.0 → `from`, 1.0 → `to`). At 0 the mark is
/// exactly the field colour, so it rises out of the water rather than over it.
#[must_use]
pub fn lerp_color(from: Color, to: Color, amount: f32) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    match (from, to) {
        (Color::Rgb(fr, fg, fb), Color::Rgb(tr, tg, tb)) => Color::Rgb(
            lerp_channel(fr, tr, amount),
            lerp_channel(fg, tg, amount),
            lerp_channel(fb, tb, amount),
        ),
        // Indexed colours have no channels to interpolate; snap at midpoint.
        _ => {
            if amount >= 0.5 {
                to
            } else {
                from
            }
        }
    }
}

fn lerp_channel(from: u8, to: u8, amount: f32) -> u8 {
    let from = f32::from(from);
    let to = f32::from(to);
    (from + (to - from) * amount).round().clamp(0.0, 255.0) as u8
}

/// Raised-cosine rise over `duration_ms`, saturating at 1.0. Wall-clock keyed,
/// so a dropped frame costs smoothness, never correctness.
#[must_use]
pub fn surface_progress(elapsed_ms: u128, duration_ms: u128) -> f32 {
    if duration_ms == 0 || elapsed_ms >= duration_ms {
        return 1.0;
    }
    let t = elapsed_ms as f32 / duration_ms as f32;
    0.5 * (1.0 - (std::f32::consts::PI * t).cos())
}

/// Paint the fluke centred in `area`, in `ink`, emerging from `field`.
/// `progress` is `[0,1]`; reduced motion passes `1.0`, so the still frame is
/// this same drawing at its endpoint and the two cannot drift. Returns the
/// painted rect.
pub fn render_fluke(
    area: Rect,
    buf: &mut Buffer,
    size: MarkSize,
    ink: Color,
    field: Color,
    progress: f32,
    ascii_safe: bool,
) -> Rect {
    let rows = size.rows(ascii_safe);
    let (cols, row_count) = size.cells();
    if area.width < cols || area.height < row_count {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let x0 = area.x + (area.width - cols) / 2;
    let color = lerp_color(field, ink, progress);
    for (index, line) in rows.iter().enumerate() {
        let y = area.y + index as u16;
        if y >= area.bottom() {
            break;
        }
        // Skip blanks rather than pad: they must not erase the field behind.
        for (x, glyph) in (x0..).zip(line.chars()) {
            if x >= area.right() {
                break;
            }
            if glyph != ' '
                && let Some(cell) = buf.cell_mut((x, y))
            {
                cell.set_symbol(&glyph.to_string());
                cell.set_style(Style::default().fg(color));
            }
        }
    }
    Rect::new(x0, area.y, cols, row_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rung_matches_its_declared_footprint() {
        for size in [MarkSize::Small, MarkSize::Medium, MarkSize::Large] {
            let (cols, rows) = size.cells();
            for ascii_safe in [false, true] {
                let art = size.rows(ascii_safe);
                assert_eq!(
                    art.len(),
                    usize::from(rows),
                    "{size:?} ascii={ascii_safe} row count"
                );
                for line in art {
                    assert!(
                        line.chars().count() <= usize::from(cols),
                        "{size:?} ascii={ascii_safe} row {line:?} exceeds {cols} cols"
                    );
                }
            }
        }
    }

    #[test]
    fn ascii_rungs_have_the_same_silhouette_as_the_block_rungs() {
        // Different drawing, same cells — else the mark shifts on downgrade.
        for size in [MarkSize::Small, MarkSize::Medium, MarkSize::Large] {
            let block = size.rows(false);
            let ascii = size.rows(true);
            for (b, a) in block.iter().zip(ascii.iter()) {
                let b_cells: Vec<bool> = b.chars().map(|c| c != ' ').collect();
                let a_cells: Vec<bool> = a.chars().map(|c| c != ' ').collect();
                assert_eq!(b_cells, a_cells, "{size:?}: {b:?} vs {a:?}");
            }
        }
    }

    #[test]
    fn ascii_rungs_use_no_glyph_the_ascii_lane_would_rewrite() {
        for size in [MarkSize::Small, MarkSize::Medium, MarkSize::Large] {
            for line in size.rows(true) {
                for glyph in line.chars() {
                    assert!(
                        glyph.is_ascii(),
                        "{size:?} ascii rung leaks non-ASCII {glyph:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_mark_has_a_prompt_eye() {
        for size in [MarkSize::Small, MarkSize::Medium, MarkSize::Large] {
            for ascii_safe in [false, true] {
                let art = size.rows(ascii_safe);
                assert!(
                    art.iter().any(|line| line.contains('>')),
                    "{size:?} ascii={ascii_safe} is missing the `>` prompt-eye"
                );
            }
        }
    }

    #[test]
    fn for_area_never_returns_a_rung_that_would_clip() {
        for width in 0u16..40 {
            for height in 0u16..20 {
                let area = Rect::new(0, 0, width, height);
                if let Some(size) = MarkSize::for_area(area, 3) {
                    let (cols, rows) = size.cells();
                    assert!(width >= cols, "{size:?} too wide for {width}");
                    assert!(
                        height.saturating_sub(3) >= rows,
                        "{size:?} too tall for {height}"
                    );
                }
            }
        }
    }

    #[test]
    fn for_area_prefers_the_largest_rung_that_fits() {
        assert_eq!(
            MarkSize::for_area(Rect::new(0, 0, 40, 20), 3),
            Some(MarkSize::Large)
        );
        assert_eq!(
            MarkSize::for_area(Rect::new(0, 0, 16, 12), 3),
            Some(MarkSize::Medium)
        );
        assert_eq!(
            MarkSize::for_area(Rect::new(0, 0, 10, 8), 3),
            Some(MarkSize::Small)
        );
        assert_eq!(MarkSize::for_area(Rect::new(0, 0, 8, 8), 3), None);
    }

    #[test]
    fn surface_progress_is_a_monotonic_rise_that_settles_at_one() {
        assert!((surface_progress(0, 640) - 0.0).abs() < 1e-6);
        assert!((surface_progress(320, 640) - 0.5).abs() < 1e-3);
        assert!((surface_progress(640, 640) - 1.0).abs() < 1e-6);
        assert!((surface_progress(5_000, 640) - 1.0).abs() < 1e-6);
        let mut previous = -1.0;
        for ms in (0..=640).step_by(20) {
            let value = surface_progress(ms, 640);
            assert!(value >= previous, "regressed at {ms}ms");
            previous = value;
        }
    }

    #[test]
    fn a_zero_length_rise_is_settled_not_divided_by_zero() {
        assert!((surface_progress(0, 0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_mark_emerges_from_the_field_rather_than_fading_over_it() {
        let field = Color::Rgb(3, 7, 13);
        let gold = Color::Rgb(246, 196, 83);
        assert_eq!(lerp_color(field, gold, 0.0), field);
        assert_eq!(lerp_color(field, gold, 1.0), gold);
        let mid = lerp_color(field, gold, 0.5);
        assert_eq!(mid, Color::Rgb(125, 102, 48));
    }

    #[test]
    fn render_centres_the_mark_and_leaves_blanks_untouched() {
        let area = Rect::new(0, 0, 21, 8);
        let mut buf = Buffer::empty(area);
        for x in 0..21u16 {
            for y in 0..8u16 {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol("~");
                }
            }
        }
        let painted = render_fluke(
            area,
            &mut buf,
            MarkSize::Medium,
            Color::Rgb(246, 196, 83),
            Color::Rgb(3, 7, 13),
            1.0,
            false,
        );
        assert_eq!(painted, Rect::new(4, 0, 13, 6));
        // Row 0 is `  ▄██████>   `; leading blanks keep the field.
        assert_eq!(buf.cell((4, 0)).map(|c| c.symbol()), Some("~"));
        assert_eq!(buf.cell((5, 0)).map(|c| c.symbol()), Some("~"));
        assert_eq!(buf.cell((6, 0)).map(|c| c.symbol()), Some("▄"));
        assert_eq!(buf.cell((13, 0)).map(|c| c.symbol()), Some(">"));
        assert_eq!(buf.cell((14, 0)).map(|c| c.symbol()), Some("~"));
    }

    #[test]
    fn render_declines_rather_than_clipping_when_the_area_is_too_small() {
        let area = Rect::new(0, 0, 8, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 3));
        let painted = render_fluke(
            area,
            &mut buf,
            MarkSize::Medium,
            Color::Rgb(246, 196, 83),
            Color::Rgb(3, 7, 13),
            1.0,
            false,
        );
        assert_eq!(painted.width, 0);
        assert_eq!(buf.cell((0, 0)).map(|c| c.symbol()), Some(" "));
    }
}
