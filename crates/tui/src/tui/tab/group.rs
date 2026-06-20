//! Tab groups for organizing related tabs
//!
//! A TabGroup is a named collection of tabs (e.g. "Frontend Refactor",
//! "Backend Bug Hunt"). Groups help users manage 9 tabs by clustering
//! them by project/topic.
//!
//! Groups are purely organizational - they don't change delegation,
//! meeting, or any other tab behavior. They just provide:
//! - Visual separation in the tab bar (color/icon)
//! - Filtering in the tab switcher
//! - Quick group switching (next/prev group)

// WIP collaboration surface — narrow harvest. See `tab/mod.rs` for the
// PR #2753 context.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::TabId;

/// Visual style/color identifier for a group
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum GroupColor {
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    #[default]
    Blue,
    Magenta,
    Gray,
}

impl GroupColor {
    /// Short name for the color (1-3 chars)
    pub fn short(&self) -> &'static str {
        match self {
            GroupColor::Red => "Rd",
            GroupColor::Orange => "Or",
            GroupColor::Yellow => "Yl",
            GroupColor::Green => "Gn",
            GroupColor::Cyan => "Cy",
            GroupColor::Blue => "Bl",
            GroupColor::Magenta => "Mg",
            GroupColor::Gray => "Gy",
        }
    }

    /// Cycle to the next color (used by the group cycle command)
    pub fn next(&self) -> Self {
        match self {
            GroupColor::Red => GroupColor::Orange,
            GroupColor::Orange => GroupColor::Yellow,
            GroupColor::Yellow => GroupColor::Green,
            GroupColor::Green => GroupColor::Cyan,
            GroupColor::Cyan => GroupColor::Blue,
            GroupColor::Blue => GroupColor::Magenta,
            GroupColor::Magenta => GroupColor::Gray,
            GroupColor::Gray => GroupColor::Red,
        }
    }
}

/// A named collection of tabs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabGroup {
    pub id: String,
    pub name: String,
    pub color: GroupColor,
    pub tab_ids: Vec<TabId>,
    pub created_at: DateTime<Utc>,
}

impl TabGroup {
    pub fn new(name: String, color: GroupColor) -> Self {
        let id = format!("group_{}", chrono::Utc::now().timestamp_millis());
        Self {
            id,
            name,
            color,
            tab_ids: Vec::new(),
            created_at: Utc::now(),
        }
    }

    /// Add a tab to this group
    pub fn add_tab(&mut self, tab_id: TabId) {
        if !self.tab_ids.contains(&tab_id) {
            self.tab_ids.push(tab_id);
        }
    }

    /// Remove a tab from this group
    pub fn remove_tab(&mut self, tab_id: TabId) -> bool {
        if let Some(pos) = self.tab_ids.iter().position(|t| *t == tab_id) {
            self.tab_ids.swap_remove(pos);
            true
        } else {
            false
        }
    }

    /// Number of tabs in this group
    pub fn len(&self) -> usize {
        self.tab_ids.len()
    }

    /// Check if the group is empty
    pub fn is_empty(&self) -> bool {
        self.tab_ids.is_empty()
    }

    /// Check if this group contains a tab
    pub fn contains(&self, tab_id: TabId) -> bool {
        self.tab_ids.contains(&tab_id)
    }
}

/// Manager for tab groups
pub struct TabGroupManager {
    pub(crate) groups: HashMap<String, TabGroup>,
    /// Maps tab_id -> group_id for quick lookup
    pub(crate) tab_to_group: HashMap<TabId, String>,
    next_id: u64,
}

impl TabGroupManager {
    pub fn new() -> Self {
        Self {
            groups: HashMap::new(),
            tab_to_group: HashMap::new(),
            next_id: 1,
        }
    }

    /// Create a new group
    pub fn create_group(&mut self, name: String, color: GroupColor) -> String {
        let id = self.generate_group_id();
        let group = TabGroup {
            id: id.clone(),
            name,
            color,
            tab_ids: Vec::new(),
            created_at: Utc::now(),
        };
        self.groups.insert(id.clone(), group);
        id
    }

    /// Delete a group (tabs themselves are not deleted)
    pub fn delete_group(&mut self, group_id: &str) -> bool {
        if let Some(group) = self.groups.remove(group_id) {
            for tab_id in &group.tab_ids {
                self.tab_to_group.remove(tab_id);
            }
            true
        } else {
            false
        }
    }

    /// Assign a tab to a group
    pub fn assign_tab(&mut self, tab_id: TabId, group_id: &str) -> bool {
        // Remove from any previous group first
        self.unassign_tab(tab_id);

        if let Some(group) = self.groups.get_mut(group_id) {
            group.add_tab(tab_id);
            self.tab_to_group.insert(tab_id, group_id.to_string());
            true
        } else {
            false
        }
    }

    /// Remove a tab from its group (if any)
    pub fn unassign_tab(&mut self, tab_id: TabId) {
        if let Some(prev_group_id) = self.tab_to_group.remove(&tab_id)
            && let Some(group) = self.groups.get_mut(&prev_group_id)
        {
            group.remove_tab(tab_id);
        }
    }

    /// Get the group a tab is assigned to
    pub fn group_of(&self, tab_id: TabId) -> Option<&TabGroup> {
        self.tab_to_group
            .get(&tab_id)
            .and_then(|id| self.groups.get(id))
    }

    /// Get all groups
    pub fn all_groups(&self) -> Vec<&TabGroup> {
        let mut groups: Vec<&TabGroup> = self.groups.values().collect();
        // Sort by name for stable display
        groups.sort_by(|a, b| a.name.cmp(&b.name));
        groups
    }

    /// Get tabs in a specific group
    pub fn tabs_in_group(&self, group_id: &str) -> Option<&Vec<TabId>> {
        self.groups.get(group_id).map(|g| &g.tab_ids)
    }

    /// Get the number of groups
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// Iterate over groups
    pub fn iter(&self) -> impl Iterator<Item = (&String, &TabGroup)> {
        self.groups.iter()
    }

    /// Cycle a tab to the next group (or unassign if at the end)
    pub fn cycle_tab_group(&mut self, tab_id: TabId) {
        let group_ids: Vec<String> = self.all_groups().iter().map(|g| g.id.clone()).collect();

        if let Some(current_group_id) = self.tab_to_group.get(&tab_id).cloned() {
            if let Some(pos) = group_ids.iter().position(|id| id == &current_group_id) {
                if pos + 1 < group_ids.len() {
                    let next = group_ids[pos + 1].clone();
                    self.assign_tab(tab_id, &next);
                } else {
                    self.unassign_tab(tab_id);
                }
            }
        } else if !group_ids.is_empty() {
            let first = group_ids[0].clone();
            self.assign_tab(tab_id, &first);
        }
    }

    fn generate_group_id(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 1;
        format!("group_{id}")
    }

    pub(crate) fn advance_next_id_past_existing_groups(&mut self) {
        let max_seen = self
            .groups
            .keys()
            .filter_map(|id| id.strip_prefix("group_"))
            .filter_map(|suffix| suffix.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max_seen + 1);
    }
}

impl Default for TabGroupManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_delete_group() {
        let mut mgr = TabGroupManager::new();
        let id = mgr.create_group("Frontend".to_string(), GroupColor::Blue);
        assert_eq!(mgr.group_count(), 1);
        assert!(mgr.delete_group(&id));
        assert_eq!(mgr.group_count(), 0);
    }

    #[test]
    fn test_assign_tab() {
        let mut mgr = TabGroupManager::new();
        let group_id = mgr.create_group("Backend".to_string(), GroupColor::Green);
        let tab1 = TabId::new(1);
        let tab2 = TabId::new(2);

        assert!(mgr.assign_tab(tab1, &group_id));
        assert!(mgr.assign_tab(tab2, &group_id));

        let group = mgr.group_of(tab1).unwrap();
        assert_eq!(group.len(), 2);
        assert!(group.contains(tab1));
    }

    #[test]
    fn test_unassign_tab() {
        let mut mgr = TabGroupManager::new();
        let group_id = mgr.create_group("Test".to_string(), GroupColor::Red);
        let tab1 = TabId::new(1);
        mgr.assign_tab(tab1, &group_id);

        mgr.unassign_tab(tab1);
        assert!(mgr.group_of(tab1).is_none());

        let group = mgr.groups.get(&group_id).unwrap();
        assert_eq!(group.len(), 0);
    }

    #[test]
    fn test_reassign_tab() {
        let mut mgr = TabGroupManager::new();
        let g1 = mgr.create_group("G1".to_string(), GroupColor::Blue);
        let g2 = mgr.create_group("G2".to_string(), GroupColor::Red);
        let tab1 = TabId::new(1);

        mgr.assign_tab(tab1, &g1);
        assert_eq!(mgr.group_of(tab1).unwrap().id, g1);

        mgr.assign_tab(tab1, &g2);
        assert_eq!(mgr.group_of(tab1).unwrap().id, g2);
        // G1 should now be empty
        let g1_ref = mgr.groups.get(&g1).unwrap();
        assert_eq!(g1_ref.len(), 0);
    }

    #[test]
    fn test_color_cycle() {
        let c = GroupColor::Red;
        assert_eq!(c.next(), GroupColor::Orange);
        assert_eq!(c.next().next(), GroupColor::Yellow);
        // Cycle back to Red after Gray
        let gray = GroupColor::Gray;
        assert_eq!(gray.next(), GroupColor::Red);
    }

    #[test]
    fn test_cycle_tab_group() {
        let mut mgr = TabGroupManager::new();
        let g1 = mgr.create_group("G1".to_string(), GroupColor::Blue);
        let g2 = mgr.create_group("G2".to_string(), GroupColor::Red);
        let tab1 = TabId::new(1);

        // Not assigned yet -> assign to first
        mgr.cycle_tab_group(tab1);
        assert_eq!(mgr.group_of(tab1).unwrap().id, g1);

        // Cycle to g2
        mgr.cycle_tab_group(tab1);
        assert_eq!(mgr.group_of(tab1).unwrap().id, g2);

        // Cycle past end -> unassign
        mgr.cycle_tab_group(tab1);
        assert!(mgr.group_of(tab1).is_none());
    }

    #[test]
    fn test_delete_group_clears_assignments() {
        let mut mgr = TabGroupManager::new();
        let g1 = mgr.create_group("G1".to_string(), GroupColor::Blue);
        let tab1 = TabId::new(1);
        mgr.assign_tab(tab1, &g1);

        mgr.delete_group(&g1);
        assert!(mgr.group_of(tab1).is_none());
        assert!(!mgr.tab_to_group.contains_key(&tab1));
    }

    #[test]
    fn test_advance_next_id_after_restore() {
        let mut mgr = TabGroupManager::new();
        mgr.groups.insert(
            "group_7".to_string(),
            TabGroup {
                id: "group_7".to_string(),
                name: "Restored".to_string(),
                color: GroupColor::Blue,
                tab_ids: Vec::new(),
                created_at: Utc::now(),
            },
        );

        mgr.advance_next_id_past_existing_groups();
        let new_id = mgr.create_group("Fresh".to_string(), GroupColor::Green);

        assert_eq!(new_id, "group_8");
        assert!(mgr.groups.contains_key("group_7"));
        assert!(mgr.groups.contains_key("group_8"));
    }
}
