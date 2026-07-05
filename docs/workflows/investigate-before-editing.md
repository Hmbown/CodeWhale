# Investigate before editing

For an unfamiliar codebase, make CodeWhale explore and propose before it
touches anything. Plan mode keeps read-only tools available and disables
shell and patch execution.

```text
/mode plan
```

Then ask for a reviewable plan:

```text
Trace how login sessions are validated. List the files involved, then
propose a fix plan for the expiry bug — do not edit anything yet.
```

Read the plan. When you agree, press `Tab` (or run `/mode agent`) to switch to
Agent mode and say "execute the plan".

**Done when:** you saw the file list and plan *before* the first edit, and the
execution follows the plan you approved.

See also: [MODES.md](../MODES.md) (tool availability by mode), [GUIDE.md](../GUIDE.md).
