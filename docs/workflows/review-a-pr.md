# Review a PR

Two built-in surfaces run a CodeWhale-powered review over a git diff.

In the TUI, use the `/review` command with the branch, range, or PR you want
reviewed — it activates the bundled `review` skill:

```text
/review origin/main...feature/retry-backoff
```

From a script or CI shell, use the CLI:

```bash
codewhale review
```

Ask for a verdict, not vibes: "flag correctness bugs and risky changes, cite
file and line for each finding, and say what you did *not* check."

**Done when:** every finding cites a file/line you can open, and the review
states its blind spots instead of implying full coverage.

See also: [GUIDE.md](../GUIDE.md), [SUBAGENTS.md](../SUBAGENTS.md) (the
read-only `review` role for parallel review work).
