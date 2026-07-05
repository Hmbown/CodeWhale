# Your first real task

Give CodeWhale a small, verifiable change — one with a test or observable
behavior you can check — instead of a vague "improve the code".

```bash
cd path/to/project
codewhale
```

Then describe the task with acceptance criteria in the composer:

```text
Fix the off-by-one in the pagination helper. Done when: the existing
pagination tests pass and the last page shows the remaining items.
```

CodeWhale starts in **Agent** mode: it reads files and edits without prompting,
and asks for approval before each shell command. Review each approval — the
command is shown before it runs.

**Done when:** the agent reports the change with evidence (test output, diff),
and you can re-run the verification yourself.

See also: [GUIDE.md](../GUIDE.md), [MODES.md](../MODES.md).
