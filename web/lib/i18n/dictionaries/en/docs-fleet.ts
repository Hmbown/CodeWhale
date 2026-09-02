import type { DocsFleetDict } from "../types";

export const docsFleet: DocsFleetDict = {
  metaTitle: "Fleet & Workflow · Codewhale Docs",
  metaDescription:
    "The durable Agent roster and member-selection layer, plus the optional Workflow orchestration overlay.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Fleet & Workflow",
  overviewLead:
    "Fleet is the durable roster: who is available and which member is selected. It is not an execution or authority engine. Runtime launches and tracks the selected member as a headless codewhale exec run, owns retry and remote placement, and writes the durable receipts and ledger projection. The ledger file, saved rosters, config tables, and the workflow --fleet flag share the Fleet name.",
  runTitle: "Run a fleet",
  runLead:
    "The Runtime's fleet-run projection lives in the workspace's .codewhale/fleet.jsonl ledger, with worker logs under .codewhale/fleet/. codewhale fleet resume <run-id> asks Runtime to replay the ledger and reconcile stale leases; it is idempotent after a manager exit, laptop sleep, or runtime restart.",
  statusLead:
    "The TUI command {fleetStatusTui} and the shell command {fleetStatusShell} read the same durable fleet ledger. Use {fleetWorkers} (or {subagents}) for the sub-agents attached only to the current interactive session.",
  profilesTitle: "Saved fleets, roles, and /fleet setup",
  profilesLead:
    "{fleetSaved} opens the picker for named saved fleets; bare /fleet opens the selected fleet's member roster. /fleet setup opens a progressive wizard for authoring a reusable roster member: one focused choice at a time — semantic role, model (inherit or a concrete configured route), thinking tier, then an exact identity/route review before save. Profiles live in project scope (.codewhale/agents/<role>.toml) or personal scope ($CODEWHALE_HOME/agents/<role>.toml); a same-id project profile wins. Runtime separately owns trust, filesystem/network reach, secrets, approvals, sandboxing, and tools, so profile storage scope never widens execution authority.",
  workflowTitle: "Workflow orchestration",
  workflowLead:
    "Ordinary multi-agent work does not need Workflow: send normal messages in Operate and let Codewhale prefer background workers when parallelism, isolation, or duration makes delegation useful. Use Workflow when ordered phases, gates, shared budgets, replay, or deterministic fan-in matter. A Workflow script coordinates only: it selects fleet members but has no filesystem or shell; Runtime launches the real workers under live authority policy. Scripts use a declarative compile-only JS subset that lowers to a typed WorkflowSpec validated and executed by Rust; import, fetch, process, eval, and async/await are rejected.",
  workflowLimits:
    "Default validation bounds: up to 1,000 worker agents per Workflow run, Workflow IR structural nesting no deeper than 5, loops must declare max_iterations, and dynamic expand nodes must declare max_children plus a template. Runtime child delegation is a separate execution budget: it defaults to 3 levels and has an opt-in hard ceiling of 8. These are population and shape limits, not launch concurrency: Runtime admits at most 16 live workers for one run and queues the rest. Omitted or zero max_steps stays unbounded; only a positive value adds a model-turn ceiling.",
  sourceNote:
    "Source documents: docs/FLEET.md, docs/WORKFLOW_AUTHORING.md · Update docs-map.ts when changing.",
};
