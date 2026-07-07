export default workflow({
  "id": "v0868-release-gate",
  "goal": "Run final v0.8.68 verification gate and produce release handoff",
  "description": "Parallel verification agents (build, test, milestone audit) then synthesize release readiness verdict.",
  "nodes": [
    {
      "branch": {
        "id": "verify-parallel",
        "parallel": true,
        "children": [
          {
            "agent": {
              "id": "verify-build",
              "prompt": "Run the v0.8.68 verification gate build steps:\n```\ncargo fmt --all --check\ncargo clippy --workspace --all-features --locked -D warnings -A clippy::uninlined_format_args -A clippy::too_many_arguments -A clippy::unnecessary_map_or -A clippy::collapsible_if -A clippy::assertions_on_constants\ncargo build --release -p codewhale-tui -p codewhale-cli --locked\n```\nReport pass/fail per command with error excerpts if any fail.",
              "agent_type": "verifier",
              "mode": "read_only",
              "budget": { "max_steps": 8, "timeout_secs": 1800 }
            }
          },
          {
            "agent": {
              "id": "verify-tests",
              "prompt": "Run workspace tests and focused TUI suites:\n```\ncargo test --workspace --locked\ncargo test -p codewhale-tui -- workflow fleet model_picker subagent\n```\nReport pass/fail counts and any failing test names with likely cause.",
              "agent_type": "verifier",
              "mode": "read_only",
              "budget": { "max_steps": 8, "timeout_secs": 2400 }
            }
          },
          {
            "agent": {
              "id": "audit-milestone",
              "prompt": "Audit v0.8.68 milestone readiness using gh CLI:\n```\ngh issue list -R Hmbown/CodeWhale --milestone v0.8.68 --state open --limit 200\ngh issue list -R Hmbown/CodeWhale --label release-blocker --state open\ngh pr list -R Hmbown/CodeWhale --state open --json number,title,isDraft,mergeable,milestone\n```\nClassify open issues: stopship / should-ship / defer-0.9.0. Count by theme (catalog, workflow, tui, refactor). Read #4092 for prior handoff context.",
              "agent_type": "explore",
              "mode": "read_only",
              "budget": { "max_steps": 10, "timeout_secs": 600 }
            }
          }
        ]
      }
    },
    {
      "reduce": {
        "id": "release-verdict",
        "inputs": ["verify-build", "verify-tests", "audit-milestone"],
        "prompt": "Produce v0.8.68 release gate report.\n\n## VERDICT\nready-to-tag / partial / blocked\n\n## GATE RESULTS\n| Check | Status |\n\n## OPEN STOPSHIP\n\n## SHIP LIST (issues that can close on tag)\n\n## DEFER LIST (move to 0.9.0)\n\n## HANDOFF FOR NEXT AGENT\nBranch, HEAD, PRs to review, recommended next wave.\n\nUpdate guidance for issue #4092 comment draft (do not post without approval)."
      }
    }
  ]
});
