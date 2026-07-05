# Use Plan, Agent, and YOLO deliberately

Press `Tab` in an idle composer to cycle **Plan → Agent → YOLO**, or switch
directly:

```text
/mode plan    # read-only investigation; no shell, no patches
/mode agent   # default: edits allowed, shell approval-gated
/mode yolo    # trust mode + auto-approve everything; trusted repos only
```

Approval is a separate dimension from mode. Set it in `/config`
(`approval_mode`): `suggest` (default, per-mode rules), `auto` (approve all
tool calls), or `never` (block anything that isn't read-only).

A good default rhythm: **Plan** to scope the work, **Agent** to execute with
approvals, and YOLO only for repos where you'd accept any local change —
YOLO also lifts the workspace file boundary.

**Done when:** you can predict, before each turn, whether the agent may edit
files and whether a shell command will prompt you.

See also: [MODES.md](../MODES.md) — the full tool-availability-by-mode table.
