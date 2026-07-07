export default workflow({
  "id": "v0868-tui-copy-lane",
  "goal": "Polish v0.8.68 default transcript copy and progressive disclosure (Section 5)",
  "description": "Parallel audit of copy-slop findings, then batch implementation of P1/P2 dedupe items. DEFERRED until stopship (#4090, #4093, #4094) is green.",
  "nodes": [
    {
      "branch": {
        "id": "scout-copy",
        "parallel": true,
        "children": [
          {
            "agent": {
              "id": "scout-context-disclosure",
              "prompt": "Audit context percent disclosure (#4142, copy finding #11). Read `header.rs`, `footer_ui.rs`, `sidebar.rs` for context % rendering. Issue: `gh issue view 4142 -R Hmbown/CodeWhale`. Report how many surfaces show context % simultaneously and recommended single disclosure point.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/tui/widgets/header.rs", "crates/tui/src/tui/footer_ui.rs", "crates/tui/src/tui/sidebar.rs"],
              "budget": { "max_steps": 10, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "scout-transcript-words",
              "prompt": "Audit default transcript copy issues (#4112, #4143-#4148). Read issues via `gh issue view 4112 4143 4144 4145 4146 4147 4148 -R Hmbown/CodeWhale`. Search `en.json` and history renderers for: mode picker body copy, setup hints, Searching verb mismatch, reasoning quiet default, sidebar Tasks label, duplicate/leaky words. Report per-issue file targets.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/locales/en.json", "crates/tui/src/tui/history.rs", "crates/tui/src/tui/views/mode_picker.rs"],
              "budget": { "max_steps": 12, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "scout-compact-mode",
              "prompt": "Audit compact mode default (#4095). `gh issue view 4095 -R Hmbown/CodeWhale`. Find compact mode config, default TUI presentation settings, busy chrome sources. Report whether compact should become default and what changes are needed in config/TuiPrefs.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/config.rs", "crates/tui/src/tui/ui.rs"],
              "budget": { "max_steps": 10, "timeout_secs": 600 }
            }
          }
        ]
      }
    },
    {
      "sequence": {
        "id": "implement-copy",
        "children": [
          {
            "agent": {
              "id": "impl-copy-dedupe",
              "prompt": "Implement copy dedupe batch (#4142-#4148, #4112) per scout findings. Rules: disclose once not thrice; header OR footer OR sidebar owns each fact. Touch `en.json`, `mode_picker.rs`, `history.rs`, `sidebar.rs` as needed. Add/adjust tests. `cargo test -p codewhale-tui history mode_picker`.",
              "agent_type": "implementer",
              "mode": "write",
              "file_scope": ["crates/tui/locales/en.json", "crates/tui/src/tui/history.rs"],
              "budget": { "max_steps": 20, "timeout_secs": 1200 }
            }
          },
          {
            "agent": {
              "id": "impl-compact-default",
              "prompt": "If scout-compact-mode recommends it, make compact presentation the default (#4095) with safe migration for existing users. Minimal config/default change. Document in CHANGELOG snippet. `cargo test -p codewhale-tui config`.",
              "agent_type": "implementer",
              "mode": "write",
              "file_scope": ["crates/tui/src/config.rs"],
              "budget": { "max_steps": 12, "timeout_secs": 900 }
            }
          }
        ]
      }
    },
    {
      "reduce": {
        "id": "copy-handoff",
        "inputs": ["scout-context-disclosure", "scout-transcript-words", "scout-compact-mode", "impl-copy-dedupe", "impl-compact-default"],
        "prompt": "Synthesize TUI copy lane.\n\n## SECTION 5 STATUS\n| Issue | Status | Files |\n\n## COPY SLOP REMAINING\n\n## UX VERDICT\nready for release polish / needs more work"
      }
    }
  ]
});
