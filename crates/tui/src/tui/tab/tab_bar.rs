//! Tab bar renderer for displaying tabs at the top of the screen

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
};

use crate::tui::app::App;
use crate::tui::tab::group::GroupColor;

/// Tab bar height in rows
pub const TAB_BAR_HEIGHT: u16 = 1;

/// Map a GroupColor to a ratatui Color
fn group_color_to_ratatui(color: GroupColor) -> ratatui::style::Color {
    match color {
        GroupColor::Red => ratatui::style::Color::Red,
        GroupColor::Orange => ratatui::style::Color::Rgb(255, 165, 0),
        GroupColor::Yellow => ratatui::style::Color::Yellow,
        GroupColor::Green => ratatui::style::Color::Green,
        GroupColor::Cyan => ratatui::style::Color::Cyan,
        GroupColor::Blue => ratatui::style::Color::Blue,
        GroupColor::Magenta => ratatui::style::Color::Magenta,
        GroupColor::Gray => ratatui::style::Color::Gray,
    }
}

/// Render the tab bar at the top of the screen
///
/// Shows the current tabs with their titles, highlighting the active tab.
/// Format: `[1: Tab1] [2: Tab2*] [3: Tab3]` where `*` marks the active tab.
pub fn render_tab_bar(area: Rect, buf: &mut Buffer, app: &App) {
    if area.height < TAB_BAR_HEIGHT {
        return;
    }

    let bg_style = Style::default().bg(ratatui::style::Color::DarkGray);
    for x in area.x..area.x + area.width {
        buf.set_string(x, area.y, " ", bg_style);
    }

    let active_index = app.tab_manager.active_index().unwrap_or(0);

    if app.tab_manager.is_empty() {
        let hint = Span::styled(
            " [Ctrl+Shift+N] New tab  Ctrl+`: Switcher ",
            Style::default().fg(ratatui::style::Color::White),
        );
        buf.set_span(area.x, area.y, &hint, area.width);
        return;
    }

    let mut x = area.x;
    for (i, tab) in app.tab_manager.iter() {
        if x >= area.x + area.width {
            break;
        }

        let is_active = i == active_index;
        let title = if tab.metadata.title.chars().count() > 8 {
            let truncated: String = tab.metadata.title.chars().take(7).collect();
            format!("{truncated}…")
        } else {
            tab.metadata.title.clone()
        };
        let icon = tab.metadata.tab_type.icon();

        // Look up the group once and reuse for both the tag and the active style.
        let group = app.tab_manager.tab_group(tab.metadata.id);
        let group_tag = group
            .map(|g| format!("⟨{}⟩", g.color.short()))
            .unwrap_or_default();

        // Build the display string only once for the active case, and once
        // for the inactive case — no unconditional `label` allocation.
        let display = if is_active {
            format!("[{} {}:{}{}*]", icon, i + 1, title, group_tag)
        } else {
            format!("[{} {}:{}{}]", icon, i + 1, title, group_tag)
        };

        let style = if is_active {
            let bg = group
                .map(|g| group_color_to_ratatui(g.color))
                .unwrap_or(ratatui::style::Color::Cyan);
            Style::default().fg(ratatui::style::Color::Black).bg(bg)
        } else {
            Style::default()
                .fg(ratatui::style::Color::White)
                .bg(ratatui::style::Color::Reset)
        };

        let display_len = display.chars().count() as u16;
        if x + display_len > area.x + area.width {
            break;
        }

        let line = Line::from(vec![Span::styled(display, style)]);
        buf.set_line(x, area.y, &line, display_len);

        x += display_len;
        if x < area.x + area.width {
            buf.set_string(x, area.y, " ", Style::default());
            x += 1;
        }
    }

    // Show help on the right
    if x < area.x + area.width {
        let remaining = area.x + area.width - x;
        if remaining > 20 {
            let hint = " Ctrl+`: Switch";
            let style = Style::default().fg(ratatui::style::Color::DarkGray);
            buf.set_string(x, area.y, hint, style);
        }
    }
}
