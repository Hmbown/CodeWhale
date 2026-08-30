//! Tideline rail — the left column of the work screen (spec §5a "Rail",
//! §5b work layout): five groups (RUNS / WHALES / POD / WORK / CONTEXT),
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
use crate::tui::app::App;
use crate::tui::background_indicator::{PendingItemKind, PendingWork, pending_work_from_app};
use crate::tui::history::TidelineStream;

use super::WorkSurfacePlacement;

/// A compact live rail needs one row for each of its five labels and facts.
/// When the transcript slot is shorter than this, leave the current compact
/// work-surface behavior intact instead of reserving a column that cannot
/// truthfully show its full state.
const LIVE_TIDELINE_MIN_HEIGHT: u16 = 10;

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
    /// The five groups in display order: RUNS, WHALES, POD, WORK, CONTEXT.
    pub groups: &'a [TidelineRailGroup],
    /// Collapsed state — a 2-column `»` expander remains.
    pub collapsed: bool,
    /// Focused (keyboard Tab target per §6).
    pub focused: bool,
    pub ascii_safe: bool,
    /// Whether the caller has registered real keyboard and mouse behavior for
    /// the rail's help/settings/collapse affordances. Passive summaries omit
    /// those controls rather than painting inert UI.
    interactive: bool,
    /// Use a dense label/fact cadence. The live summary uses this to keep all
    /// five factual groups visible in a normal terminal-height chat slot.
    compact: bool,
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
            interactive: true,
            compact: false,
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

    /// Render the rail as a passive state summary.
    ///
    /// The live landing slice has no registered rail input targets yet, so it
    /// deliberately shows no `? help`, settings, or collapse glyph. This
    /// preserves the mockup's information architecture without implying that
    /// an unimplemented control can be clicked or focused.
    #[must_use]
    pub fn summary(mut self) -> Self {
        self.interactive = false;
        self.compact = true;
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

    if rail.interactive && rail.collapsed {
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
    let controls_height = if rail.interactive { 2 } else { 0 };
    let content_bottom = area.y + area.height.saturating_sub(controls_height);
    let mut y = area.y;
    for group in rail.groups {
        if y >= content_bottom {
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
            if y >= content_bottom {
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
        if !rail.compact {
            y += 1;
        }
    }

    // Meta rows then the collapse toggle.
    if rail.interactive && area.height >= 3 {
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
    if rail.interactive && area.height >= 2 {
        rput(
            buf,
            area.x,
            area.y + area.height - 1,
            &rail.sym("« collapse"),
            rchrome(theme, focus_edge_ink),
        );
    }
}

/// Return the Tideline reservation for a real active session.
///
/// `work_surface` remains the owner of placement and detailed work rows. The
/// default Left placement gets this summary only when its legacy side surface
/// has nothing to render; an occupied legacy rail is never overlaid or
/// duplicated. Top also receives the summary, while explicit Right and Off
/// keep their existing behavior.
#[must_use]
pub(crate) fn active_session_tideline_rail_width(
    app: &App,
    chat_area: Rect,
    legacy_side_rail_visible: bool,
) -> u16 {
    if app.launch.visible
        || app.current_session_id.is_none()
        || legacy_side_rail_visible
        || !matches!(
            app.work_surface.placement,
            WorkSurfacePlacement::Top | WorkSurfacePlacement::Left
        )
        || chat_area.height < LIVE_TIDELINE_MIN_HEIGHT
    {
        return 0;
    }
    tideline_rail_width(chat_area.width)
}

/// Paint the non-interactive active-session Tideline summary and its visual
/// divider. The rail owns no hitboxes until its controls have keyboard and
/// pointer parity; all facts are read from existing App projections.
pub(crate) fn render_active_session_tideline_rail(area: Rect, buf: &mut Buffer, app: &App) {
    if area.width < 2 || area.height < LIVE_TIDELINE_MIN_HEIGHT {
        return;
    }

    let theme = &app.ui_theme;
    let blank = " ".repeat(usize::from(area.width));
    for y in area.y..area.y.saturating_add(area.height) {
        rput(
            buf,
            area.x,
            y,
            &blank,
            Style::default().bg(theme.surface_bg),
        );
    }

    let content_area = Rect {
        width: area.width.saturating_sub(1),
        ..area
    };
    let groups = active_session_tideline_rail_groups(app);
    let rail = TidelineRail::new(theme, &groups)
        .summary()
        .ascii_safe(crate::tui::color_compat::ascii_safe_enabled());
    render_tideline_rail(content_area, buf, &rail);

    let divider = rail.sym("│");
    let divider_x = area.x.saturating_add(area.width.saturating_sub(1));
    for y in area.y..area.y.saturating_add(area.height) {
        rput(
            buf,
            divider_x,
            y,
            &divider,
            rchrome(theme, ChromeInk::MetadataDim).bg(theme.surface_bg),
        );
    }
}

/// Derive the five groups from the existing App state. This is intentionally
/// a read-only projection: no independent Tideline store, catalog, or async
/// refresh path is introduced here.
#[must_use]
pub(crate) fn active_session_tideline_rail_groups(app: &App) -> Vec<TidelineRailGroup> {
    let pending = pending_work_from_app(app);
    let running_whales = pending.count(PendingItemKind::Agent);
    // Progress-only workers can briefly precede their cache snapshot. Count
    // at least the live workers so the POD summary never claims none exist.
    let pod_total = app.subagent_cache.len().max(running_whales);
    let whale_capacity = app.max_subagents;

    let run_label = match app.workflow_panel.as_ref() {
        Some(panel) => {
            let chip = panel.top_bar_chip();
            chip.strip_prefix("wf ").unwrap_or(&chip).to_string()
        }
        None if app.is_loading => {
            if app.turn_counter == 0 {
                "turn active".to_string()
            } else {
                format!("turn {}", app.turn_counter)
            }
        }
        None => "idle".to_string(),
    };
    let whales = format!("{running_whales}/{whale_capacity} ready");
    let pod_label = if pod_total == 0 {
        "no agents".to_string()
    } else {
        format!("{running_whales}/{pod_total} live")
    };
    let work_line = active_session_work_line(app, &pending);

    tideline_rail_groups(
        &run_label,
        &whales,
        &pod_label,
        &[work_line.as_str()],
        crate::tui::phase_strip::context_percent_from_app(app),
    )
}

fn active_session_work_line(app: &App, pending: &PendingWork) -> String {
    let Ok(todos) = app.todos.try_lock() else {
        return "checklist busy".to_string();
    };
    let snapshot = todos.snapshot();
    if !snapshot.items.is_empty() {
        let total = snapshot.items.len();
        let open = snapshot
            .items
            .iter()
            .filter(|item| !item.status.is_settled())
            .count();
        return if open == 0 {
            format!("{total}/{total} done")
        } else {
            format!("{open}/{total} open")
        };
    }

    let background_count = pending.items.len();
    if background_count > 0 {
        format!("{background_count} active")
    } else if app.is_loading {
        "turn active".to_string()
    } else {
        "no checklist".to_string()
    }
}

/// The five-group fixture projection used by goldens and the preview pane:
/// RUNS / WHALES / POD / WORK / CONTEXT in display order.
#[must_use]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn tideline_rail_groups(
    run_label: &str,
    whales: &str,
    pod_label: &str,
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
            label: "POD",
            lines: vec![(pod_label.to_string(), ChromeInk::Active)],
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
    if !rail.interactive {
        return out;
    }
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
