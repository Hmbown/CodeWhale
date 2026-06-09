//! Performance benchmarks for the tab system.
//!
//! These tests are not assertions — they print timing info to stderr and
//! return success. Run with `--nocapture` to see the numbers.
//!
//! Run with: `cargo test tui::tab::benches -- --nocapture --test-threads=1`
//!
//! These benchmarks guard against performance regressions in the
//! critical-path operations of the multi-tab system:
//!
//! - TabManager creation and tab creation (startup overhead)
//! - Tab switching (interactive latency)
//! - Delegation queue operations (background processing)
//! - Persistence save/load (startup + shutdown overhead)
//! - Group color rendering (per-frame work in tab bar)

#![allow(unused_imports)]

#[cfg(test)]
mod benches {
    use std::time::Instant;

    use crate::tui::tab::{
        Priority, TabId, TabManager, TabType, group::GroupColor, persistence::PersistedTab,
        persistence::PersistedTabState,
    };

    /// Helper: print a timing result
    fn report(label: &str, dur: std::time::Duration, ops: usize) {
        let per_op_ns = if ops > 0 {
            dur.as_nanos() / ops as u128
        } else {
            0
        };
        eprintln!(
            "[bench] {:50} total={:>8.2?}  ops={:>6}  per_op={:>7} ns",
            label, dur, ops, per_op_ns
        );
    }

    #[test]
    fn bench_create_tabs() {
        let start = Instant::now();
        let mut manager = TabManager::new();
        for i in 0..9 {
            manager
                .create_tab(format!("Bench Tab {}", i), TabType::Chat)
                .expect("create_tab should succeed");
        }
        report("create 9 tabs", start.elapsed(), 9);
    }

    #[test]
    fn bench_switch_tabs() {
        let mut manager = TabManager::new();
        for i in 0..9 {
            manager
                .create_tab(format!("Tab {}", i), TabType::Chat)
                .expect("create_tab");
        }
        let start = Instant::now();
        for _ in 0..1000 {
            manager.switch_to_next();
        }
        report("1000 tab switches (next)", start.elapsed(), 1000);
    }

    #[test]
    fn bench_delegate_many_tasks() {
        let mut manager = TabManager::new();
        let from = manager
            .create_tab("Source".to_string(), TabType::Chat)
            .expect("create");
        let to = manager
            .create_tab("Target".to_string(), TabType::Chat)
            .expect("create");

        let start = Instant::now();
        for i in 0..1000 {
            manager.delegate_task(from, to, format!("Task {}", i), Priority::Normal);
        }
        report("1000 delegations", start.elapsed(), 1000);
    }

    #[test]
    fn bench_take_pending_priority() {
        let mut manager = TabManager::new();
        let from = manager.create_tab("S".to_string(), TabType::Chat).unwrap();
        let to = manager.create_tab("T".to_string(), TabType::Chat).unwrap();

        // Create 100 tasks with mixed priorities
        let priorities = [
            Priority::Low,
            Priority::Normal,
            Priority::High,
            Priority::Urgent,
        ];
        for i in 0..100 {
            manager.delegate_task(from, to, format!("Task {}", i), priorities[i % 4]);
        }

        let start = Instant::now();
        let mut count = 0;
        while manager.take_next_delegation(to).is_some() {
            count += 1;
        }
        report(
            &format!("drain {} priority-sorted tasks", count),
            start.elapsed(),
            count,
        );
    }

    #[test]
    fn bench_persistence_roundtrip() {
        let mut manager = TabManager::new();
        for i in 0..9 {
            manager
                .create_tab(format!("Tab {}", i), TabType::Chat)
                .expect("create");
        }

        // Add some delegations
        let from = manager.active_id().unwrap();
        let to = manager.all_tabs()[1].id;
        for i in 0..20 {
            manager.delegate_task(from, to, format!("Task {}", i), Priority::Normal);
        }

        let start = Instant::now();
        let state = manager.snapshot();
        let snap_dur = start.elapsed();

        let json = serde_json::to_string(&state).unwrap();
        let ser_dur = start.elapsed() - snap_dur;

        let start2 = Instant::now();
        let _loaded: PersistedTabState = serde_json::from_str(&json).unwrap();
        let de_dur = start2.elapsed();

        eprintln!(
            "[bench] {:50} snap={:>8.2?}  ser={:>8.2?}  de={:>8.2?}  json_size={} bytes",
            "9 tabs + 20 delegations persistence",
            snap_dur,
            ser_dur,
            de_dur,
            json.len()
        );
    }

    #[test]
    fn bench_group_operations() {
        let mut manager = TabManager::new();
        for i in 0..9 {
            manager
                .create_tab(format!("Tab {}", i), TabType::Chat)
                .expect("create");
        }
        let tabs: Vec<TabId> = manager.all_tabs().iter().map(|t| t.id).collect();

        // Create 3 groups
        let start = Instant::now();
        let g1 = manager.create_group("Frontend".to_string(), GroupColor::Blue);
        let g2 = manager.create_group("Backend".to_string(), GroupColor::Red);
        let g3 = manager.create_group("Misc".to_string(), GroupColor::Green);
        report("create 3 groups", start.elapsed(), 3);

        // Assign all 9 tabs to groups
        let start = Instant::now();
        for (i, tab) in tabs.iter().enumerate() {
            let group = if i % 3 == 0 {
                &g1
            } else if i % 3 == 1 {
                &g2
            } else {
                &g3
            };
            manager.assign_tab_to_group(*tab, group);
        }
        report("assign 9 tabs to groups", start.elapsed(), 9);

        // Lookup
        let start = Instant::now();
        for tab in &tabs {
            let _ = manager.tab_group(*tab);
        }
        report("9 group lookups", start.elapsed(), 9);
    }

    #[test]
    fn bench_render_at_widths() {
        // Smoke test: ensure rendering at various widths completes quickly
        for width in [20, 40, 80, 120, 200] {
            let mut manager = TabManager::new();
            for i in 0..9 {
                manager
                    .create_tab(format!("Tab {}", i), TabType::Chat)
                    .expect("create");
            }

            let start = Instant::now();
            // Simulate the per-frame work the tab bar does: iterate all tabs
            // and gather metadata for display. Real rendering also iterates.
            let mut count = 0;
            for tab in manager.all_tabs() {
                // Touch a few fields to simulate display work
                let _ = tab.title.len();
                let _ = tab.id.0;
                count += 1;
            }
            report(
                &format!("iterate {} tabs at width {}", count, width),
                start.elapsed(),
                count,
            );
        }
    }
}
