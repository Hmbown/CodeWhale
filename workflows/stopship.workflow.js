// Live acceptance on DeepSeek Flash and GLM-5-Turbo measured the inherited
// read-only prompt/tool envelope at 17,457 and 17,550 tokens before the first
// useful tool turn completed. Budget 24k per intended model turn, then pair
// that token allowance with a small max_steps bound so every role has usable
// headroom without becoming open-ended. The five role caps total 360k.
export default workflow({
  "id": "stopship-release-acceptance",
  "goal": "Verify the current Codewhale Fleet, Workflow, Lane, Runtime, and gate receipt path without changing the workspace",
  "description": "Version-neutral, read-only release acceptance fixture. Every Fleet role inspects checked-in runtime evidence; no step creates branches, edits files, installs dependencies, or publishes anything.",
  "gates": [
    {
      "id": "scout-evidence",
      "role": "scout",
      "on": "role_complete",
      "gate": "approve",
      "on_fail": "block",
      "blocks_role": "implementer",
      "max_retries": 0,
      "artifact_kind": "source_evidence",
      "require_explicit_verdict": true
    },
    {
      "id": "implementation-plan",
      "role": "implementer",
      "on": "role_complete",
      "gate": "approve",
      "on_fail": "block",
      "blocks_role": "reviewer",
      "max_retries": 0,
      "artifact_kind": "verification_plan",
      "require_explicit_verdict": true
    },
    {
      "id": "review-findings",
      "role": "reviewer",
      "on": "role_complete",
      "gate": "review",
      "on_fail": "block",
      "blocks_role": "verifier",
      "max_retries": 0,
      "artifact_kind": "review_report",
      "require_explicit_verdict": true
    },
    {
      "id": "verifier-evidence",
      "role": "verifier",
      "on": "role_complete",
      "gate": "verify",
      "on_fail": "block",
      "blocks_role": "release_lead",
      "max_retries": 0,
      "artifact_kind": "verification_report",
      "require_explicit_verdict": true
    }
  ],
  "nodes": [
    {
      "sequence": {
        "id": "acceptance-chain",
        "children": [
          {
            "agent": {
              "id": "scout-runtime",
              "prompt": "Read the checked-in release orchestration path only. Use `grep_files` first for the named symbols, then use `read_file` only on bounded relevant snippets; never read an entire large source file. Inspect workflows/stopship.workflow.js, fleets/stopship.toml, crates/cli/src/lib.rs, crates/workflow/src/role_resolve.rs, and crates/tui/src/tools/workflow.rs. The first non-empty line of your response must be exactly APPROVE or exactly BLOCK. Use APPROVE only when every named source owner is found; otherwise use BLOCK. Follow with concise path-and-symbol evidence for: the stopship alias, named Fleet loading, role-to-profile resolution, tmux Lane launch, typed task_started receipts, gate_updated receipts, and terminal workflow receipts. Do not edit files, create branches, run shell commands, access GitHub, or infer success where source evidence is absent.",
              "agent_type": "explore",
              "role": "scout",
              "mode": "read_only",
              "file_scope": [
                "workflows/stopship.workflow.js",
                "fleets/stopship.toml",
                "crates/cli/src/lib.rs",
                "crates/workflow/src/role_resolve.rs",
                "crates/tui/src/tools/workflow.rs"
              ],
              "budget": { "max_steps": 4, "timeout_secs": 480, "max_tokens": 96000 }
            }
          },
          {
            "agent": {
              "id": "plan-verification",
              "prompt": "Act as the Fleet implementer role for a verification-only acceptance run. Use the promoted scout source_evidence handoff to produce a no-edit verification plan for the Fleet/Workflow/Lane/Runtime contract. If source confirmation is needed, use `grep_files` first and `read_file` only on bounded relevant snippets; never read an entire large source file. The first non-empty line of your response must be exactly APPROVE or exactly BLOCK. Use APPROVE only when the plan names concrete receipt fields for role resolution, gate promotion or blocking, and terminal Lane reconciliation; otherwise use BLOCK. This is deliberately not an implementation task: do not edit files, create branches, run shell commands, or propose fixes unrelated to missing acceptance evidence.",
              "agent_type": "implementer",
              "role": "implementer",
              "mode": "read_only",
              "file_scope": [
                "crates/cli/src/lib.rs",
                "crates/lane/src/registry.rs",
                "crates/tui/src/tools/workflow.rs"
              ],
              "budget": { "max_steps": 3, "timeout_secs": 420, "max_tokens": 72000 }
            }
          },
          {
            "agent": {
              "id": "review-contract",
              "prompt": "Review the promoted verification_plan handoff against the checked-in runtime. Use `grep_files` first for each claimed owner and `read_file` only on bounded relevant snippets; never read an entire large source file. Look specifically for false-green risks: declared role versus resolved profile, gate state versus prose verdict, tmux process exit versus terminal workflow receipt, and a completed Lane with missing child evidence. The first non-empty line of your response must be exactly APPROVE or exactly BLOCK. Use APPROVE only when each claimed receipt has a concrete source owner; otherwise use BLOCK and list the missing evidence. Remain read-only and do not run shell commands or edit anything.",
              "agent_type": "review",
              "role": "reviewer",
              "mode": "read_only",
              "file_scope": [
                "crates/cli/src/lib.rs",
                "crates/lane/src/registry.rs",
                "crates/tui/src/tools/workflow.rs"
              ],
              "budget": { "max_steps": 3, "timeout_secs": 420, "max_tokens": 72000 }
            }
          },
          {
            "agent": {
              "id": "verify-receipts",
              "prompt": "Statically verify the promoted review_report against existing tests and receipt serialization. Use `grep_files` first for the receipt and test symbols, then use `read_file` only on bounded relevant snippets; never read an entire large source file. Inspect the Workflow and CLI test modules for role-resolved task_started, gate_updated, run_completed, metadata, and Lane exit-receipt assertions. The first non-empty line of your response must be exactly APPROVE or exactly BLOCK. Use APPROVE only when every required receipt has an exact test name or source symbol; otherwise use BLOCK. Follow with a compact evidence matrix. Do not run commands, edit files, or create build artifacts; the host gate interprets the explicit first-line verdict.",
              "agent_type": "verifier",
              "role": "verifier",
              "mode": "read_only",
              "file_scope": [
                "crates/cli/src/lib.rs",
                "crates/lane/src/registry.rs",
                "crates/tui/src/tools/workflow.rs"
              ],
              "budget": { "max_steps": 3, "timeout_secs": 420, "max_tokens": 72000 }
            }
          },
          {
            "agent": {
              "id": "release-receipt",
              "prompt": "Use the promoted verification_report handoff to produce the final acceptance receipt for the Fleet/Workflow/Lane/Runtime contract. If source confirmation is needed, use `grep_files` first and `read_file` only on bounded relevant snippets; never read an entire large source file. The first non-empty line of your response must be exactly APPROVE or exactly BLOCK. Use APPROVE only when the receipt includes declared Fleet role and resolved profile evidence, every observed gate state, and the required terminal workflow status; otherwise use BLOCK and name the closure blocker. Never claim that source inspection substitutes for a live Lane log. Do not edit, publish, close issues, run shell commands, or mutate the workspace.",
              "agent_type": "general",
              "role": "release_lead",
              "mode": "read_only",
              "file_scope": [
                "crates/cli/src/lib.rs",
                "crates/lane/src/registry.rs",
                "crates/tui/src/tools/workflow.rs"
              ],
              "budget": { "max_steps": 2, "timeout_secs": 300, "max_tokens": 48000 }
            }
          }
        ]
      }
    }
  ]
});

