export default workflow({
  "id": "v0868-issue-implement",
  "goal": "Implement a single v0.8.68 GitHub issue end-to-end (set ISSUE_NUMBER in prompt)",
  "description": "Generic per-issue workflow: fetch issue, scout code, implement fix, verify, prepare PR handoff. Branch from main; do not use codex/0868-next unless cherry-picking a specific commit.",
  "nodes": [
    {
      "agent": {
        "id": "fetch-issue",
        "prompt": "The target issue number is provided in the workflow goal or user message. Run:\n```\ngh issue view <ISSUE_NUMBER> -R Hmbown/CodeWhale --json number,title,body,labels,milestone,comments\n```\nExtract: goal, acceptance criteria, key files mentioned, verification commands, out-of-scope items. Confirm issue is in milestone v0.8.68 or has label v0.8.68. Output structured ISSUE_BRIEF with SCOPE, KEY_FILES, ACCEPTANCE, VERIFICATION, OUT_OF_SCOPE.",
        "agent_type": "explore",
        "mode": "read_only",
        "budget": { "max_steps": 8, "timeout_secs": 300 }
      }
    },
    {
      "agent": {
        "id": "scout-code",
        "prompt": "Using ISSUE_BRIEF from fetch-issue, inspect current code at cited paths. Use `git grep` and read files. Report: already-done / partial / not-started with path:line evidence. Propose minimal implementation plan (numbered steps). Read-only.",
        "agent_type": "explore",
        "mode": "read_only",
        "file_scope": ["crates/"],
        "budget": { "max_steps": 12, "timeout_secs": 600 }
      }
    },
    {
      "agent": {
        "id": "implement-fix",
        "prompt": "Implement the issue per scout-code plan. Branch from main (`git checkout main && git pull && git checkout -b codex/v0868-fix-<N>`). Stay within OUT_OF_SCOPE. Smallest correct diff. Run VERIFICATION commands from ISSUE_BRIEF. If blocked, report blocker clearly instead of guessing.",
        "agent_type": "implementer",
        "mode": "write",
        "file_scope": ["crates/"],
        "budget": { "max_steps": 24, "timeout_secs": 1800 }
      }
    },
    {
      "agent": {
        "id": "verify-fix",
        "prompt": "Re-run all verification commands from ISSUE_BRIEF. Map each acceptance criterion to pass/fail with evidence. Prepare PR title and body with `Fixes #<N>` footer. Do not push or open PR without approval.",
        "agent_type": "verifier",
        "mode": "read_only",
        "budget": { "max_steps": 8, "timeout_secs": 900 }
      }
    },
    {
      "reduce": {
        "id": "issue-handoff",
        "inputs": ["fetch-issue", "scout-code", "implement-fix", "verify-fix"],
        "prompt": "Synthesize per-issue implementation handoff.\n\n## ISSUE\n\n## STATUS\nimplemented / partial / blocked\n\n## CHANGES\n\n## TESTS\n\n## PR DRAFT\nTitle + body\n\n## BLOCKERS"
      }
    }
  ]
});
