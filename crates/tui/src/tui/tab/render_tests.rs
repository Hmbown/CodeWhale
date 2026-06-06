//! End-to-end render tests using ratatui's TestBackend
//!
//! These tests verify the visual output of the tab system widgets without
//! requiring a real terminal. They catch regressions in:
//! - Tab bar layout (alignment, truncation, wrapping)
//! - Active tab highlighting
//! - Group color rendering
//! - Picker / switcher dialog rendering
//!
//! Run with: `cargo test tui::tab::render_tests`

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    use crate::tui::app::App;
    use crate::tui::tab::TabType;
    use crate::tui::tab::group::GroupColor;
    use crate::tui::tab::tab_bar::{TAB_BAR_HEIGHT, render_tab_bar};

    /// Helper: render the tab bar to a string buffer and return it
    fn render_to_string<F>(width: u16, render_fn: F) -> String
    where
        F: FnOnce(&mut Buffer, ratatui::layout::Rect),
    {
        let backend = TestBackend::new(width, TAB_BAR_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                render_fn(f.buffer_mut(), area);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer_to_string(&buffer)
    }

    fn buffer_to_string(buf: &Buffer) -> String {
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = buf.cell((x, y)).unwrap();
                s.push_str(cell.symbol());
            }
            s.push('\n');
        }
        s
    }

    fn make_test_app(tabs: Vec<(&str, TabType)>) -> App {
        let mut app = App::new_for_test();
        for (title, tab_type) in tabs {
            app.tab_manager.create_tab(title.to_string(), tab_type);
        }
        app
    }

    #[test]
    fn test_render_empty_bar() {
        let app = make_test_app(vec![]);
        let output = render_to_string(80, |buf, area| {
            render_tab_bar(area, buf, &app);
        });
        // Should show the "new tab" hint
        assert!(output.contains("New tab"));
        assert!(output.contains("Ctrl"));
    }

    #[test]
    fn test_render_single_tab_no_highlight() {
        // Single tab: no number prefix, no active marker
        let app = make_test_app(vec![("Solo", TabType::Chat)]);
        let output = render_to_string(80, |buf, area| {
            render_tab_bar(area, buf, &app);
        });
        assert!(output.contains("Solo"));
        // Should have 💬 icon
        assert!(output.contains("💬"));
    }

    #[test]
    fn test_render_multiple_tabs() {
        let app = make_test_app(vec![
            ("First", TabType::Chat),
            ("Second", TabType::Review),
            ("Third", TabType::Meeting),
        ]);
        let output = render_to_string(120, |buf, area| {
            render_tab_bar(area, buf, &app);
        });
        // All three titles should be visible
        assert!(output.contains("First"));
        assert!(output.contains("Second"));
        assert!(output.contains("Third"));
        // Active (last) tab should have * marker
        assert!(output.contains("*"));
    }

    #[test]
    fn test_render_with_group_color() {
        let mut app = make_test_app(vec![("A", TabType::Chat), ("B", TabType::Chat)]);
        let group_id = app
            .tab_manager
            .create_group("TestGroup".to_string(), GroupColor::Red);
        let tab_ids: Vec<_> = app.tab_manager.all_tabs().iter().map(|t| t.id).collect();
        if let Some(id) = tab_ids.first() {
            app.tab_manager.assign_tab_to_group(*id, &group_id);
        }
        let output = render_to_string(80, |buf, area| {
            render_tab_bar(area, buf, &app);
        });
        // Should have the group tag
        assert!(
            output.contains("⟨Rd⟩"),
            "Expected group tag, got: {}",
            output
        );
    }

    #[test]
    fn test_render_truncates_long_titles() {
        let app = make_test_app(vec![(
            "This is a very long tab title that should be truncated",
            TabType::Chat,
        )]);
        let output = render_to_string(30, |buf, area| {
            render_tab_bar(area, buf, &app);
        });
        // Should contain ellipsis indicating truncation
        assert!(output.contains("…") || output.contains("..."));
    }

    #[test]
    fn test_render_respects_width() {
        // With 3 long titles, the bar should not exceed its width
        let app = make_test_app(vec![
            ("LongName1", TabType::Chat),
            ("LongName2", TabType::Review),
            ("LongName3", TabType::Meeting),
        ]);
        // Very narrow width
        let output = render_to_string(20, |buf, area| {
            render_tab_bar(area, buf, &app);
        });
        // No line should be longer than 20 chars
        for line in output.lines() {
            // Strip trailing space
            let trimmed = line.trim_end();
            assert!(
                trimmed.chars().count() <= 20,
                "Line exceeds width: '{}' ({} chars)",
                trimmed,
                trimmed.chars().count()
            );
        }
    }

    #[test]
    fn test_render_zero_width() {
        // Should not panic on tiny areas
        let app = make_test_app(vec![("A", TabType::Chat)]);
        let _ = render_to_string(0, |buf, area| {
            render_tab_bar(area, buf, &app);
        });
        let _ = render_to_string(1, |buf, area| {
            render_tab_bar(area, buf, &app);
        });
    }

    #[test]
    fn test_render_different_icons_per_type() {
        let app = make_test_app(vec![
            ("Chat", TabType::Chat),
            ("Del", TabType::Delegation),
            ("Rev", TabType::Review),
            ("Meet", TabType::Meeting),
        ]);
        let output = render_to_string(120, |buf, area| {
            render_tab_bar(area, buf, &app);
        });
        assert!(output.contains("💬"));
        assert!(output.contains("📤"));
        assert!(output.contains("🔍"));
        assert!(output.contains("👥"));
    }

    #[test]
    fn test_render_active_tab_has_marker() {
        let mut app = make_test_app(vec![("A", TabType::Chat), ("B", TabType::Chat)]);
        // Switch to first tab
        app.tab_manager.switch_to(0);
        let output = render_to_string(80, |buf, area| {
            render_tab_bar(area, buf, &app);
        });
        // First tab should be active (has * marker)
        let first_line = output.lines().next().unwrap_or("");
        assert!(
            first_line.contains("*"),
            "First tab should be active, got: {}",
            first_line
        );
    }
}
