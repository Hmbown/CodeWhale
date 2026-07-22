# Delegated coordination contract

Codewhale records the small amount of shared state that parallel work needs to
remain attributable. This is coordination metadata, not an approval system and
not a store for model reasoning or transcripts.

## Launch and write ownership

Every write-capable child persists the same `ChildLaunchManifest` used by the
runtime. Its mutation claim contains normalized repo-relative directory roots,
exact files, and named contracts. Paths that are absolute or escape with `..`
fail validation.

A prompt-only general child starts read-only. Callers that want a writer must
declare at least one `write_roots`, `exact_files`, or
`coordination_contracts` value. Codewhale does not infer a repo-wide `.` claim.
An active shared-workspace claim blocks another active owner when either tree
contains the other, exact files collide, or a named contract matches. A real
isolated worktree may proceed concurrently. Scope expansion uses
`agents/coordinate action=claim`; a collision records a bounded contention
receipt and fails before mutation without opening a permission modal.

Fleet workers follow the same rule. Write-capable Fleet tasks declare
`workspace.writable_paths` or `metadata.coordination_contracts`, and the
resolved values are persisted in their launch manifest.

This record is a cooperative Codewhale coordination boundary, not an operating
system sandbox. Fleet carries a machine-readable outer cap into each worker,
rechecks structured mutation targets, rejects symlink aliases, and denies
unbounded shell, Git, code, plugin, and mutating MCP execution. Those checks
prevent one Codewhale worker from silently exceeding its declared claim; they
do not promise containment against a separate hostile process racing filesystem
paths. Use an OS sandbox or an isolated host when that adversarial boundary is
required.

Authority-bound Fleet subprocesses are explicit leaves in v0.9.1. Their MCP,
LSP, snapshot, custom-tool, plugin, shell, and nested-agent startup surfaces are
disabled so configured background executables cannot bypass the structured
mutation path. The persisted receipt reports `max_spawn_depth = 0`.

## Decisions and projected context

Coordination schema version 1 persists decision records with a stable id,
subject, proposed/accepted/superseded status, one owner, applicability scope,
concise constraints, evidence handles, version, and sequence. Only the owner
may change a decision's status. A second accepted decision for the same subject
cannot silently replace the first.

At child launch, Codewhale projects only accepted decisions whose scope matches
the child's declared paths, contracts, role, or tool capabilities. The
projection is deduplicated, limited to eight decisions and 4096 UTF-8 bytes,
and receipted by child id and decision ids. The task prompt may separately
carry at most eight explicit dependency facts and eight observable acceptance
checks. Parent transcripts, secrets, and raw reasoning are never projected.

## Neutral fan-in

Conflicting candidates remain preserved as branch, patch, or artifact handles.
The neutral owner is the nearest common Planner/manager/operator in the
persisted parent tree, falling back to the root release owner. Neither candidate
author may claim that role. Reconciliation records:

- both or all input decision ids and candidate handles;
- a retry count and a limit of at most three;
- distinct independent Reviewer and Verifier evidence handles;
- a verified, failed, or blocked verification outcome; and
- the neutral disposition and bounded evidence handles.

Retry exhaustion is a terminal, inspectable receipt, not permission to discard
either candidate. Restart/replay preserves the schema, decisions, claims,
contention, projections, and reconciliation sequence.

## Inspection

`agents/list` exposes concise per-child claims and accepted decisions.
`agents/coordinate action=inspect` exposes bounded decision, claim, contention,
projection, and reconciliation receipts plus deterministic hottest-path counts.
Metrics without an authoritative source, such as package growth or route cost,
remain explicitly null instead of being inferred.
