import type { DocsSubagentsDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/docs/subagents/page.tsx`.
 * Copy moved verbatim from the page's `isZh` ternaries — any wording change
 * belongs in its own commit, never mixed into a structural move.
 */
export const docsSubagents: DocsSubagentsDict = {
  metaTitle: "Sub-Agents · Codewhale Docs",
  metaDescription:
    "The agent tool, fleet roles, context forking, worktree isolation, and concurrency caps.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Sub-Agents",
  overviewLead:
    "A parent session launches one focused sub-agent through the agent tool and immediately gets back an agent_id, a compact receipt, and a transcript handle while the worker runs in the background. Sub-agents inherit the parent's tool registry by default, but they are leaf workers: they do not receive agent or nested lifecycle tools. agent launches detached background work — cancelling the parent turn stops the parent's wait path, but it does not kill already-opened child runs.",
  overviewFleetNote:
    "For work that must survive process restarts, sleep, or remote execution, prefer a fleet or a Workflow-backed fleet run over a short in-session agent call.",
  roles: [
    [
      "worker",
      "Flexible multi-step execution of the parent's brief; writes and shell allowed. The default role.",
    ],
    ["scout", "Read-only, maps the relevant code fast — “find every call site of Foo.”"],
    [
      "planner",
      "Analyse and produce a strategy without executing — “design the migration; don't run it.”",
    ],
    ["reviewer", "Read-and-grade with severity scores — “audit this PR for bugs.”"],
    ["builder", "Land a specific change with minimal edits; writes and shell allowed."],
    ["verifier", "Run tests and validation gates and report the outcome; no code edits."],
    [
      "consultant",
      "Read-only high-reasoning counsel for judgement calls and design critique.",
    ],
    ["custom", "An explicit narrow tool allowlist for locked-down dispatch."],
  ],
  forkTitle: "Context forking",
  forkLead:
    "{agentTool} starts fresh by default: the child gets its role prompt plus the task you pass. When the task depends on decisions, files, todos, or plan state already in the parent transcript, use {forkContext} — the runtime keeps the parent's request prefix byte-identical where available (preserving prefix-cache reuse), appends a structured state snapshot, then adds the sub-agent role instructions and task at the tail. Use fresh sessions for independent exploration and forked sessions for continuation, review, summarization, or compaction work.",
  worktreeTitle: "Worktree isolation",
  worktreeLead:
    "Launch parallel edit lanes with {worktreeFlag}: Codewhale creates a fresh git worktree and branch for the child (default {branchPattern}, checked out beside the parent repo under {worktreeDir}) so the parent checkout stays clean. Isolation is not write authority: a prompt-only worker starts read-only, and a writer also declares {writeAuthority} plus at least one normalized {writeRoots}, {exactFiles}, or {coordinationContracts} value. Overlapping shared write claims fail before any mutation.",
  capacityTitle: "Concurrency caps",
  capacityLead:
    "The sub-agent capacity source of truth is crates/tui/src/config/subagent_limits.rs: default configured concurrency is 64, maximum configured concurrency is 128, and maximum admitted running-plus-queued work is 1024. These are capacity ceilings, not advice to dispatch every slot — a manager should use the smallest useful fan-out, keep a single fan-in owner, and verify worker receipts before reporting combined completion.",
  sourceNote: "Source document: docs/SUBAGENTS.md · Update docs-map.ts when changing.",
};
