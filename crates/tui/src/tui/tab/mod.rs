//! Multi-tab system for CodeWhale TUI
//!
//! This module provides support for multiple concurrent agent sessions
//! in a tabbed interface, similar to Claude Code Windows.

// Cross-tab collaboration APIs (delegator, meeting, cross_tab, group,
// mention) are intentionally exposed here as a public surface for the
// narrow tab-core harvest in PR #2753. They are not yet wired into the
// TUI host (that lands in a follow-up UI pass) and therefore trip
// `dead_code` inside the binary crate. See the PR description for the
// v0.9 harvest plan agreed with @Hmbown.
#![allow(dead_code)]

#[cfg(test)]
mod benches;
mod cross_tab;
mod delegator;
pub mod group;
#[cfg(test)]
mod key_e2e;
mod manager;
pub mod meeting;
pub mod mention;
pub mod persistence;
#[cfg(test)]
mod render_tests;
mod tab_bar;

pub use manager::TabManager;
pub use tab_bar::{TAB_BAR_HEIGHT, render_tab_bar};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Unique identifier for a tab
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabId(pub u64);

impl TabId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Tab type determining the session mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TabType {
    /// Regular conversation session
    #[default]
    Chat,
    /// Task delegation session
    Delegation,
    /// Code review session
    Review,
    /// Multi-agent meeting session
    Meeting,
}

impl TabType {
    /// Short single-character icon for display in tight UI (tab bar, picker)
    pub fn icon(&self) -> &'static str {
        match self {
            TabType::Chat => "💬",
            TabType::Delegation => "📤",
            TabType::Review => "🔍",
            TabType::Meeting => "👥",
        }
    }

    /// ASCII fallback icon (for terminals without emoji support)
    pub fn ascii_icon(&self) -> &'static str {
        match self {
            TabType::Chat => "[C]",
            TabType::Delegation => "[D]",
            TabType::Review => "[R]",
            TabType::Meeting => "[M]",
        }
    }

    /// Display name for the tab type
    pub fn display_name(&self) -> &'static str {
        match self {
            TabType::Chat => "Chat",
            TabType::Delegation => "Delegation",
            TabType::Review => "Review",
            TabType::Meeting => "Meeting",
        }
    }
}

/// Metadata for a tab
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabMetadata {
    pub id: TabId,
    pub title: String,
    pub tab_type: TabType,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
    pub unread_count: usize,
    pub agent_name: Option<String>,
    pub session_path: Option<PathBuf>,
}

impl TabMetadata {
    pub fn new(id: TabId, title: String, tab_type: TabType) -> Self {
        let now = Utc::now();
        Self {
            id,
            title,
            tab_type,
            created_at: now,
            last_active: now,
            unread_count: 0,
            agent_name: None,
            session_path: None,
        }
    }

    pub fn touch(&mut self) {
        self.last_active = Utc::now();
    }

    pub fn increment_unread(&mut self) {
        self.unread_count += 1;
    }

    pub fn clear_unread(&mut self) {
        self.unread_count = 0;
    }
}

/// Priority levels for task delegation
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum Priority {
    Low = 0,
    #[default]
    Normal = 1,
    High = 2,
    Urgent = 3,
}

/// Status of a tab
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TabStatus {
    Active,
    #[default]
    Idle,
    Loading,
    Error,
}
