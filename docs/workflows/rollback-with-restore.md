# Roll back a bad turn with /restore

CodeWhale snapshots your working tree into a side git repository (under
`~/.codewhale/snapshots/`, never your own `.git`) before each turn and before
each file-modifying tool call. When a turn goes wrong:

```text
/restore            # list the 20 most recent snapshots
/restore list 50    # list more (up to 100)
/restore 3          # restore the 3rd-most-recent snapshot (1 = newest)
```

Each listing row shows a number, UTC time, short SHA, and a label such as
`pre-turn:12` with the prompt that started the turn — pick the last point
that was good.

`/restore` reverts **workspace files only**; the conversation is kept, so you
can say "that approach broke the build — try X instead" with full context.
Outside YOLO it asks you to run `/trust on` (or `/mode yolo`) first. For the
inverse case — keep the files, unwind the exchange — use `/undo`.

**Done when:** `git status` / your build reflects the restored files and the
conversation still has the history you wanted to keep.

See also: [RESTORE.md](../RESTORE.md) for the full rollback story.
