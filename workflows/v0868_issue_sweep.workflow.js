export default workflow({
  "id": "v0868-issue-sweep",
  "goal": "Triage, investigate, and produce an actionable release plan for all CodeWhale v0.8.68 issues",
  "description": "Parallel issue sweep for v0.8.68. Phase 1 scouts each lane via gh CLI; Phase 2 deep-triages blockers; Phase 3 synthesizes tracker-ready release plan.",
  "nodes": [
    {
      "branch": {
        "id": "phase1-scout",
        "parallel": true,
        "children": [
          {
            "agent": {
              "id": "scout-milestone-board",
              "prompt": "Fetch live v0.8.68 board truth. Run:\n```\ngh issue view 4092 -R Hmbown/CodeWhale\ngh issue list -R Hmbown/CodeWhale --milestone v0.8.68 --state open --limit 200\ngh issue list -R Hmbown/CodeWhale --label release-blocker --state open\ngh pr list -R Hmbown/CodeWhale --state open --limit 50 --json number,title,isDraft,mergeable,milestone\n```\nRead `docs/AGENT_WORKFLOWS_0868.md` for wave structure. Summarize: open count, release blockers, PR candidates, recommended execution order from #4092.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["docs/AGENT_WORKFLOWS_0868.md"],
              "budget": { "max_steps": 10, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "scout-workflow-runtime",
              "prompt": "Audit workflow runtime for v0.8.68 (#4038, #4011, #4013, #4110). Read `crates/tui/src/tools/workflow.rs` for completion_from_manager, cancel/VM interrupt, budget.spent(), SharedWorkflowRuns usage. Read issues: `gh issue view 4038 4011 4013 4110 -R Hmbown/CodeWhale`. Report per item: done/partial/stub/missing with path:line, effort S/M/L, release-blocking yes/no.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/tools/workflow.rs", "crates/tui/src/tui/history.rs"],
              "budget": { "max_steps": 12, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "scout-catalog-lane",
              "prompt": "Audit model catalog lane (#4109, #4114-#4119, #4139-#4141). Read `crates/tui/src/client.rs`, `crates/config/src/catalog.rs`, `crates/tui/src/provider_lake.rs`, `crates/tui/src/tui/model_picker.rs`. Run `git grep -n model_completion_names_for_provider -- crates/`. Report Section 1 status with evidence.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/client.rs", "crates/config/src/catalog.rs", "crates/tui/src/provider_lake.rs"],
              "budget": { "max_steps": 12, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "scout-tui-stopship",
              "prompt": "Audit TUI stopship issues: #4090, #4093, #4094, #3986, #3990, #3985, #3987, #3993, #3995. Fetch via `gh issue view`. Search `crates/tui/src/tui/` for Ctrl+C handling, fleet setup modal, sub-agent sidebar, onboarding copy, slash autocomplete. Per issue: severity P0-P3, root cause hypothesis, fix complexity S/M/L.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/tui/"],
              "budget": { "max_steps": 14, "timeout_secs": 600 }
            }
          }
        ]
      }
    },
    {
      "sequence": {
        "id": "phase2-deep-triage",
        "children": [
          {
            "agent": {
              "id": "triage-stopship",
              "prompt": "Using scout-tui-stopship findings, produce ordered stopship fix train for #4090, #4093, #4094. Minimal fix approach per issue, test strategy, whether fixes ship independently. Read-only plan only.",
              "agent_type": "review",
              "mode": "read_only",
              "profile": "reviewer",
              "file_scope": ["crates/tui/src/tui/app.rs", "crates/tui/src/tui/sidebar.rs"],
              "budget": { "max_steps": 10, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "triage-defer-matrix",
              "prompt": "Using scout-milestone-board, classify ALL open v0.8.68 issues into: SHIP (0.8.68), DEFER (0.9.0), CLOSE (duplicate/wontfix), INVESTIGATE. Use gh-compile-issues disposition style with evidence. Flag v0.8.69-labeled issues still in milestone — default DEFER unless they unblock stopship (#4090, #4093, #4094). Defer v0.8.69 refactors and Waves 2–4 feature lanes until stopship is green. Implementation source of truth is main (not codex/0868-next). Output counts per bucket.",
              "agent_type": "review",
              "mode": "read_only",
              "profile": "reviewer",
              "budget": { "max_steps": 12, "timeout_secs": 900 }
            }
          }
        ]
      }
    },
    {
      "reduce": {
        "id": "release-plan",
        "inputs": [
          "scout-milestone-board",
          "scout-workflow-runtime",
          "scout-catalog-lane",
          "scout-tui-stopship",
          "triage-stopship",
          "triage-defer-matrix"
        ],
        "prompt": "Synthesize ALL upstream findings into a CodeWhale v0.8.68 release plan.\n\n## VERDICT\nready / partial / blocked — one sentence why\n\n## SCOPE (ship in 0.8.68)\n- Stopship / release-blockers\n- P1 features\n- Good-first issues\n\n## DEFER (0.9.0+)\n- Issue numbers + reason\n\n## CLOSE\n- Issue numbers + reason\n\n## WAVES (map to workflow files)\n| Wave | Workflow file | Issues | Owner lane |\n\n## PR HARVEST\n| PR | Issue | Action |\n\n## VERIFICATION GATE\n- Commands + manual checklist\n\n## OPEN RISKS\n\nBe decisive. Cite issue numbers. Use only upstream evidence."
      }
    }
  ]
});
