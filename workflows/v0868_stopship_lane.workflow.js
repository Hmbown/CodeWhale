export default workflow({
  "id": "v0868-stopship-lane",
  "goal": "Fix v0.8.68 release-stopship blockers before any feature work",
  "description": "Sequential stopship lane: #4090 Ctrl+C regression, release-blockers #4093/#4094, then re-verify dogfood fixes #3986/#3990. Branch all implementation from main (not codex/0868-next).",
  "nodes": [
    {
      "branch": {
        "id": "scout-stopship",
        "parallel": true,
        "children": [
          {
            "agent": {
              "id": "scout-ctrl-c",
              "prompt": "Investigate GitHub issue #4090 (repeated Ctrl+C re-prompts in PTY/raw-mode). Run: `gh issue view 4090 -R Hmbown/CodeWhale`. Then search `crates/tui/src/` for Ctrl+C handling, raw mode teardown, and exit confirmation paths. Report: repro hypothesis, exact files/functions, whether a regression test or PTY trace exists, minimal fix approach. Read-only.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/tui/app.rs", "crates/tui/src/tui/ui.rs", "crates/tui/src/main.rs"],
              "budget": { "max_steps": 12, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "scout-fleet-modal",
              "prompt": "Investigate release-blocker #4093 (Fleet setup modal provider-scoped instead of role/profile roster). Run: `gh issue view 4093 -R Hmbown/CodeWhale`. Inspect `crates/tui/src/tui/views/fleet_setup.rs`, `crates/tui/src/fleet/roster.rs`, and related Fleet UI. Report current vs expected behavior, root cause, files to change, test strategy. Read-only.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/tui/views/fleet_setup.rs", "crates/tui/src/fleet/"],
              "budget": { "max_steps": 12, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "scout-subagent-panel",
              "prompt": "Investigate release-blocker #4094 (sub-agent detail panel empty / TUI freeze). Run: `gh issue view 4094 -R Hmbown/CodeWhale`. Inspect sidebar agents panel, sub-agent detail rendering, and redraw paths under load. Report root cause hypothesis, files, whether throttle or empty-state bug, severity. Read-only.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/tui/sidebar.rs", "crates/tui/src/tui/widgets/agent_card.rs"],
              "budget": { "max_steps": 12, "timeout_secs": 600 }
            }
          }
        ]
      }
    },
    {
      "sequence": {
        "id": "implement-stopship",
        "children": [
          {
            "agent": {
              "id": "fix-4090",
              "prompt": "Fix #4090 using scout-ctrl-c findings. Branch from main (git checkout main && git pull && git checkout -b codex/v0868-fix-4090). Implement minimal fix so double Ctrl+C exits cleanly in PTY/raw-mode while preserving cancel/copy behavior. Add regression test or PTY/key-path trace. Run: `cargo test -p codewhale-tui` for touched modules and `cargo clippy -p codewhale-tui -- -D warnings`. Do not close the issue; report files changed and test output.",
              "agent_type": "implementer",
              "mode": "write",
              "file_scope": ["crates/tui/src/tui/app.rs", "crates/tui/src/tui/ui.rs"],
              "budget": { "max_steps": 20, "timeout_secs": 1200 }
            }
          },
          {
            "agent": {
              "id": "fix-4093-4094",
              "prompt": "Using scout findings, fix #4093 and/or #4094 if root causes are clear and independent. Branch from main (separate branches per issue if needed: codex/v0868-fix-4093, codex/v0868-fix-4094). Prefer smallest correct diffs. For #4093: Fleet setup should edit role/profile roster not provider scope. For #4094: sub-agent detail panel must render and not freeze TUI. Add tests where feasible. Run targeted `cargo test -p codewhale-tui fleet sidebar`. Report per-issue status: fixed/partial/blocked.",
              "agent_type": "implementer",
              "mode": "write",
              "file_scope": ["crates/tui/src/tui/views/fleet_setup.rs", "crates/tui/src/tui/sidebar.rs"],
              "budget": { "max_steps": 24, "timeout_secs": 1800 }
            }
          },
          {
            "agent": {
              "id": "verify-dogfood",
              "prompt": "Re-verify dogfood fixes for #3986 (API-key onboarding copy shows CODEWHALE_HOME path) and #3990 (slash autocomplete alias duplication). Read issue bodies via `gh issue view`. Check if fixes exist on current branch; if missing, implement minimal fixes. Run verification gate subset: `cargo fmt --all --check`, `cargo test -p codewhale-tui -- onboarding slash`. Report done/partial/missing per issue.",
              "agent_type": "verifier",
              "mode": "write",
              "file_scope": ["crates/tui/src/tui/", "crates/tui/locales/en.json"],
              "budget": { "max_steps": 16, "timeout_secs": 900 }
            }
          }
        ]
      }
    },
    {
      "reduce": {
        "id": "stopship-handoff",
        "inputs": ["scout-ctrl-c", "scout-fleet-modal", "scout-subagent-panel", "fix-4090", "fix-4093-4094", "verify-dogfood"],
        "prompt": "Synthesize stopship lane results.\n\n## VERDICT\nstopship-green / partial / blocked\n\n## PER-ISSUE STATUS\n| Issue | Status | Evidence |\n\n## TESTS RUN\n\n## REMAINING BLOCKERS\n\n## NEXT WAVE\nRecommend whether catalog lane (v0868_catalog_lane) is safe to start.\n\nCite only upstream agent evidence. Be decisive."
      }
    }
  ]
});
