//! Phase and identity bands for the underwater shell.
//!
//! Two one-row bands bracket the composer, and they never trade places
//! with it:
//!
//! * the **identity band** below the composer is the canonical, persistent
//!   home for `provider · model · thinking level`. The same standing facts
//!   render before, during, and after a prompt; a live turn never relocates
//!   them and neither band ever duplicates them.
//! * the **activity band** above the composer carries the transient pulse:
//!   phase marker, live work detail, session notices, and the cost/metrics
//!   ledger.
//!
//! Both bands are reserved in every frame, so a turn moving between idle,
//! thinking, tool use, approval, completion, failure, and cancellation
//! changes text inside fixed rows and never displaces the composer — the
//! route identity does not jump above the prompt when a turn starts, and
//! the prompt does not slide down to make room for it.

use std::borrow::Cow;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::localization::{MessageId, tr};
use crate::palette::{ChromeInk, UiTheme};
use crate::tui::{
    app::App,
    underwater::{LiveActivity, ShellPhase, ShellTier, phase_marker_with_activity},
};

/// Fixed one-row reservation for the identity band below the composer.
#[must_use]
pub fn height() -> u16 {
    1
}

/// Fixed one-row reservation for the activity band above the composer.
/// Reserved in every phase — including idle — so the composer's rows never
/// move when a turn starts, settles, fails, or is cancelled.
#[must_use]
pub fn activity_height() -> u16 {
    1
}

fn span_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}

/// Compact working detail for the phase band: `×N` for tools or `1m 15s`
/// while the model is thinking.
/// Kept quieter than the classic footer's verbose tool-status line so the
/// transcript owns the ledger and the strip only names the live pulse.
fn working_detail(app: &App, activity: LiveActivity) -> Option<String> {
    let running = activity.running_tool_count();
    let secs = app
        .turn_started_at
        .map(|started| started.elapsed().as_secs());
    match (running, secs) {
        (0, Some(secs)) if secs > 0 => Some(crate::elapsed::format_elapsed_secs(secs)),
        (n, Some(_)) if n > 0 => Some(format!("×{n}")),
        (n, None) if n > 0 => Some(format!("×{n}")),
        _ => None,
    }
}

fn session_cache_hit_percentage(app: &App) -> Option<u8> {
    let hit = u64::from(app.session.total_cache_hit_tokens);
    let miss = u64::from(app.session.total_cache_miss_tokens);
    let total = hit + miss;
    if total == 0 {
        return None;
    }

    // Round to the nearest whole percent. Widen before adding so sessions
    // with saturated u32 telemetry counters can never render above 100%.
    Some(((hit * 100 + total / 2) / total) as u8)
}

/// Route identity for the rail, shed field by field until it fits `budget`.
///
/// The old version composed the full `provider · model · effort` label and
/// then `truncate_to_width`'d it to a fixed 24/44/64 columns, which happily
/// rendered `deepseek-v4-flash-prev…`. A clipped model name is worse than no
/// model name: routes share prefixes, so the ellipsis is the rail admitting
/// it will not tell you which model is answering. Shed the qualifiers
/// instead — provider first, then effort — and if the bare model name still
/// does not fit, shed the whole group. `/model` and `/status` own the full
/// route either way.
fn route_identity_fields(app: &App, tier: ShellTier, budget: usize) -> Option<Vec<String>> {
    let (provider, model) = app.effective_route_identity_display();
    let effort = app.reasoning_effort_display_label();
    if model.is_empty() {
        return None;
    }
    let mut candidates: Vec<Vec<String>> = Vec::new();
    if tier != ShellTier::Compact && !provider.is_empty() && !effort.is_empty() {
        // The smallest shell never repeats the provider: model and effort are
        // the two facts that change what comes back.
        candidates.push(vec![provider, model.clone(), effort.clone()]);
    }
    if !effort.is_empty() {
        candidates.push(vec![model.clone(), effort]);
    }
    candidates.push(vec![model]);
    candidates.into_iter().find(|fields| {
        let width = fields.iter().map(|field| field.width()).sum::<usize>()
            + fields.len().saturating_sub(1) * ITEM_SEPARATOR_WIDTH;
        width <= budget
    })
}

/// Paint an identity group: the model name one step brighter than the
/// qualifiers that narrow it, so the field a mid-session glance is looking
/// for is the field the eye lands on. Two weights, no new colour.
fn identity_spans(fields: &[String], model_index: usize, theme: &UiTheme) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(fields.len() * 2);
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(
                ITEM_SEPARATOR,
                Style::default().fg(ChromeInk::MetadataDim.color(theme)),
            ));
        }
        let ink = if index == model_index {
            ChromeInk::MetadataValue
        } else {
            ChromeInk::Metadata
        };
        spans.push(Span::styled(
            field.clone(),
            Style::default().fg(ink.color(theme)),
        ));
    }
    spans
}

/// Split a notice at its joints, coarsest first.
///
/// A rail notice is prose, and prose has joints. Cutting at a joint keeps
/// every word that survives true; cutting mid-phrase and hanging an ellipsis
/// off the end only advertises that the row lost the argument. Sentence stops
/// are the joint we want; the inner marks are the fallback for a one-sentence
/// notice that is still too long for a narrow rail — losing the second half
/// of `Auto-denied exec_shell: denied earlier` beats losing the warning.
fn notice_clauses<'a>(text: &'a str, marks: &[char]) -> Vec<&'a str> {
    let mut clauses = Vec::new();
    let mut start = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if !marks.contains(&ch) {
            continue;
        }
        // Full-width marks carry no trailing space, so they break on sight.
        // ASCII marks only break before whitespace, which keeps `0.9.11`,
        // `docs/TELEMETRY.md`, and `https://…` in one piece.
        let breaks = !ch.is_ascii() || chars.peek().is_none_or(|(_, next)| next.is_whitespace());
        if !breaks {
            continue;
        }
        let end = idx + ch.len_utf8();
        let clause = text[start..end].trim();
        if !clause.is_empty() {
            clauses.push(clause);
        }
        start = end;
    }
    let rest = text[start..].trim();
    if !rest.is_empty() {
        clauses.push(rest);
    }
    clauses
}

/// Sentence stops — the joint a notice prefers to be cut at.
const SENTENCE_MARKS: [char; 7] = ['.', '!', '?', '…', '。', '！', '？'];
/// Inner joints, used only when one sentence still will not fit the rail.
const CLAUSE_MARKS: [char; 8] = [';', ':', ',', '—', '；', '：', '，', '、'];

fn join_while_fitting(clauses: &[&str], budget: usize) -> Option<String> {
    let mut fitted = String::new();
    for clause in clauses {
        // A full-width stop already carries its own breathing room; putting
        // a Latin space after `。` is a typographic accent in the wrong
        // language.
        let space = usize::from(!fitted.is_empty() && fitted.ends_with(|ch: char| ch.is_ascii()));
        let candidate = fitted.width() + space + clause.width();
        if candidate > budget {
            break;
        }
        if space == 1 {
            fitted.push(' ');
        }
        fitted.push_str(clause);
    }
    // A phrase that ends on `:` or `;` is still telling you more is coming —
    // the same lie an ellipsis tells. Cut the mark and let the phrase stand.
    let fitted = fitted
        .trim_end_matches(|ch| CLAUSE_MARKS.contains(&ch) || ch == ' ')
        .to_string();
    (!fitted.is_empty()).then_some(fitted)
}

/// Fit a notice into `budget` by dropping whole trailing clauses.
///
/// Returns `None` only when not even the first inner phrase fits, and the
/// rail then says nothing rather than dangling a stump. Notices get first
/// call on the row: identity and the ledger chips have already stood down by
/// the time this is asked, and the key hints stand down after it if that is
/// what the notice needs.
fn fit_notice(text: &str, budget: usize) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || budget == 0 {
        return None;
    }
    if text.width() <= budget {
        return Some(text.to_string());
    }
    let sentences = notice_clauses(text, &SENTENCE_MARKS);
    if let Some(fitted) = join_while_fitting(&sentences, budget) {
        return Some(fitted);
    }
    let first = sentences.first().copied().unwrap_or(text);
    join_while_fitting(&notice_clauses(first, &CLAUSE_MARKS), budget)
}

/// Toasts share the footer rail, so their typed level must resolve through
/// the same closed status-bar grammar as the phase marker around them.
fn status_toast_ink(level: crate::tui::app::StatusToastLevel) -> ChromeInk {
    match level {
        crate::tui::app::StatusToastLevel::Info => ChromeInk::Info,
        crate::tui::app::StatusToastLevel::Success => ChromeInk::Outcome,
        crate::tui::app::StatusToastLevel::Warning => ChromeInk::Attention,
        crate::tui::app::StatusToastLevel::Error => ChromeInk::Failure,
    }
}

/// Paint the activity band: the transient row above the composer.
///
/// The band speaks in priority order and sheds from the bottom of that
/// order when the row is narrow:
///
/// 1. **phase marker + verb** — the row's reason to exist; never sheds.
/// 2. **an urgent notice** — Warning/Error toasts are actionable and may
///    evict every lower-priority group to fit.
/// 3. **working detail** — tool count or elapsed time while a turn is live.
/// 4. **`Esc to interrupt`** — the interrupt affordance beside the work.
/// 5. **a routine notice** — informational toasts yield to the groups above
///    rather than evicting them.
/// 6. **ledger** — cost chip, cache chip, then session-metrics groups; each
///    fits whole or is not painted.
///
/// Route identity never appears here: the identity band below the composer
/// owns it in every phase, without duplication.
pub fn render_activity(area: Rect, buf: &mut Buffer, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let status_toast = app.active_status_toast();
    let activity = LiveActivity::from_app(app);
    let phase = ShellPhase::from_app_with_activity(app, activity);
    let tier = ShellTier::for_chrome_width(area.width);
    // Quiet chrome background — never paint the full row in phase accent.
    Block::default()
        .style(Style::default().bg(app.ui_theme.footer_bg))
        .render(area, buf);

    // Compact left rail: one accent cell + marker + verb (not a full-width band).
    let rail_color = phase.color(app);
    let (marker, phase_label) = phase_marker_with_activity(app, phase, activity);
    let phase_style = Style::default().fg(rail_color).add_modifier(
        if matches!(phase, ShellPhase::Waiting | ShellPhase::Approval) {
            Modifier::BOLD
        } else {
            Modifier::empty()
        },
    );
    let mut left = vec![
        Span::styled("▌", phase_style),
        Span::styled(marker, phase_style),
        Span::raw(" "),
        Span::styled(phase_label.clone(), phase_style),
    ];
    let mut used = span_width(&left);
    let available = usize::from(area.width);

    // Live work detail and the interrupt affordance share the left group.
    let mut detail: Vec<Span<'static>> = Vec::new();
    if tier != ShellTier::Compact && matches!(phase, ShellPhase::Working | ShellPhase::Verifying) {
        if let Some(detail_text) = working_detail(app, activity) {
            detail.push(Span::styled(
                ITEM_SEPARATOR,
                Style::default().fg(ChromeInk::MetadataDim.color(&app.ui_theme)),
            ));
            detail.push(Span::styled(
                detail_text,
                Style::default().fg(ChromeInk::Active.color(&app.ui_theme)),
            ));
        }
        detail.push(Span::styled(
            ITEM_SEPARATOR,
            Style::default().fg(ChromeInk::MetadataDim.color(&app.ui_theme)),
        ));
        // `Esc to interrupt` is a key hint that happens to sit next to the
        // thing it interrupts. It reads in the hint weight, not in the
        // separator weight it used to share.
        detail.push(Span::styled(
            tr(app.ui_locale, MessageId::FooterHintEscInterrupt).into_owned(),
            Style::default().fg(ChromeInk::MetadataHint.color(&app.ui_theme)),
        ));
    }
    let mut show_detail = !detail.is_empty();

    let notice = (tier != ShellTier::Compact)
        .then_some(status_toast)
        .flatten()
        .filter(|toast| {
            // Completion may land in the same event drain as an approval
            // denial. Keep unresolved attention/error receipts visible after
            // `done`; only routine informational completion copy yields to
            // the stable done marker.
            let survives_completion = matches!(
                toast.level,
                crate::tui::app::StatusToastLevel::Warning
                    | crate::tui::app::StatusToastLevel::Error
            );
            (phase != ShellPhase::Done || survives_completion)
                && !toast.text.trim().is_empty()
                && toast.text.trim() != phase_label.as_ref()
        })
        .map(|toast| {
            let urgent = matches!(
                toast.level,
                crate::tui::app::StatusToastLevel::Warning
                    | crate::tui::app::StatusToastLevel::Error
            );
            (toast.text.clone(), status_toast_ink(toast.level), urgent)
        });

    // Urgent notices win the row over detail and the ledger; routine ones
    // stand down instead. Route identity no longer competes — it lives
    // below the composer — so an urgent notice evicts only transient groups.
    let notice = notice.and_then(|(text, ink, urgent)| {
        let detail_cost = if show_detail { span_width(&detail) } else { 0 };
        let beside = available.saturating_sub(used + detail_cost + GROUP_GAP_WIDTH);
        if let Some(fitted) = fit_notice(&text, beside) {
            return Some((fitted, ink));
        }
        if !urgent {
            return None;
        }
        let alone = available.saturating_sub(used + GROUP_GAP_WIDTH);
        let fitted = fit_notice(&text, alone)?;
        show_detail = false;
        Some((fitted, ink))
    });

    if show_detail {
        used += span_width(&detail);
        left.extend(detail);
    }
    if let Some((text, ink)) = notice {
        left.push(Span::raw(GROUP_GAP));
        left.push(Span::styled(
            text,
            Style::default().fg(ink.color(&app.ui_theme)),
        ));
    } else {
        // No notice: the ledger may spend the rest of the row. Each group
        // fits whole or is not painted.
        left.extend(ledger_spans(app, tier, available.saturating_sub(used)));
    }

    Paragraph::new(Line::from(left)).render(area, buf);
}

/// Cost and session-metrics chips for whatever columns the activity band has
/// left. The gutter opens the ledger group; the bar is the metrics strip's
/// own internal divider, so it only shows up once the group is already open.
fn ledger_spans(app: &App, tier: ShellTier, available: usize) -> Vec<Span<'static>> {
    let mut ledger: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let chip = app.cumulative_usage_chip();
    if tier != ShellTier::Compact
        && let Some(amount) = match &chip {
            crate::route_billing::UsageChip::Money(amount) => Some(amount.clone()),
            crate::route_billing::UsageChip::PricedSubtotal { .. } => {
                crate::route_billing::format_usage_chip(&chip)
            }
            _ => None,
        }
        && amount.width() + GROUP_GAP_WIDTH <= available.saturating_sub(used)
    {
        used += GROUP_GAP_WIDTH + amount.width();
        ledger.push(Span::raw(GROUP_GAP));
        ledger.push(Span::styled(
            amount,
            Style::default().fg(ChromeInk::Metadata.color(&app.ui_theme)),
        ));
    }

    // The session metrics strip owns the cache cell when it is on; the
    // standalone `cache N%` chip stays for users who turned the strip off.
    let metrics_enabled = app
        .status_items
        .contains(&crate::config::StatusItem::SessionMetrics);
    if !metrics_enabled
        && tier != ShellTier::Compact
        && app.status_items.contains(&crate::config::StatusItem::Cache)
        && let Some(pct) = session_cache_hit_percentage(app)
    {
        let chip = format!("cache {pct}%");
        let separator_width = if ledger.is_empty() {
            GROUP_GAP_WIDTH
        } else {
            ITEM_SEPARATOR_WIDTH
        };
        if chip.width() + separator_width <= available.saturating_sub(used) {
            used += separator_width + chip.width();
            ledger.push(if ledger.is_empty() {
                Span::raw(GROUP_GAP)
            } else {
                Span::styled(
                    ITEM_SEPARATOR,
                    Style::default().fg(ChromeInk::MetadataDim.color(&app.ui_theme)),
                )
            });
            ledger.push(Span::styled(
                chip,
                Style::default().fg(ChromeInk::Metadata.color(&app.ui_theme)),
            ));
        }
    }

    // Session metrics strip (`4 turns · 108 steps │ LLM 11m46s · tools
    // 1m52s │ TTFT 1.5s · 120 tok/s │ cache 99% │ in 9.3M`). It takes
    // whatever columns are genuinely free and sheds its lowest-value
    // groups to fit rather than truncating a number.
    if metrics_enabled && tier != ShellTier::Compact {
        let snapshot = crate::tui::session_metrics::snapshot_from_app(app);
        if !snapshot.is_empty() {
            let budget = available.saturating_sub(used + LEDGER_OPENER_WIDTH);
            let ascii = crate::tui::color_compat::ascii_safe_enabled();
            let strip = crate::tui::session_metrics::fit_to_width(
                crate::tui::session_metrics::build_groups(snapshot, app.ui_locale),
                budget,
                crate::tui::session_metrics::Separators::for_ascii(ascii),
            );
            if !strip.is_empty() {
                ledger.push(if ledger.is_empty() {
                    Span::raw(GROUP_GAP)
                } else {
                    Span::styled(
                        if ascii { " | " } else { " │ " },
                        Style::default().fg(ChromeInk::MetadataDim.color(&app.ui_theme)),
                    )
                });
                ledger.extend(crate::tui::session_metrics::spans(&strip, &app.ui_theme));
            }
        }
    }
    ledger
}

/// Paint the identity band: the persistent row below the composer.
///
/// Canonical home of `provider · model · thinking level`, before, during,
/// and after a prompt. Under width pressure the group sheds whole fields —
/// provider first, then the thinking level — and then stands down entirely
/// rather than clip a model name (`/model` and `/status` own the full
/// route). Right-aligned key hints are the only other resident: the chord
/// chorus while idle or drafting, plus the agent-focus keys whenever the
/// empty composer still owns them.
pub fn render_identity(area: Rect, buf: &mut Buffer, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let phase = ShellPhase::from_app(app);
    let tier = ShellTier::for_chrome_width(area.width);
    // Quiet chrome background — the identity row never takes phase accent.
    Block::default()
        .style(Style::default().bg(app.ui_theme.footer_bg))
        .render(area, buf);

    // Hints come from shell_key_routing so advertised chords match handlers;
    // bare letters are never advertised — the composer owns printable keys.
    // Live phases keep the row quiet; idle and drafting may advertise keys.
    let mut right_text: Cow<'static, str> = if matches!(
        phase,
        ShellPhase::Working
            | ShellPhase::Verifying
            | ShellPhase::Waiting
            | ShellPhase::Approval
            | ShellPhase::Failed
    ) {
        Cow::Borrowed("")
    } else {
        use crate::tui::shell_key_routing::{ShellBindingId, binding, footer_action_hints};
        let hint_keys = tr(app.ui_locale, MessageId::FooterHintKeys);
        let hint_output = tr(app.ui_locale, MessageId::FooterHintOutput);
        Cow::Owned(match tier {
            ShellTier::Compact => {
                format!("{}:{hint_keys}", binding(ShellBindingId::Help).footer_chord)
            }
            // Wide used to add `/context:context`. The rail advertises
            // chords you cannot discover any other way; a slash command
            // announces itself the moment you type `/`, so it was paying
            // eighteen columns of a 24-row screen to tell you something
            // the composer already tells you. The rail now reads the same
            // at 80 columns as at 200.
            ShellTier::Normal | ShellTier::Wide => footer_action_hints(false)
                .replace("{output}", hint_output.as_ref())
                .replace("{keys}", hint_keys.as_ref()),
        })
    };

    // `← for agents · ↓ to manage`: advertise only while the empty
    // composer owns those keys and a worker exists. Focused surfaces,
    // modals, attachments, and draft text keep the arrows' local meaning.
    let agent_hints = (tier != ShellTier::Compact
        // Slash and mention menus require non-empty trigger text, which the
        // shared predicate rejects; dispatch additionally passes its exact
        // post-completion menu ownership into the same predicate.
        && crate::tui::agent_focus::shell_shortcuts_available(app, false))
    .then(|| crate::tui::agent_focus::footer_agent_hints(app));
    right_text = match agent_hints {
        Some(hints) if !right_text.is_empty() => {
            Cow::Owned(format!("{hints}{ITEM_SEPARATOR}{right_text}"))
        }
        // A settled turn keeps the agent keys meaningful, so they stay
        // visible on the identity row after `done` even without the chorus.
        Some(hints) if phase == ShellPhase::Done => Cow::Owned(hints),
        _ => right_text,
    };

    let available = usize::from(area.width);
    let hint_width = right_text.width();
    let mut left = Vec::new();
    if let Some(fields) = route_identity_fields(app, tier, available) {
        let model_index = usize::from(fields.len() == 3);
        left.extend(identity_spans(&fields, model_index, &app.ui_theme));
    }
    let left_width = span_width(&left);
    // Identity outranks the chorus. The route is the row's contract; key
    // hints are lower-priority neighbors and stand down first when the row
    // is tight.
    if hint_width > 0 && left_width + HINT_GAP + hint_width <= available {
        left.push(Span::raw(" ".repeat(available - left_width - hint_width)));
        left.push(Span::styled(
            right_text.into_owned(),
            Style::default().fg(ChromeInk::MetadataHint.color(&app.ui_theme)),
        ));
    }
    Paragraph::new(Line::from(left)).render(area, buf);
}

/// Peers inside one group — provider and model, a count and its verb — keep
/// the middle dot.
const ITEM_SEPARATOR: &str = " · ";
const ITEM_SEPARATOR_WIDTH: usize = 3;
/// Blank gutter between the rail's semantic groups.
///
/// The rail used to join every group with that same ` · ` in that same ink,
/// so live state, route identity, a privacy notice, and keyboard hints all
/// read as one run-on list of peers. A dot separates peers inside a group; a
/// gutter separates groups. Same three columns, no extra ink, and the eye
/// finally has something to skim by.
const GROUP_GAP: &str = "   ";
const GROUP_GAP_WIDTH: usize = 3;
/// Blank columns kept between the left run and the right-aligned key hints.
const HINT_GAP: usize = 2;
/// Width of whichever mark opens the session metrics strip — the group
/// gutter when the ledger starts with it, the ` │ ` bar when chips came
/// first. Both are three columns.
const LEDGER_OPENER_WIDTH: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        tui::active_cell::ActiveCell,
        tui::app::TuiOptions,
        tui::history::{ExecCell, ExecSource, HistoryCell, ToolCell, ToolStatus},
    };
    use ratatui::{Terminal, backend::TestBackend};
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };

    fn test_app() -> App {
        App::new(
            TuiOptions {
                model: "deepseek-v4-flash".to_string(),
                ..crate::test_support::test_tui_options(PathBuf::from("."))
            },
            &Config::default(),
        )
    }

    fn band_text(app: &mut App, width: u16, band: fn(Rect, &mut Buffer, &mut App)) -> String {
        let backend = TestBackend::new(width, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| band(frame.area(), frame.buffer_mut(), app))
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn activity_text(app: &mut App, width: u16) -> String {
        band_text(app, width, render_activity)
    }

    fn identity_text(app: &mut App, width: u16) -> String {
        band_text(app, width, render_identity)
    }

    /// One running tool cell on the live stack, shared by the state matrix
    /// and the detail tests.
    fn running_tool_app(command: &str) -> App {
        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::En;
        app.is_loading = true;
        app.turn_started_at = Some(Instant::now() - Duration::from_secs(12));
        let mut active = ActiveCell::new();
        active.push_tool(
            "exec-1",
            HistoryCell::Tool(ToolCell::Exec(ExecCell {
                command: command.to_string(),
                status: ToolStatus::Running,
                output: None,
                live_output: None,
                shell_task_id: None,
                owner_agent_id: None,
                owner_agent_name: None,
                started_at: app.turn_started_at,
                duration_ms: None,
                stale_elapsed_since_output_ms: None,
                source: ExecSource::Assistant,
                interaction: None,
                output_summary: None,
            })),
        );
        app.active_cell = Some(active);
        app
    }

    /// Mutates a fresh app into one perceptual phase of `ShellPhase::from_app`.
    type PhaseSetup = fn(&mut App);

    /// Every turn state the bands live through.
    fn state_matrix() -> Vec<(&'static str, PhaseSetup)> {
        fn idle(app: &mut App) {
            app.runtime_turn_status = None;
        }
        fn typing(app: &mut App) {
            app.input = "draft".to_string();
            app.cursor_position = app.input.chars().count();
        }
        fn thinking(app: &mut App) {
            app.is_loading = true;
            app.turn_started_at = Some(Instant::now());
        }
        fn using_tool(app: &mut App) {
            let tool = running_tool_app("cargo build -p tui");
            app.active_cell = tool.active_cell;
            app.is_loading = true;
            app.turn_started_at = Some(Instant::now() - Duration::from_secs(12));
        }
        fn verifying(app: &mut App) {
            let tool = running_tool_app("cargo test -p tui");
            app.active_cell = tool.active_cell;
            app.is_loading = true;
            app.turn_started_at = Some(Instant::now() - Duration::from_secs(12));
        }
        fn waiting(app: &mut App) {
            app.pending_user_input_prompt = Some((
                "Which database?".to_string(),
                crate::tools::user_input::UserInputRequest {
                    questions: Vec::new(),
                },
            ));
        }
        fn done(app: &mut App) {
            app.runtime_turn_status = Some("completed".to_string());
        }
        fn failed(app: &mut App) {
            app.turn_error_posted = true;
        }
        vec![
            ("idle", idle),
            ("typing", typing),
            ("thinking", thinking),
            ("using_tool", using_tool),
            ("verifying", verifying),
            ("waiting", waiting),
            ("done", done),
            ("failed", failed),
        ]
    }

    /// The contract this module exists to keep: the identity band below the
    /// composer is the canonical route home in every phase, and the activity
    /// band above the composer never takes it over — not when a turn starts,
    /// not while it waits on the user, not after it fails or settles.
    #[test]
    fn route_identity_stays_on_its_own_row_in_every_phase() {
        for (name, prepare) in state_matrix() {
            let mut app = test_app();
            app.ui_locale = crate::localization::Locale::En;
            prepare(&mut app);
            let identity = identity_text(&mut app, 120);
            assert!(
                identity.starts_with("DeepSeek · deepseek-v4-flash"),
                "{name}: identity row lost the route: {identity:?}"
            );
            let activity = activity_text(&mut app, 120);
            assert!(
                !activity.contains("deepseek-v4-flash"),
                "{name}: activity row duplicated the model: {activity:?}"
            );
            assert!(
                !activity.contains("DeepSeek"),
                "{name}: activity row duplicated the provider: {activity:?}"
            );
        }
    }

    /// A turn's state transitions rewrite text inside the two fixed rows;
    /// they never relocate the identity. The same route prefix renders
    /// before, during, and after a prompt.
    #[test]
    fn identity_row_keeps_one_stable_prefix_across_turn_state_transitions() {
        let mut prefix: Option<String> = None;
        for (name, prepare) in state_matrix() {
            let mut app = test_app();
            app.ui_locale = crate::localization::Locale::En;
            prepare(&mut app);
            let identity = identity_text(&mut app, 120);
            let route_end = identity
                .find("deepseek-v4-flash")
                .map(|start| start + "deepseek-v4-flash".len())
                .unwrap_or(0);
            let route_prefix = identity[..route_end].to_string();
            let expected = prefix.get_or_insert(route_prefix.clone());
            assert_eq!(
                &route_prefix, expected,
                "{name}: the route moved or changed across a state transition: {identity:?}"
            );
        }
    }

    #[test]
    fn footer_toasts_stay_inside_the_closed_color_grammar() {
        use crate::tui::app::StatusToastLevel;

        for (level, expected) in [
            (StatusToastLevel::Info, ChromeInk::Info),
            (StatusToastLevel::Success, ChromeInk::Outcome),
            (StatusToastLevel::Warning, ChromeInk::Attention),
            (StatusToastLevel::Error, ChromeInk::Failure),
        ] {
            assert_eq!(status_toast_ink(level), expected, "{level:?}");

            let mut app = test_app();
            app.ui_theme = crate::palette::ThemeId::Dracula.ui_theme();
            app.push_status_toast("toast proof", level, None);
            let area = Rect::new(0, 0, 160, 1);
            let mut buf = Buffer::empty(area);
            render_activity(area, &mut buf, &mut app);
            let rendered = (0..area.width)
                .map(|x| buf[(x, 0)].symbol())
                .collect::<String>();
            let byte = rendered
                .find("toast proof")
                .unwrap_or_else(|| panic!("{level:?} toast should render: {rendered:?}"));
            let x = rendered[..byte].width() as u16;
            assert_eq!(
                buf[(x, 0)].fg,
                expected.color(&app.ui_theme),
                "{level:?} must use the active theme's grammar slot"
            );
        }
    }

    #[test]
    fn working_marker_uses_the_live_work_status_role() {
        let app = test_app();
        assert_eq!(ShellPhase::Working.color(&app), app.ui_theme.status_working);
        assert_ne!(ShellPhase::Working.color(&app), app.ui_theme.info);
        assert_eq!(
            crate::tui::underwater::phase_ink(ShellPhase::Working),
            ChromeInk::Active
        );
        assert_eq!(
            crate::tui::underwater::phase_ink(ShellPhase::Failed),
            ChromeInk::Failure
        );
        assert_ne!(
            crate::tui::underwater::phase_ink(ShellPhase::Working).family(),
            crate::palette::SemanticFamily::Failure
        );
    }

    #[test]
    fn working_band_names_tool_use_and_bounded_count_without_key_chorus() {
        let mut app = running_tool_app("cargo build -p tui");
        let text = activity_text(&mut app, 80);
        assert!(text.contains("using tool"), "{text}");
        assert!(text.contains("×1"), "{text}");
        assert!(
            !text.contains("12s"),
            "tool elapsed time belongs to the live tool row: {text}"
        );
        assert!(
            !text.contains("run ×1"),
            "detail repeated the tool verb: {text}"
        );
        assert!(
            !text.contains("Alt+?") && !text.contains("F1:"),
            "live phase band stays quiet: {text}"
        );
        assert!(text.contains("Esc to interrupt"), "{text}");
        assert!(
            !text.contains("deepseek"),
            "route identity must not migrate into the activity band: {text}"
        );

        // The identity row keeps the route through the same live turn, and
        // carries no activity detail or key chorus of its own.
        let identity = identity_text(&mut app, 80);
        assert!(identity.contains("deepseek-v4-flash"), "{identity}");
        assert!(!identity.contains("Esc"), "{identity}");
        assert!(!identity.contains("keys"), "{identity}");
    }

    #[test]
    fn compact_activity_band_keeps_only_the_semantic_label() {
        let mut app = running_tool_app("cargo build -p tui");
        let text = activity_text(&mut app, 50);
        assert!(text.contains("using tool"), "{text}");
        assert!(
            !text.contains('×'),
            "tool count is detail, not semantics: {text}"
        );
        assert!(!text.contains("Esc"), "{text}");
    }

    #[test]
    fn working_band_keeps_elapsed_time_when_model_is_thinking() {
        let mut app = test_app();
        app.is_loading = true;
        app.turn_started_at = Some(Instant::now() - Duration::from_secs(12));

        assert_eq!(
            working_detail(&app, LiveActivity::from_app(&app)).as_deref(),
            Some("12s")
        );

        let text = activity_text(&mut app, 80);
        assert!(text.contains("12s"), "{text}");
        assert!(text.contains("Esc to interrupt"), "{text}");
    }

    #[test]
    fn completed_band_keeps_unresolved_warning_visible() {
        let mut app = test_app();
        app.runtime_turn_status = Some("completed".to_string());
        app.push_status_toast(
            "Auto-denied exec_shell: denied earlier; restart Codewhale",
            crate::tui::app::StatusToastLevel::Warning,
            Some(12_000),
        );

        let text = activity_text(&mut app, 100);
        assert!(text.contains("done"), "completion phase missing: {text}");
        assert!(
            text.contains("Auto-denied exec_shell"),
            "completion hid unresolved warning: {text}"
        );

        let identity = identity_text(&mut app, 100);
        assert!(
            identity.starts_with("DeepSeek · deepseek-v4-flash"),
            "completion must not disturb the route row: {identity:?}"
        );
    }

    #[test]
    fn cache_percentage_uses_wide_arithmetic_and_rounds() {
        let mut app = test_app();
        assert_eq!(session_cache_hit_percentage(&app), None);

        app.session.total_cache_hit_tokens = 2_000_000_000;
        app.session.total_cache_miss_tokens = 1_000_000_000;
        assert_eq!(session_cache_hit_percentage(&app), Some(67));

        app.session.total_cache_hit_tokens = 1;
        app.session.total_cache_miss_tokens = 1;
        assert_eq!(session_cache_hit_percentage(&app), Some(50));
    }

    #[test]
    fn cache_chip_is_labeled_configurable_and_hidden_when_compact() {
        let mut app = test_app();
        app.status_items = vec![crate::config::StatusItem::Cache];
        app.session.total_cache_hit_tokens = 7;
        app.session.total_cache_miss_tokens = 3;

        let text = activity_text(&mut app, 80);
        assert!(text.contains("cache 70%"), "{text}");

        app.status_items.clear();
        let text = activity_text(&mut app, 80);
        assert!(!text.contains("cache"), "{text}");

        app.status_items = vec![crate::config::StatusItem::Cache];
        let text = activity_text(&mut app, 50);
        assert!(!text.contains("cache"), "compact strip: {text}");
    }

    fn app_with_session_metrics() -> App {
        let mut app = test_app();
        app.status_items = vec![crate::config::StatusItem::SessionMetrics];
        app.turn_counter = 4;
        // Two model calls: 100 tokens over a 2 s stream (TTFT 500 ms, whole
        // call 2.4 s) and 20 tokens over 1 s (TTFT 300 ms, 1.1 s).
        app.session_metrics
            .record_model_call(100, 2_000, Some(500), Some(2_400));
        app.session_metrics
            .record_model_call(20, 1_000, Some(300), Some(1_100));
        app.session_metrics.record_tool_started("t1");
        app.session_metrics.record_tool_completed("t1");
        app.session.total_input_tokens = 9_300_000;
        app.session.total_cache_hit_tokens = 900_000;
        app.session.total_cache_miss_tokens = 10_000;
        app
    }

    #[test]
    fn session_metrics_paint_the_activity_row_and_leave_the_route_to_its_own() {
        let mut app = app_with_session_metrics();
        let activity = activity_text(&mut app, 190);
        assert!(activity.contains("4 turns · 3 steps"), "{activity}");
        assert!(activity.contains("LLM 3.5s · Tool call"), "{activity}");
        assert!(activity.contains("TTFT avg 400ms · 40 tok/s"), "{activity}");
        assert!(activity.contains("Cache hit 99%"), "{activity}");
        assert!(activity.contains("Input 9.3M"), "{activity}");
        assert!(
            !activity.contains("deepseek"),
            "metrics must not pull the route up into the activity row: {activity}"
        );

        // The identity row owns the route and the key chorus; both fit
        // because they no longer compete with the metrics for one row.
        let identity = identity_text(&mut app, 150);
        assert!(
            identity.contains("DeepSeek · deepseek-v4-flash · max"),
            "{identity}"
        );
        assert!(identity.contains("keys"), "{identity}");
    }

    #[test]
    fn session_metrics_shed_groups_on_narrow_rows_and_never_truncate() {
        let mut app = app_with_session_metrics();
        let normal = activity_text(&mut app, 100);
        assert!(normal.contains("Input 9.3M"), "{normal}");
        assert!(normal.contains("Cache hit 99%"), "{normal}");
        assert!(!normal.contains("tok/s"), "{normal}");

        // Whatever survives at 60 columns is whole cells, never a cut number.
        let compact = activity_text(&mut app, 60);
        for cell in ["9.3M", "99%", "3.5s", "4 turns"] {
            if compact.contains(cell) {
                assert!(
                    compact.contains("Input 9.3M")
                        || compact.contains("Cache hit 99%")
                        || compact.contains("LLM 3.5s")
                        || compact.contains("4 turns"),
                    "{compact}"
                );
            }
        }
        assert!(!compact.contains("Tool call"), "{compact}");
        assert!(!compact.contains("tok/s"), "{compact}");
    }

    #[test]
    fn session_metrics_are_hidden_when_compact() {
        let mut app = app_with_session_metrics();
        // 59 columns is the widest Compact row. The working detail and the
        // cache chip already stand down here, so the metrics strip claiming
        // the leftovers is the one thing still crowding the label.
        let text = activity_text(&mut app, 59);
        assert!(!text.contains("turns"), "compact strip: {text}");
        assert!(!text.contains("LLM"), "compact strip: {text}");
        assert!(!text.contains('│'), "compact strip: {text}");

        // Compact never repeats the provider; the model still fits whole.
        let identity = identity_text(&mut app, 59);
        assert!(identity.contains("deepseek-v4-flash"), "{identity}");
        assert!(!identity.contains("DeepSeek ·"), "{identity}");
    }

    #[test]
    fn session_metrics_are_hidden_when_the_status_item_is_off_or_nothing_happened() {
        let mut app = app_with_session_metrics();
        app.status_items = vec![crate::config::StatusItem::Cache];
        let text = activity_text(&mut app, 120);
        assert!(!text.contains("turns"), "{text}");
        // The legacy standalone cache chip still serves users who turned
        // the strip off.
        assert!(text.contains("cache 99%"), "{text}");

        let mut fresh = test_app();
        fresh.status_items = vec![crate::config::StatusItem::SessionMetrics];
        let text = activity_text(&mut fresh, 120);
        assert!(!text.contains("turns"), "{text}");
        assert!(!text.contains('│'), "{text}");
    }

    /// Live state and route identity used to share one row; the gutter kept
    /// them apart. They now live on separate rows entirely — the activity
    /// band owns the phase verb, the identity band owns the route, and
    /// neither leaks into the other.
    #[test]
    fn live_state_and_route_identity_live_on_separate_rows() {
        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::En;
        let activity = activity_text(&mut app, 120);
        assert!(activity.contains("idle"), "{activity}");
        assert!(!activity.contains("DeepSeek"), "{activity}");
        assert!(!activity.contains("deepseek"), "{activity}");

        let identity = identity_text(&mut app, 120);
        // Peers inside the identity group keep the middle dot.
        assert!(
            identity.contains("DeepSeek · deepseek-v4-flash · max"),
            "{identity}"
        );
        assert!(!identity.contains("idle"), "{identity}");
    }

    /// `/context:context` cost eighteen columns to advertise something the
    /// composer announces the moment you type `/`. The rail now reads the
    /// same at 80 columns as at 200.
    #[test]
    fn the_rail_advertises_chords_not_slash_commands() {
        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::En;
        for width in [80, 120, 200] {
            let text = identity_text(&mut app, width);
            assert!(text.contains("keys"), "{width}: {text}");
            assert!(!text.contains("/context"), "{width}: {text}");
        }
    }

    #[test]
    fn notice_clauses_split_on_sentences_and_keep_versions_and_paths_whole() {
        assert_eq!(
            notice_clauses(
                "Counts are on. Code is never collected. See docs/T.md",
                &SENTENCE_MARKS
            ),
            vec![
                "Counts are on.",
                "Code is never collected.",
                "See docs/T.md"
            ]
        );
        assert_eq!(
            notice_clauses("Updated to 0.9.11 from 0.9.10", &SENTENCE_MARKS),
            vec!["Updated to 0.9.11 from 0.9.10"]
        );
        // Full-width stops carry no trailing space, so they break on sight.
        assert_eq!(
            notice_clauses(
                "匿名の利用回数は有効です。会話とコードは収集されません。",
                &SENTENCE_MARKS
            ),
            vec![
                "匿名の利用回数は有効です。",
                "会話とコードは収集されません。"
            ]
        );
        // A colon inside a URL is not a joint.
        assert_eq!(
            notice_clauses("Docs: https://example.test/x", &CLAUSE_MARKS),
            vec!["Docs:", "https://example.test/x"]
        );
    }

    #[test]
    fn shed_clauses_rejoin_without_a_latin_space_after_a_full_width_stop() {
        const JA: &str = "匿名の利用状況集計はオンです。会話やコードは一切収集しません。/settings で変更できます。スキーマ: docs/TELEMETRY.md";
        let clauses = notice_clauses(JA, &SENTENCE_MARKS);
        let joined = join_while_fitting(&clauses, 200).expect("fits");
        assert!(!joined.contains("。 "), "{joined:?}");
        assert!(joined.ends_with("docs/TELEMETRY.md"), "{joined:?}");
    }

    #[test]
    fn a_notice_sheds_whole_clauses_and_never_dangles() {
        const NOTICE: &str = "Anonymous usage counts are on. Conversations and code are never collected. Change this in /settings; schema: docs/TELEMETRY.md";
        assert_eq!(fit_notice(NOTICE, 200).as_deref(), Some(NOTICE));
        assert_eq!(
            fit_notice(NOTICE, 80).as_deref(),
            Some("Anonymous usage counts are on. Conversations and code are never collected.")
        );
        assert_eq!(
            fit_notice(NOTICE, 40).as_deref(),
            Some("Anonymous usage counts are on.")
        );
        assert_eq!(fit_notice("   ", 40), None);
    }

    /// The failure this caught: a one-sentence warning longer than the row
    /// used to have no sentence joint to shed at, so the rail dropped the
    /// whole warning. Inner joints are the fallback, and the phrase that
    /// survives never ends on a `:` or `;` — that mark says "more is coming"
    /// as loudly as an ellipsis does.
    #[test]
    fn a_clause_less_warning_sheds_at_inner_joints_rather_than_vanishing() {
        const WARNING: &str =
            "Auto-denied exec_shell: denied earlier; restart Codewhale to re-enable it.";
        assert_eq!(fit_notice(WARNING, 120).as_deref(), Some(WARNING));
        assert_eq!(
            fit_notice(WARNING, 60).as_deref(),
            Some("Auto-denied exec_shell: denied earlier")
        );
        assert_eq!(
            fit_notice(WARNING, 30).as_deref(),
            Some("Auto-denied exec_shell")
        );

        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::En;
        app.push_status_toast(WARNING, crate::tui::app::StatusToastLevel::Warning, None);
        for width in [60u16, 72, 80, 100, 120] {
            let text = activity_text(&mut app, width);
            assert!(
                text.contains("Auto-denied exec_shell"),
                "{width} dropped the warning: {text}"
            );
            assert!(!text.contains('…'), "{width} dangled: {text}");
        }
    }

    /// Identity is standing information; a notice is something the session
    /// is telling you. They no longer compete for one row, so severity now
    /// arbitrates only among the activity band's own transient groups: a
    /// routine notice yields to the work detail, an urgent one evicts it.
    /// The route row is untouched in every case.
    #[test]
    fn routine_notices_yield_to_work_detail_but_urgent_ones_do_not() {
        let mut app = running_tool_app("cargo build -p tui");
        app.push_status_toast(
            "Anonymous usage counts are on. Conversations and code are never collected. Change this in /settings; schema: docs/TELEMETRY.md",
            crate::tui::app::StatusToastLevel::Info,
            Some(12_000),
        );

        // Wide: the receipt and the work detail coexist, and the route row
        // carries the model the whole time.
        let wide = activity_text(&mut app, 140);
        assert!(wide.contains("Anonymous usage counts are on."), "{wide}");
        assert!(wide.contains("using tool"), "{wide}");
        assert!(
            identity_text(&mut app, 140).contains("deepseek-v4-flash"),
            "a routine notice must not disturb the route row"
        );

        // Narrow: the routine receipt sheds clauses before the work detail
        // stands down, and never dangles.
        let narrow = activity_text(&mut app, 90);
        assert!(narrow.contains("using tool"), "{narrow}");
        assert!(!narrow.contains('…'), "no dangling ellipsis: {narrow}");
        assert!(
            identity_text(&mut app, 90).contains("deepseek-v4-flash"),
            "the route survives at 90"
        );

        // An urgent notice is actionable and does get the columns when the
        // band cannot hold both. (Below 60 columns the compact band keeps
        // only the semantic label — notices included.)
        let mut urgent = running_tool_app("cargo build -p tui");
        urgent.push_status_toast(
            "Auto-denied exec_shell: denied earlier; restart Codewhale to re-enable it.",
            crate::tui::app::StatusToastLevel::Warning,
            None,
        );
        for width in [60u16, 80, 120] {
            let text = activity_text(&mut urgent, width);
            assert!(
                text.contains("Auto-denied exec_shell"),
                "{width} dropped an actionable warning: {text}"
            );
            assert!(!text.contains('…'), "{width} dangled: {text}");
            assert!(
                identity_text(&mut urgent, width).contains("deepseek-v4-flash"),
                "{width}: an urgent notice must never evict the route row"
            );
        }
    }

    #[test]
    fn nothing_on_either_band_advertises_truncation() {
        let mut app = app_with_session_metrics();
        app.push_status_toast(
            "Anonymous usage counts are on. Conversations and code are never collected. Change this in /settings; schema: docs/TELEMETRY.md",
            crate::tui::app::StatusToastLevel::Info,
            Some(12_000),
        );
        for width in [40u16, 50, 59, 60, 72, 80, 100, 120, 160] {
            let activity = activity_text(&mut app, width);
            assert!(
                !activity.contains('…'),
                "{width} activity dangled: {activity}"
            );
            let identity = identity_text(&mut app, width);
            assert!(
                !identity.contains('…'),
                "{width} identity dangled: {identity}"
            );
        }
    }

    /// `deepseek-v4-flash-prev…` could be any of several routes. A clipped
    /// model name is worse than no model name, so the qualifiers go first.
    #[test]
    fn identity_sheds_qualifiers_before_it_would_clip_a_model_name() {
        let model = "deepseek-v4-flash-preview-2026-05-01";
        let mut app = App::new(
            TuiOptions {
                model: model.to_string(),
                ..crate::test_support::test_tui_options(PathBuf::from("."))
            },
            &Config::default(),
        );
        app.ui_locale = crate::localization::Locale::En;

        let wide = identity_text(&mut app, 140);
        assert!(
            wide.contains("DeepSeek · deepseek-v4-flash-preview-2026-05-01 · max"),
            "{wide}"
        );

        // Standard: identity outranks the key chorus now, so the provider
        // survives beside the model wherever the whole group fits.
        let standard = identity_text(&mut app, 60);
        assert!(standard.contains(model), "{standard}");

        // Below the full group's width the provider sheds first; the model
        // stays whole.
        let shed_provider = identity_text(&mut app, 46);
        assert!(shed_provider.contains(model), "{shed_provider}");
        assert!(
            !shed_provider.contains("DeepSeek ·"),
            "provider sheds first: {shed_provider}"
        );

        // Narrow and very narrow: the model appears whole or not at all.
        for width in [30u16, 34, 40, 50] {
            let narrow = identity_text(&mut app, width);
            assert!(!narrow.contains('…'), "{width} dangled: {narrow}");
            if narrow.contains("deepseek-v4-flash-p") {
                assert!(
                    narrow.contains(model),
                    "{width} clipped the model name: {narrow}"
                );
            }
        }
    }

    /// A named custom route can carry a long provider identity next to a
    /// long model id. The identity band must shed whole fields — provider
    /// first, then the effort label — and stand down entirely rather than
    /// clip either name, across compact, standard, and wide rows.
    #[test]
    fn long_custom_route_names_shed_whole_fields_across_width_tiers() {
        let model = "deepseek-v4-flash-vision-preview-2026-08-01";
        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::En;
        app.set_provider_identity(
            crate::config::ApiProvider::Custom,
            "acme-research-gateway-eu-central",
        );
        app.model = model.to_string();

        let wide = identity_text(&mut app, 160);
        assert!(wide.contains("acme-research-gateway-eu-central"), "{wide}");
        assert!(wide.contains(model), "{wide}");

        // Standard: the provider sheds before the model name is touched.
        let standard = identity_text(&mut app, 80);
        assert!(standard.contains(model), "{standard}");
        assert!(
            !standard.contains("acme-research-gateway-eu-central"),
            "{standard}"
        );

        // Narrow and very narrow: the model appears whole or not at all,
        // and no row ever clips a name.
        for width in [30u16, 40, 50, 60, 70] {
            let narrow = identity_text(&mut app, width);
            assert!(!narrow.contains('…'), "{width} dangled: {narrow}");
            if narrow.contains("deepseek-v4-flash-vision") {
                assert!(
                    narrow.contains(model),
                    "{width} clipped the model name: {narrow}"
                );
            }
            assert!(
                !narrow.contains("acme-research-gateway-eu-c"),
                "{width} clipped the provider name: {narrow}"
            );
        }
    }

    #[test]
    fn session_metrics_strip_is_on_by_default() {
        assert!(
            crate::config::StatusItem::default_footer()
                .contains(&crate::config::StatusItem::SessionMetrics)
        );
        assert_eq!(
            crate::config::StatusItem::from_key("session_metrics"),
            Some(crate::config::StatusItem::SessionMetrics)
        );
    }
}
