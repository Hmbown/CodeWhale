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

    // Background
    let bg_style = Style::default().bg(ratatui::style::Color::DarkGray);
    for x in area.x..area.x + area.width {
        buf.set_string(x, area.y, " ", bg_style);
    }

    let tabs = app.tab_manager.all_tabs();
    let active_index = app.tab_manager.active_index().unwrap_or(0);

    if tabs.is_empty() {
        let hint = Span::styled(
            " [Ctrl+Shift+N] New tab  Ctrl+`: Switcher ",
            Style::default().fg(ratatui::style::Color::White),
        );
        buf.set_span(area.x, area.y, &hint, (area.width as usize).try_into().unwrap_or(u16::MAX));
        return;
    }

    // Render each tab
    let mut x = area.x;
    for (i, tab) in tabs.iter().enumerate() {
        if x >= area.x + area.width {
            break;
        }

        let is_active = i == active_index;
        let number = format!("{}", i + 1);
        let title = if tab.title.chars().count() > 8 {
            let truncated: String = tab.title.chars().take(7).collect();
            format!("{truncated}…")
        } else {
            tab.title.clone()
        };
        let icon = tab.tab_type.icon();

        // If the tab is in a group, show a group color tag
        let group_tag = app
            .tab_manager
            .tab_group(tab.id)
            .map(|g| format!("⟨{}⟩", g.color.short()))
            .unwrap_or_default();

        // Format: ` [💬 1: title ⟨Bl⟩]` or ` [💬 1: title ⟨Bl⟩*]`
        let label = format!("[{} {}:{}{}]", icon, number, title, group_tag);
        let style = if is_active {
            // Use group color if present, otherwise default cyan
            let bg = app
                .tab_manager
                .tab_group(tab.id)
                .map(|g| group_color_to_ratatui(g.color))
                .unwrap_or(ratatui::style::Color::Cyan);
            Style::default()
                .fg(ratatui::style::Color::Black)
                .bg(bg)
        } else {
            Style::default()
                .fg(ratatui::style::Color::White)
                .bg(ratatui::style::Color::Reset)
        };

        // Render the tab with active marker
        let display = if is_active {
            format!("[{} {}:{}{}*]", icon, number, title, group_tag)
        } else {
            label
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
