export default workflow({
  "id": "v0868-workflow-ui-lane",
  "goal": "Build v0.8.68 Workflow automatic orchestration UI (Section 2)",
  "description": "Scout workflow.rs and TUI activity surfaces, then implement typed events, WorkflowPanel, and automatic launch path. DEFERRED until stopship (#4090, #4093, #4094) is green — v0.8.69 refactors out of scope unless they unblock stopship.",
  "nodes": [
    {
      "branch": {
        "id": "scout-workflow",
        "parallel": true,
        "children": [
          {
            "agent": {
              "id": "scout-typed-events",
              "prompt": "Audit workflow progress typing for v0.8.68 (#4118, #4120-4125). Read `crates/tui/src/tools/workflow.rs` and `crates/tui/src/tui/history.rs`. Check if progress uses Vec<String> or typed WorkflowUiEvent. Read issues: `gh issue view 4118 4120 4121 4122 4123 4124 4125 -R Hmbown/CodeWhale`. Report done/partial/missing with path:line evidence.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/tools/workflow.rs", "crates/tui/src/tui/history.rs"],
              "budget": { "max_steps": 14, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "scout-workflow-panel",
              "prompt": "Audit WorkflowPanel / unified activity surface (#4121, #4038, #4110). Search for `workflow_panel`, `SharedWorkflowRuns`, activity panel above input. Read `gh issue view 4038 4110 4121 -R Hmbown/CodeWhale`. Report whether TUI reads workflow run state, panel file exists, history card renderer status.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/tui/", "crates/tui/src/tools/workflow.rs"],
              "budget": { "max_steps": 12, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "scout-auto-launch",
              "prompt": "Audit automatic Workflow launch path (#4127, #4128, #4129). Read engine prompts and workflow tool schema for auto-trigger, suppression rules, config keys. Issues: `gh issue view 4127 4128 4129 4130 -R Hmbown/CodeWhale`. Check `config.example.toml` for `[workflow]` section. Report gaps.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/core/engine/", "crates/tui/src/config.rs", "config.example.toml"],
              "budget": { "max_steps": 12, "timeout_secs": 600 }
            }
          }
        ]
      }
    },
    {
      "sequence": {
        "id": "implement-workflow-ui",
        "children": [
          {
            "agent": {
              "id": "impl-typed-events",
              "prompt": "Implement typed WorkflowUiEvent stream (#4118) per scout findings. Replace string progress where needed; add resolution fields (provider, model, route_source). Wire sub-agent spawn metadata (#4119). Default parallel write children to worktree (#4120). Tests: `cargo test -p codewhale-tui workflow`.",
              "agent_type": "implementer",
              "mode": "write",
              "file_scope": ["crates/tui/src/tools/workflow.rs", "crates/tui/src/tools/subagent/mod.rs"],
              "budget": { "max_steps": 28, "timeout_secs": 2400 }
            }
          },
          {
            "agent": {
              "id": "impl-workflow-panel",
              "prompt": "Implement WorkflowPanel unified activity surface (#4121, #4122, #4125) and history card routing (#4122). Create or extend `workflow_panel.rs`. Panel: collapsed/expanded, phase list, child rows, cancel. History card: phase summary, failure details. Tests for state transitions (#4123). `cargo test -p codewhale-tui workflow_panel history`.",
              "agent_type": "implementer",
              "mode": "write",
              "file_scope": ["crates/tui/src/tui/history.rs"],
              "budget": { "max_steps": 28, "timeout_secs": 2400 }
            }
          },
          {
            "agent": {
              "id": "impl-auto-launch",
              "prompt": "Implement automatic launch pieces (#4127-4129): planner-to-workflow structured launch (#4124), parent prompt update (#4125), approval card for elevated plans (#4126), trigger suppression (#4127), config defaults (#4128), sandbox regression tests (#4129). Prioritize config keys + suppression + prompt; defer full dogfood (#4131) if timeboxed.",
              "agent_type": "implementer",
              "mode": "write",
              "file_scope": ["crates/tui/src/tools/workflow.rs", "crates/workflow-js/src/vm.rs", "config.example.toml"],
              "budget": { "max_steps": 28, "timeout_secs": 2400 }
            }
          }
        ]
      }
    },
    {
      "reduce": {
        "id": "workflow-ui-handoff",
        "inputs": ["scout-typed-events", "scout-workflow-panel", "scout-auto-launch", "impl-typed-events", "impl-workflow-panel", "impl-auto-launch"],
        "prompt": "Synthesize Workflow UI lane.\n\n## SECTION 2 STATUS (2.1-2.14)\n| Phase | Items | Status |\n\n## RELEASE-BLOCKING GAPS\n\n## DOGFOOD SCENARIOS READY?\n\n## NEXT: Fleet/AgentProfile lane or TUI copy lane?"
      }
    }
  ]
});
