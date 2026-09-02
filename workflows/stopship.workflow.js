// The read-only child prompt/tool envelope is regression-pinned in
// crates/tui/src/tools/subagent/tests.rs
// (read_only_child_envelope_stays_within_measured_ceiling): 80,856 bytes
// (~20k tokens) measured 2026-08-02 on this tree, ceiling +10%. Historical
// live acceptance on DeepSeek Flash / GLM-5-Turbo measured 17,457–17,550
// tokens. Budget 24k per intended evidence turn, then add token-neutral
// max_steps headroom for the required final verdict. The token ceiling
// remains independent and the five role caps still total 360k.
export default workflow({
  "id": "stopship-release-acceptance",
  "goal": "Verify the current Codewhale Fleet, Workflow, Lane, Runtime, and gate receipt path without changing the workspace",
  "description": "Version-neutral, read-only release acceptance fixture. Every Fleet role inspects checked-in runtime evidence; no step creates branches, edits files, installs dependencies, or publishes anything.",
  "gates": [
    {
      "id": "explore-evidence",
      "role": "explore",
      "on": "role_complete",
      "gate": "approve",
      "on_fail": "block",
      "blocks_role": "implement",
      "max_retries": 0,
      "artifact_kind": "source_evidence",
      "require_explicit_verdict": true
    },
    {
      "id": "implement-plan",
      "role": "implement",
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
      "blocks_role": "test",
      "max_retries": 0,
      "artifact_kind": "review_report",
      "require_explicit_verdict": true
    },
    {
      "id": "test-evidence",
      "role": "test",
      "on": "role_complete",
      "gate": "verify",
      "on_fail": "block",
      "blocks_role": "release_lead",
      "max_retries": 0,
      "artifact_kind": "verification_report",
      "require_explicit_verdict": true
    },
    {
      "id": "release-receipt",
      "role": "release_lead",
      "on": "role_complete",
      "gate": "approve",
      "on_fail": "block",
      "max_retries": 0,
      "artifact_kind": "final_receipt",
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
              "id": "explore-runtime",
              "prompt": "Verify the runtime release-orchestration owners using only the five files in File scope. The host's typed run_started receipt already owns the compiled Workflow id and source path; do not re-verify the Workflow alias. You have at most six model responses and must reserve the verdict. Response 1 must make exactly one `File` call with `action` set to `search_content`, `path` set to `.`, and `include` set exactly to [`fleets/stopship.toml`, `crates/cli/src/lib.rs`, `crates/workflow/src/role_resolve.rs`, `crates/tui/src/tools/workflow.rs`, `crates/lane/src/runtime.rs`], using this high-signal alternation pattern: `name = \"stopship\"|load_named_fleet|start_lane|resolve_workflow_agent|record_task_started|WorkflowUiEventKind::GateUpdated|WorkflowUiEventKind::RunCompleted|terminal_completed_receipt|process_exit_receipt|lane_reconciled|tmux_reconcile_folds_detached_process_exit_into_lane_status|stopship_acceptance_fixture_emits_role_gate_and_terminal_receipts`. Set at most 80 results and 2 context lines. Matches outside that exact include list do not count. Do not add generic field names such as `resolved_profile` or `exit_code` to the pattern. Do not call `File` more than once and do not use any action except `search_content` or call any other tool. Response 2 must return the verdict with no tool calls; any later reserved response must do the same instead of gathering more evidence. Treat an exact match naming a call site, typed event constructor, reconciliation branch, or test assertion in a scoped file as source-owner evidence. Apply this decision rule literally: if you can populate all seven required SOURCE EVIDENCE entries from the search result, return APPROVE; never return BLOCK after citing all seven. Return BLOCK only when at least one named owner has no matching citation, and identify each missing owner as MISSING. The first non-empty line of your response must be exactly APPROVE or exactly BLOCK. Do not put any words before that verdict: no confirmation, summary, heading, or phrase such as `Here is the verdict`. After the verdict, include a `SOURCE EVIDENCE` section with concise `path: symbol` evidence for named Fleet loading, role-to-profile resolution, tmux Lane launch, typed task_started, gate_updated, terminal run_completed, and tmux process-exit reconciliation receipts. The terminal run_completed entry must carry both the `WorkflowUiEventKind::RunCompleted` constructor and the `terminal_completed_receipt` integration assertion. A bare verdict is invalid. Do not edit files, create branches, run shell commands, access GitHub, or infer success where source evidence is absent.",
              "agent_type": "explore",
              "role": "explore",
              "mode": "read_only",
              "file_scope": [
                "fleets/stopship.toml",
                "crates/cli/src/lib.rs",
                "crates/workflow/src/role_resolve.rs",
                "crates/tui/src/tools/workflow.rs",
                "crates/lane/src/runtime.rs"
              ],
              "budget": { "max_steps": 6, "timeout_secs": 480, "max_tokens": 96000 }
            }
          },
          {
            "agent": {
              "id": "plan-verification",
              "prompt": "Act as the Fleet implement role for a verification-only acceptance run. Use only the promoted explore source_evidence handoff to produce a no-edit verification plan for the Fleet/Workflow/Lane/Runtime contract. Tools are intentionally unavailable because the promoted handoff is the evidence boundary; do not request source reads or gather new evidence. You have at most four model responses, but the first response must return APPROVE or BLOCK with no tool calls. After that verdict, include a compact `PLAN` section that copies exact `path: symbol` citations for all seven owners from source_evidence: named Fleet loading, role-to-profile resolution, tmux Lane launch, typed task_started, gate_updated, terminal run_completed, and tmux process-exit reconciliation. Use one concise row per owner and no explanatory prose so the seven-owner matrix survives handoff compaction. The Workflow row must carry both the `WorkflowUiEventKind::RunCompleted` constructor and the `terminal_completed_receipt` integration assertion. The Lane-exit row must carry a `crates/lane/src/runtime.rs` citation for `process_exit_receipt`, `lane_reconciled`, or its reconciliation test; `start_lane` alone is not terminal evidence. A bare verdict is invalid. The first non-empty line of your response must be exactly APPROVE or exactly BLOCK. Do not put any words before that verdict: no confirmation, summary, heading, or phrase such as `Here is the verdict`. Use APPROVE only when that complete concrete plan can be produced from the handoff; otherwise use BLOCK. This is deliberately not an implementation task: do not edit files, create branches, run shell commands, or propose fixes unrelated to missing acceptance evidence.",
              "agent_type": "implement",
              "role": "implement",
              "mode": "read_only",
              "permissions": { "deny_all_tools": true },
              "file_scope": [
                "fleets/stopship.toml",
                "crates/cli/src/lib.rs",
                "crates/workflow/src/role_resolve.rs",
                "crates/tui/src/tools/workflow.rs",
                "crates/lane/src/runtime.rs"
              ],
              "budget": { "max_steps": 4, "timeout_secs": 420, "max_tokens": 72000 }
            }
          },
          {
            "agent": {
              "id": "review-contract",
              "prompt": "Review only the promoted verification_plan handoff against the source_evidence citations it carries. Tools are intentionally unavailable because promoted handoffs are the evidence boundary; do not request source reads or gather new evidence. You have at most four model responses, but the first response must return APPROVE or BLOCK with no tool calls. After that verdict, include a compact `EVIDENCE REVIEW` section that carries forward exact `path: symbol` citations for all seven owners: named Fleet loading, role-to-profile resolution, tmux Lane launch, typed task_started, gate_updated, terminal run_completed, and tmux process-exit reconciliation. Use one concise row per owner and no explanatory prose so the seven-owner matrix survives handoff compaction. Accept Workflow terminal evidence only when the plan carries both the `WorkflowUiEventKind::RunCompleted` constructor and the `terminal_completed_receipt` integration assertion. Accept tmux terminal evidence only when the plan carries a `crates/lane/src/runtime.rs` process-exit reconciliation or reconciliation-test citation alongside that Workflow evidence. A bare verdict is invalid. The first non-empty line of your response must be exactly APPROVE or exactly BLOCK. Do not put any words before that verdict: no confirmation, summary, heading, or phrase such as `Here is the verdict`. Use APPROVE only when every owner has a concrete source citation in the promoted evidence; otherwise use BLOCK and list the missing evidence. Remain read-only and do not run shell commands or edit anything.",
              "agent_type": "reviewer",
              "role": "reviewer",
              "mode": "read_only",
              "permissions": { "deny_all_tools": true },
              "file_scope": [
                "fleets/stopship.toml",
                "crates/cli/src/lib.rs",
                "crates/workflow/src/role_resolve.rs",
                "crates/tui/src/tools/workflow.rs",
                "crates/lane/src/runtime.rs"
              ],
              "budget": { "max_steps": 4, "timeout_secs": 420, "max_tokens": 72000 }
            }
          },
          {
            "agent": {
              "id": "verify-receipts",
              "prompt": "Statically verify only the promoted review_report and the source-evidence citations it carries against the required receipt contract. Tools are intentionally unavailable because promoted handoffs are the evidence boundary; do not request source reads or gather new evidence. You have at most four model responses, but the first response must return APPROVE or BLOCK with no tool calls. After that verdict, include a compact `EVIDENCE MATRIX` that carries forward exact test names or `path: symbol` citations for all seven owners: named Fleet loading, role-to-profile resolution, tmux Lane launch, typed task_started, gate_updated, terminal run_completed, and tmux process-exit reconciliation. Use one concise row per owner and no explanatory prose so the seven-owner matrix survives handoff compaction. The Workflow row must carry both the `WorkflowUiEventKind::RunCompleted` constructor and the `terminal_completed_receipt` integration assertion. The Lane-exit row must cite `crates/lane/src/runtime.rs` process-exit reconciliation rather than only `start_lane`. A bare verdict is invalid. The first non-empty line of your response must be exactly APPROVE or exactly BLOCK. Do not put any words before that verdict: no confirmation, summary, heading, or phrase such as `Here is the verdict`. Use APPROVE only when every required owner is covered; otherwise use BLOCK. Do not run commands, edit files, or create build artifacts; the host gate interprets the explicit first-line verdict.",
              "agent_type": "test",
              "role": "test",
              "mode": "read_only",
              "permissions": { "deny_all_tools": true },
              "file_scope": [
                "fleets/stopship.toml",
                "crates/cli/src/lib.rs",
                "crates/workflow/src/role_resolve.rs",
                "crates/tui/src/tools/workflow.rs",
                "crates/lane/src/runtime.rs"
              ],
              "budget": { "max_steps": 4, "timeout_secs": 420, "max_tokens": 72000 }
            }
          },
          {
            "agent": {
              "id": "release-receipt",
              "prompt": "Use only the promoted verification_report handoff to produce the final acceptance receipt for the Fleet/Workflow/Lane/Runtime contract. Tools are intentionally unavailable because promoted handoffs are the evidence boundary; do not request source reads or gather new evidence. You have at most three model responses, but the first response must return APPROVE or BLOCK with no tool calls. After that verdict, include a compact `FINAL RECEIPT` section that carries forward exact `path: symbol` citations for all seven owners: named Fleet loading, role-to-profile resolution, tmux Lane launch, typed task_started, gate_updated, terminal run_completed, and tmux process-exit reconciliation. Use one concise row per owner and no explanatory prose. The Workflow row must carry both the `WorkflowUiEventKind::RunCompleted` constructor and the `terminal_completed_receipt` integration assertion. A bare verdict is invalid. The first non-empty line of your response must be exactly APPROVE or exactly BLOCK. Do not put any words before that verdict: no confirmation, summary, heading, or phrase such as `Here is the verdict`. Use APPROVE only when that complete static receipt can be produced from the handoff; otherwise use BLOCK and name the closure blocker. Never claim that source inspection substitutes for the host's live Workflow or Lane terminal events; the host validates the actual run_completed and process-exit reconciliation receipts after this role completes. Do not edit, publish, close issues, run shell commands, or mutate the workspace.",
              "agent_type": "general",
              "role": "release_lead",
              "mode": "read_only",
              "permissions": { "deny_all_tools": true },
              "file_scope": [
                "fleets/stopship.toml",
                "crates/cli/src/lib.rs",
                "crates/workflow/src/role_resolve.rs",
                "crates/tui/src/tools/workflow.rs",
                "crates/lane/src/runtime.rs"
              ],
              "budget": { "max_steps": 3, "timeout_secs": 300, "max_tokens": 48000 }
            }
          }
        ]
      }
    }
  ]
});
