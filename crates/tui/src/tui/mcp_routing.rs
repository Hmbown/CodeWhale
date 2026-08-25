//! MCP UI message helpers.
//!
//! The MCP manager surface itself is the Extensions modal's MCP tab
//! (`crate::tui::views::extensions`); the stand-alone read-only manager pager
//! that used to live here opened only after a serial connect-all of every
//! configured server and offered no actions, so it was removed.

use crate::tui::app::App;
use crate::tui::history::HistoryCell;

pub(super) fn add_mcp_message(app: &mut App, content: String) {
    app.add_message(HistoryCell::System { content });
}
