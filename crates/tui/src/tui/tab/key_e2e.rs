//! End-to-end keyboard event tests for tab system
//!
//! These tests simulate the keyboard event handling that happens in
//! `ui.rs` when the user presses tab-related shortcuts. They verify that
//! the TabManager state transitions correctly in response to key events.
//!
//! The actual key event dispatch lives in `ui.rs` (which is hard to
//! test in isolation due to the engine/App dependencies), so these
//! tests exercise the underlying state transitions that the key handlers
//! would trigger.
//!
//! Run with: `cargo test tui::tab::key_e2e -- --nocapture`

#[cfg(test)]
mod tests {
    use crate::tui::tab::{Priority, TabId, TabManager, TabType};

    /// Simulate the sequence of key events the user would press
    /// to: create a new tab, switch to it, type a message, and submit.
    fn simulate_create_and_switch(manager: &mut TabManager) {
        // Ctrl+Shift+N: create new tab
        manager
            .create_tab(format!("Tab {}", manager.len() + 1), TabType::Chat)
            .expect("Ctrl+Shift+N should create tab");

        // The new tab is automatically active after creation,
        // simulating the key handler that updates active_tab.
    }

    /// Simulate Ctrl+1..9 key press
    fn simulate_ctrl_number(manager: &mut TabManager, n: u8) {
        if n == 0 || n as usize > manager.len() {
            return;
        }
        manager.switch_to((n - 1) as usize);
    }

    /// Simulate Ctrl+Tab / Ctrl+Shift+Tab
    fn simulate_ctrl_tab(manager: &mut TabManager, forward: bool) {
        if forward {
            manager.switch_to_next();
        } else {
            manager.switch_to_prev();
        }
    }

    /// Simulate Ctrl+` to open switcher (we just verify the manager
    /// can list its tabs as the switcher would)
    fn simulate_switcher_list(manager: &TabManager) -> Vec<(usize, String)> {
        manager
            .all_tabs()
            .iter()
            .enumerate()
            .map(|(i, t)| (i, t.title.clone()))
            .collect()
    }

    /// Simulate Ctrl+Shift+D: process pending delegations
    fn simulate_process_delegation(manager: &mut TabManager) -> Option<String> {
        let tab_id = manager.active_id()?;
        manager.take_next_delegation(tab_id).map(|t| t.task_id)
    }

    // === Tab creation tests ===

    #[test]
    fn test_e2e_create_first_tab() {
        let mut manager = TabManager::new();
        assert!(manager.is_empty());

        simulate_create_and_switch(&mut manager);
        assert_eq!(manager.len(), 1);
        assert_eq!(manager.active_index(), Some(0));
    }

    #[test]
    fn test_e2e_create_max_tabs() {
        let mut manager = TabManager::new();
        for _ in 0..9 {
            simulate_create_and_switch(&mut manager);
        }
        assert_eq!(manager.len(), 9);

        // 10th should fail
        let result = manager.create_tab("10th".to_string(), TabType::Chat);
        assert!(result.is_none());
    }

    // === Tab switching tests ===

    #[test]
    fn test_e2e_ctrl_number_switches() {
        let mut manager = TabManager::new();
        for i in 1..=5 {
            manager
                .create_tab(format!("Tab {}", i), TabType::Chat)
                .expect("create");
        }

        simulate_ctrl_number(&mut manager, 3);
        assert_eq!(manager.active_index(), Some(2));

        simulate_ctrl_number(&mut manager, 1);
        assert_eq!(manager.active_index(), Some(0));

        simulate_ctrl_number(&mut manager, 5);
        assert_eq!(manager.active_index(), Some(4));
    }

    #[test]
    fn test_e2e_ctrl_number_out_of_range() {
        let mut manager = TabManager::new();
        for i in 1..=3 {
            manager
                .create_tab(format!("Tab {}", i), TabType::Chat)
                .unwrap();
        }

        // Out-of-range should be a no-op
        simulate_ctrl_number(&mut manager, 9);
        assert_eq!(manager.active_index(), Some(2)); // last created
    }

    #[test]
    fn test_e2e_ctrl_tab_cycles() {
        let mut manager = TabManager::new();
        for i in 1..=3 {
            manager
                .create_tab(format!("Tab {}", i), TabType::Chat)
                .unwrap();
        }
        // Initially at last (2)
        assert_eq!(manager.active_index(), Some(2));

        simulate_ctrl_tab(&mut manager, true);
        assert_eq!(manager.active_index(), Some(0)); // wrap

        simulate_ctrl_tab(&mut manager, true);
        assert_eq!(manager.active_index(), Some(1));

        simulate_ctrl_tab(&mut manager, false);
        assert_eq!(manager.active_index(), Some(0));
    }

    // === Switcher listing tests ===

    #[test]
    fn test_e2e_switcher_lists_all_tabs() {
        let mut manager = TabManager::new();
        manager.create_tab("A".to_string(), TabType::Chat).unwrap();
        manager
            .create_tab("B".to_string(), TabType::Review)
            .unwrap();
        manager
            .create_tab("C".to_string(), TabType::Meeting)
            .unwrap();

        let listed = simulate_switcher_list(&manager);
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].1, "A");
        assert_eq!(listed[1].1, "B");
        assert_eq!(listed[2].1, "C");
    }

    // === Delegation tests ===

    #[test]
    fn test_e2e_delegate_and_process() {
        let mut manager = TabManager::new();
        let from = manager
            .create_tab("Source".to_string(), TabType::Chat)
            .unwrap();
        let to = manager
            .create_tab("Target".to_string(), TabType::Chat)
            .unwrap();

        // User delegates a task
        let task_id = manager
            .delegate_task(from, to, "Review PR".to_string(), Priority::High)
            .expect("delegate");

        // User switches to target tab
        manager.switch_to_by_id(to);

        // User presses Ctrl+Shift+D
        let processed = simulate_process_delegation(&mut manager);
        assert_eq!(processed, Some(task_id));
    }

    #[test]
    fn test_e2e_no_pending_delegation_returns_none() {
        let mut manager = TabManager::new();
        let _ = manager
            .create_tab("Solo".to_string(), TabType::Chat)
            .unwrap();

        let processed = simulate_process_delegation(&mut manager);
        assert_eq!(processed, None);
    }

    #[test]
    fn test_e2e_delegation_priority_drain() {
        let mut manager = TabManager::new();
        let from = manager.create_tab("S".to_string(), TabType::Chat).unwrap();
        let to = manager.create_tab("T".to_string(), TabType::Chat).unwrap();

        manager.delegate_task(from, to, "Low".to_string(), Priority::Low);
        manager.delegate_task(from, to, "Urgent".to_string(), Priority::Urgent);
        manager.delegate_task(from, to, "High".to_string(), Priority::High);
        manager.delegate_task(from, to, "Normal".to_string(), Priority::Normal);

        manager.switch_to_by_id(to);

        // Press Ctrl+Shift+D 4 times
        assert_eq!(simulate_process_delegation(&mut manager).is_some(), true);
        assert_eq!(simulate_process_delegation(&mut manager).is_some(), true);
        assert_eq!(simulate_process_delegation(&mut manager).is_some(), true);
        // 4th should be the last task
        assert_eq!(simulate_process_delegation(&mut manager).is_some(), true);
        // 5th should be none
        assert_eq!(simulate_process_delegation(&mut manager), None);
    }

    // === Tab close tests ===

    #[test]
    fn test_e2e_close_active_tab() {
        let mut manager = TabManager::new();
        let id_a = manager.create_tab("A".to_string(), TabType::Chat).unwrap();
        let _id_b = manager.create_tab("B".to_string(), TabType::Chat).unwrap();
        let id_c = manager.create_tab("C".to_string(), TabType::Chat).unwrap();

        // Switch to B (index 1)
        manager.switch_to(1);

        // Ctrl+Shift+W: close current
        manager.close_tab(1);
        assert_eq!(manager.len(), 2);
        // Active should now be C (index 1) since B was removed.
        // C is the previously-created 3rd tab.
        assert_eq!(manager.active_id().unwrap(), id_c);
        assert!(id_a != id_c);
    }

    #[test]
    fn test_e2e_close_only_tab_clears_active() {
        let mut manager = TabManager::new();
        manager
            .create_tab("Solo".to_string(), TabType::Chat)
            .unwrap();
        manager.close_tab(0);
        assert!(manager.is_empty());
        assert_eq!(manager.active_index(), None);
    }

    // === Group management tests ===

    #[test]
    fn test_e2e_cycle_through_groups() {
        use crate::tui::tab::group::GroupColor;

        let mut manager = TabManager::new();
        let tab1 = manager.create_tab("T1".to_string(), TabType::Chat).unwrap();
        let _g1 = manager.create_group("A".to_string(), GroupColor::Blue);
        let _g2 = manager.create_group("B".to_string(), GroupColor::Red);

        // Cycle: not assigned -> first group (A)
        manager.cycle_tab_group(tab1);
        assert!(manager.tab_group(tab1).is_some());

        // Cycle: A -> B
        manager.cycle_tab_group(tab1);
        let group = manager.tab_group(tab1).unwrap();
        assert_eq!(group.name, "B");

        // Cycle: B -> unassigned
        manager.cycle_tab_group(tab1);
        assert!(manager.tab_group(tab1).is_none());
    }

    // === Persistence e2e ===

    #[test]
    fn test_e2e_full_workflow_save_load() {
        use std::path::Path;

        let dir = std::env::temp_dir().join("codewhale_e2e_persist");
        let path = dir.join("tabs.json");
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(&path);

        // Session 1: User creates tabs and groups
        let mut manager = TabManager::new();
        manager
            .create_tab("Work".to_string(), TabType::Chat)
            .unwrap();
        manager
            .create_tab("Personal".to_string(), TabType::Chat)
            .unwrap();
        let group_id = manager.create_group(
            "Default".to_string(),
            crate::tui::tab::group::GroupColor::Blue,
        );
        manager.assign_tab_to_group(manager.all_tabs()[0].id, &group_id);
        manager.switch_to(1);

        // App saves on shutdown
        manager.save_to_file(&path).unwrap();

        // Session 2: Restore
        let mut restored = TabManager::new();
        restored.restore_from_file(&path).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.active_index(), Some(1));
        assert!(restored.tab_group(restored.all_tabs()[0].id).is_some());

        // Cleanup
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    // === Edge case tests ===

    #[test]
    fn test_e2e_rapid_create_close() {
        let mut manager = TabManager::new();
        for i in 0..9 {
            manager
                .create_tab(format!("T{}", i), TabType::Chat)
                .expect("create");
        }
        // Close all
        for _ in 0..9 {
            manager.close_tab(manager.active_index().unwrap());
        }
        assert!(manager.is_empty());
    }

    #[test]
    fn test_e2e_switch_empty_manager() {
        let mut manager = TabManager::new();
        simulate_ctrl_tab(&mut manager, true);
        // Should be a no-op
        assert!(manager.is_empty());
        simulate_ctrl_number(&mut manager, 1);
        assert!(manager.is_empty());
    }
}
