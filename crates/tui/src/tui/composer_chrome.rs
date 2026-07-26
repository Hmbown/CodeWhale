//! Ocean composer chrome policy.
//!
//! The composer auto-fits its content: one input row when empty or
//! single-line, growing with typed content up to the density cap. Density
//! no longer forces a multi-row baseline — it only bounds how tall the
//! composer may grow. Content-driven growth still wins once the user
//! types past one row, and submit/clear collapses the composer back to
//! a single input row.

use crate::tui::app::ComposerDensity;

/// Top/bottom chrome rows for the quiet rule (TOP border only) or the
/// enclosed panel (TOP + BOTTOM), plus the total-row growth cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposerChrome {
    pub border_rows: u16,
    pub max_total_rows: u16,
}

impl ComposerChrome {
    /// Baseline for the given density. Panel shape gets both borders;
    /// quiet shape keeps a single top rule so the prompt still has a
    /// clear ledge without reading as a card. Density picks the growth
    /// cap only — the composer starts at one content row regardless.
    #[must_use]
    pub fn for_density(density: ComposerDensity, enclosed_panel: bool) -> Self {
        let border_rows = if enclosed_panel { 2 } else { 1 };
        let max_total_rows = match density {
            ComposerDensity::Compact => 7,
            ComposerDensity::Comfortable => 9,
            ComposerDensity::Spacious => 12,
        };
        Self {
            border_rows,
            max_total_rows,
        }
    }
}

/// Decide how many rows the composer should occupy.
///
/// The height follows the content: one input row when the composer is
/// empty or holds a single line, growing one row per content line up to
/// the density cap (`max_total_rows`) or the available height, whichever
/// is smaller. Menu rows and the border chrome add on top. Compact
/// terminals shed the border before they shed typed content.
#[must_use]
pub fn desired_height(
    content_lines: usize,
    extra_menu_lines: usize,
    available_height: u16,
    density: ComposerDensity,
    enclosed_panel: bool,
) -> u16 {
    let chrome = ComposerChrome::for_density(density, enclosed_panel);
    let available = available_height.max(1);
    let content = content_lines.max(1);
    let wants_panel = enclosed_panel && available >= 3;

    let border = if wants_panel {
        usize::from(chrome.border_rows)
    } else if available >= 2 {
        1
    } else {
        0
    };

    let total = content
        .saturating_add(extra_menu_lines)
        .saturating_add(border);
    let max_height = usize::from(available.min(chrome.max_total_rows).max(1));
    total.clamp(1, max_height).try_into().unwrap_or(1)
}

/// Top padding inside the content budget. Keep at least one quiet row below a
/// short prompt when the budget has room, instead of bottom-pinning
/// the caret directly against the phase footer. Compact heights naturally
/// report zero padding once the budget collapses.
#[must_use]
pub fn top_padding(content_lines: usize, rows_budget: usize) -> usize {
    let content = content_lines.max(1).min(rows_budget.max(1));
    let spare = rows_budget.saturating_sub(content);
    spare.saturating_add(1) / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_composer_fits_one_input_row_plus_chrome() {
        // Auto-fit: an empty/single-line composer takes exactly one input
        // row plus the quiet-rule border, regardless of density.
        let height = desired_height(1, 0, 8, ComposerDensity::Comfortable, false);
        assert_eq!(height, 2, "1 content row + 1 border row");
    }

    #[test]
    fn compact_height_sheds_border_before_content() {
        // Only two rows available: keep a border + one content row.
        let height = desired_height(1, 0, 2, ComposerDensity::Comfortable, false);
        assert_eq!(height, 2);
    }

    #[test]
    fn content_growth_expands_up_to_the_density_cap() {
        // Six content rows + border fits under the Comfortable cap of 9.
        let height = desired_height(6, 0, 12, ComposerDensity::Comfortable, false);
        assert_eq!(height, 7, "typed content must grow the composer: {height}");

        // Past the cap the density setting wins, not the content.
        let capped = desired_height(20, 0, 30, ComposerDensity::Comfortable, false);
        assert_eq!(capped, 9, "Comfortable caps total rows at 9");
        let spacious = desired_height(20, 0, 30, ComposerDensity::Spacious, false);
        assert_eq!(spacious, 12, "Spacious caps total rows at 12");
    }

    #[test]
    fn single_line_panel_is_one_input_row_plus_both_borders() {
        let height = desired_height(1, 0, 12, ComposerDensity::Spacious, true);
        assert_eq!(height, 3, "panel = 2 borders + 1 content row, got {height}");
    }
}
