//! Tideline rail — the left column of the work screen (spec §5a "Rail",
//! §5b work layout): five groups (RUNS / WHALES / FLEET / WORK / CONTEXT),
//! then help/settings, and the `«` collapse. This is **additive** rendering
//! per the spec — #5699's shell semantics (placement, panels, hitboxes,
//! interaction) are untouched; the Tideline rail is the approved screen's
//! projection of the same facts (`WorkSurfaceState`, `subagent_cache`, run
//! list, git status are projected by the caller at the landing slice).
//!
//! Also hosts the Tideline work-stage composite (`rail │ receipt stream`)
//! whose golden buffers are `work_{w}x{h}`.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::palette::{ChromeInk, UiTheme, chrome_style};
use crate::tui::history::TidelineStream;

/// Rail width ladder (spec §5b): 22 at ≥120, 16 at ≥100, hidden below.
#[must_use]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn tideline_rail_width(host_width: u16) -> u16 {
    if host_width >= 120 {
        22
    } else if host_width >= 100 {
        16
    } else {
        0
    }
}

/// One rail group: label plus one summary line per fact.
#[derive(Debug, Clone)]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub struct TidelineRailGroup {
    pub label: &'static str,
    /// (fact line, ink) pairs, already summarized by the caller.
    pub lines: Vec<(String, ChromeInk)>,
}

/// What the caller owes the rail render.
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub struct TidelineRail<'a> {
    pub theme: &'a UiTheme,
    /// The five groups in display order: RUNS, WHALES, FLEET, WORK, CONTEXT.
    pub groups: &'a [TidelineRailGroup],
    /// Collapsed state — a 2-column `»` expander remains.
    pub collapsed: bool,
    /// Focused (keyboard Tab target per §6).
    pub focused: bool,
    pub ascii_safe: bool,
}

#[allow(dead_code)] // translation scaffolding: builder methods feed tests + the landing slice
impl<'a> TidelineRail<'a> {
    #[must_use]
    pub fn new(theme: &'a UiTheme, groups: &'a [TidelineRailGroup]) -> Self {
        Self {
            theme,
            groups,
            collapsed: false,
            focused: false,
            ascii_safe: false,
        }
    }

    #[must_use]
    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    #[must_use]
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    #[must_use]
    pub fn ascii_safe(mut self, ascii_safe: bool) -> Self {
        self.ascii_safe = ascii_safe;
        self
    }

    fn sym(&self, glyph: &str) -> String {
        if !self.ascii_safe {
            return glyph.to_string();
        }
        if let Some(fb) = crate::tui::glyphs::ascii_fallback(glyph) {
            return fb.to_string();
        }
        glyph
            .chars()
            .map(|c| {
                crate::tui::glyphs::ascii_fallback(&c.to_string())
                    .map(str::to_string)
                    .unwrap_or_else(|| c.to_string())
            })
            .collect()
    }
}

fn rchrome(theme: &UiTheme, ink: ChromeInk) -> Style {
    chrome_style(theme, ink)
}

fn rput(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) {
    buf.set_stringn(x, y, text, text.width(), style);
}

fn rtruncate(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// Paint the rail. Collapsed: a 2-column `»` expander only. Expanded:
/// group labels (dim caps), one line per fact, help/settings at the
/// bottom, and the `«` collapse toggle pinned to the last row.
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn render_tideline_rail(area: Rect, buf: &mut Buffer, rail: &TidelineRail<'_>) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let theme = rail.theme;
    let focus_edge_ink = if rail.focused {
        ChromeInk::Info
    } else {
        ChromeInk::MetadataDim
    };

    if rail.collapsed {
        // Expander spine: `»` at the top, dim focus edge down the column.
        rput(
            buf,
            area.x,
            area.y,
            &rail.sym("»"),
            rchrome(theme, focus_edge_ink),
        );
        return;
    }

    let width = area.width as usize;
    let mut y = area.y;
    for group in rail.groups {
        if y >= area.y + area.height.saturating_sub(2) {
            break;
        }
        rput(
            buf,
            area.x,
            y,
            &rtruncate(group.label, width),
            rchrome(theme, ChromeInk::MetadataDim).add_modifier(Modifier::BOLD),
        );
        y += 1;
        for (line, ink) in &group.lines {
            if y >= area.y + area.height.saturating_sub(2) {
                break;
            }
            rput(
                buf,
                area.x + 1,
                y,
                &rtruncate(&rail.sym(line), width.saturating_sub(1)),
                rchrome(theme, *ink),
            );
            y += 1;
        }
        y += 1;
    }

    // Meta rows then the collapse toggle.
    if area.height >= 3 {
        // One row above the collapse toggle.
        let bottom = area.y + area.height - 2;
        rput(
            buf,
            area.x,
            bottom,
            &rail.sym("? help · ⚙ settings"),
            rchrome(theme, ChromeInk::MetadataHint),
        );
    }
    if area.height >= 2 {
        rput(
            buf,
            area.x,
            area.y + area.height - 1,
            &rail.sym("« collapse"),
            rchrome(theme, focus_edge_ink),
        );
    }
}

/// The five-group fixture projection used by goldens and the preview pane:
/// RUNS / WHALES / FLEET / WORK / CONTEXT in display order.
#[must_use]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn tideline_rail_groups(
    run_label: &str,
    whales: &str,
    fleet_label: &str,
    work_lines: &[&str],
    context_percent: u8,
) -> Vec<TidelineRailGroup> {
    let meter_cells = 5usize;
    let filled = (usize::from(context_percent) * meter_cells / 100).min(meter_cells);
    let meter: String = (0..meter_cells)
        .map(|i| if i < filled { "▰" } else { "▱" })
        .collect();
    vec![
        TidelineRailGroup {
            label: "RUNS",
            lines: vec![(run_label.to_string(), ChromeInk::Identity)],
        },
        TidelineRailGroup {
            label: "WHALES",
            lines: vec![(whales.to_string(), ChromeInk::Info)],
        },
        TidelineRailGroup {
            label: "FLEET",
            lines: vec![(fleet_label.to_string(), ChromeInk::Active)],
        },
        TidelineRailGroup {
            label: "WORK",
            lines: work_lines
                .iter()
                .map(|line| (line.to_string(), ChromeInk::MetadataValue))
                .collect(),
        },
        TidelineRailGroup {
            label: "CONTEXT",
            lines: vec![(
                format!("{meter} {context_percent}%"),
                if context_percent >= 80 {
                    ChromeInk::Attention
                } else {
                    ChromeInk::Info
                },
            )],
        },
    ]
}

/// The work-stage composite (spec §5b): rail left, receipt stream right.
/// The ledger and composer dock into `main` in the live shell — they carry
/// their own golden suites (`ledger_*`, `composer_*`), so this composite
/// proves the rail + stream pair whose reserved golden name is `work_*`.
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub struct TidelineWorkStage<'a> {
    pub rail: TidelineRail<'a>,
    pub stream: TidelineStream<'a>,
}

/// Paint the work stage: `rail │ main` with the rail width ladder, then the
/// receipt stream filling `main`.
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn render_tideline_work_stage(area: Rect, buf: &mut Buffer, stage: &TidelineWorkStage<'_>) {
    if area.width < 10 || area.height < 1 {
        return;
    }
    let rail_w = if stage.rail.collapsed {
        2
    } else {
        tideline_rail_width(area.width)
    };
    let (rail_area, main_area) = if rail_w == 0 {
        (None, area)
    } else {
        let [rail_area, main_area] =
            Layout::horizontal([Constraint::Length(rail_w), Constraint::Min(1)]).areas(area);
        (Some(rail_area), main_area)
    };
    if let Some(rail_area) = rail_area {
        render_tideline_rail(rail_area, buf, &stage.rail);
    }
    crate::tui::history::render_tideline_stream(main_area, buf, &stage.stream);
}

/// Rail hitboxes (spec §6): the group label rows plus the collapse toggle.
/// Mirrors the painted rail; reused `WorkHitbox` semantics at the landing
/// slice.
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn tideline_rail_hitboxes(area: Rect, rail: &TidelineRail<'_>) -> Vec<Rect> {
    let mut out = Vec::new();
    if area.width < 2 || area.height < 2 || rail.collapsed {
        out.push(Rect {
            x: area.x,
            y: area.y,
            width: area.width.min(2),
            height: 1,
        });
        return out;
    }
    let mut y = area.y;
    for group in rail.groups {
        if y >= area.y + area.height {
            break;
        }
        out.push(Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1 + group.lines.len() as u16,
        });
        y += 1 + group.lines.len() as u16 + 1;
    }
    out
}

#[cfg(test)]
mod tests;
