/**
 * #4131 WF-A1 — read-only repo audit (dogfood fixture).
 *
 * Expected: scout phase with read-only children, then a synthesizer that
 * produces an operator-facing summary. No write tools required.
 *
 * Run: /workflow run docs/examples/dogfood-automatic/wf_a1_read_only_audit.workflow.js
 *      (or: codewhale workflow run --source-path docs/examples/dogfood-automatic/wf_a1_read_only_audit.workflow.js --runtime inline)
 */
export default async function (args) {
  phase("Scout");
  const [crates, unsafeHits, unwrapHits] = await parallel([
    () =>
      task({
        description:
          "List top-level crates and one-line role for each under crates/.",
        label: "map crates",
        type: "explore",
        prompt:
          "Read Cargo.toml workspace members and crates/*/Cargo.toml. Return a short bullet list of crate names and purposes. Read-only.",
      }),
    () =>
      task({
        description: "Find unsafe blocks in Rust sources.",
        label: "scan unsafe",
        type: "explore",
        prompt:
          "Search for `unsafe` in crates/**/*.rs (exclude target). Summarize count and notable hot paths. Read-only; no edits.",
      }),
    () =>
      task({
        description: "Find unwrap/expect in hot paths.",
        label: "scan unwrap",
        type: "explore",
        prompt:
          "Search for `.unwrap(` and `.expect(` in crates/tui and crates/engine (if present). Note densest files. Read-only.",
      }),
  ]);

  phase("Synthesize");
  const summary = await task({
    description: "Synthesize audit findings for the operator.",
    label: "audit summary",
    type: "general",
    prompt: [
      "Synthesize a concise security/reliability audit from these scout results.",
      "Filter null/failed scouts. Group by severity. No file edits.",
      "",
      "crates:",
      String(crates ?? "(missing)"),
      "",
      "unsafe:",
      String(unsafeHits ?? "(missing)"),
      "",
      "unwrap:",
      String(unwrapHits ?? "(missing)"),
    ].join("\n"),
  });

  return {
    scenario: "WF-A1",
    goal: args?.goal ?? "read-only repo audit",
    summary,
  };
}
