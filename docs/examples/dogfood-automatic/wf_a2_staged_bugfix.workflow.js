/**
 * #4131 WF-A2 — staged bug fix with worktree implementer + verifier.
 *
 * Expected UI: Implement phase with worktree-isolated implementer, then Verify
 * phase. The verifier checks the returned handoff while confirming the parent
 * workspace remains unchanged until an explicit apply/merge. Write/worktree
 * plans should surface approval when require_approval_for_writes is true.
 *
 * Run: /workflow run docs/examples/dogfood-automatic/wf_a2_staged_bugfix.workflow.js
 */
export default async function (args) {
  const target = args?.target ?? "docs/AUTOMATIC_WORKFLOWS.md";
  const change =
    args?.change ??
    "Add a one-line note that #4131 example scenarios live in docs/examples/dogfood-automatic/.";

  phase("Implement");
  const implement = await task({
    description: `Implement minimal docs fix in an isolated worktree: ${target}`,
    label: "implementer",
    type: "implementer",
    // Prefer worktree isolation for write children (product default #4120).
    worktree: true,
    writeAuthority: "worktree_write",
    exactFiles: [target],
    coordinationContracts: ["wf-a2-doc-fix"],
    dependencies: ["The parent workspace must remain unchanged."],
    acceptance: [
      `Only ${target} changes in the isolated worktree.`,
      "The handoff reports the exact path and diff summary.",
    ],
    prompt: [
      `Edit only ${target}.`,
      change,
      "Keep the change minimal and reversible. Do not push. Do not touch unrelated files.",
      "Return: path edited, unified-diff summary, worktree path if any.",
    ].join("\n"),
  });

  phase("Verify");
  const verify = await task({
    description: "Verify the isolated implementer handoff without further edits.",
    label: "verifier",
    type: "verifier",
    worktree: false,
    prompt: [
      "Read the implementer result and validate its reported path and diff summary.",
      "Confirm the intended one-line clarification was made only in the isolated worktree.",
      "Confirm the parent workspace remains unchanged until an explicit apply or merge.",
      "Do not implement further edits. Return PASS/FAIL with evidence.",
      "",
      "implementer_result:",
      String(implement ?? "(missing)"),
    ].join("\n"),
  });

  return {
    scenario: "WF-A2",
    target,
    implement,
    verify,
  };
}
