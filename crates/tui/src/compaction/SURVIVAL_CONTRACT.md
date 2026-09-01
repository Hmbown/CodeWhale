# Compaction survival contract

Schema version 1.

Language-invariant schema for session-tree journal entry types. Compaction
may summarize older turns; it must not drop the fields marked **survive**
below. The B1 strategy (Rust `codewhale-tui` compaction path) enforces this
over the live journal / API transcript. Later TS/Go strategies validate
against the same field names.

Failed compact must not replace live history. A second compact must replace
the prior summary instead of stacking it, and must not grow the cacheable
prefix by duplicating checkpoints. Internal protocol placeholders
(`(no summary available)`, refusal-only text) are never durable transcript
content.

Live repository, tool, and GitHub state is distinguished from summarized
facts: background-work handles and the exact branch/worktree identity
survive here or are re-read from live state before consequential work.

## Entry types

Journal kinds are the protocol `kind` strings (`header`, `user`,
`assistant`, `tool_result`, `compaction`, `branch_summary`). Session-tree
`message` / `system` entries carry the same payload on `SessionEntryKind`.

| kind | payload location | survive | summarized | pruned | not retained |
| --- | --- | --- | --- | --- | --- |
| `header` | `payload` identity, `id`, `created_at` | always | — | — | secrets |
| `user` | `payload` text / `SessionEntryKind::User.text` / `Message` text blocks | last-round verbatim (bounded token budget for older user text) | older user turns | obsolete narration | secrets, raw dumps |
| `assistant` | `payload` text / `SessionEntryKind::Assistant.text` / `Message` text + `tool_use` | last-round verbatim | older assistant text | unsigned thinking beyond cap | internal placeholders |
| `tool_result` | `payload.tool_use_id`, `content`, `is_error`, `content_blocks` | last-round bounded (8KiB) | older results | nested images, oversized blocks | secrets, raw large outputs |
| `compaction` | `payload.summary`, `tokens_before`, `tokens_after`, `model`, coverage receipt | latest checkpoint only | prior summaries replaced | stacked prior summaries | placeholder-only summaries |
| `branch_summary` | `payload.branch_id`, `summary`, `parent_branch_id` | always (not rewritten by compact) | — | — | — |
| `message` | flattened `Message.role` + `Message.content[]` | see content-block table | older turns | see prune rules | see never-durable |
| `system` | `SessionEntryKind::System.content` | constitution / repo-law identity when present | duplicated policy | — | unsupported completion claims |

## Content blocks on `message` entries

| block `type` | fields that must survive | bound / notes |
| --- | --- | --- |
| `text` | `text` on the last user round | older user text may truncate to the retained-user token budget |
| `tool_use` | `id`, `name`, `input`, `thought_signature` | last-round only; `id` is the join key for `tool_result` |
| `tool_result` | `tool_use_id`, `content`, `is_error` | last-round; `content` truncated at 8KiB, `content_blocks` dropped if truncated |
| `thinking` | `signature` (byte-for-byte) | unsigned `thinking` truncated at 4KiB; signed thinking is never rewritten |
| `image_url` | last-round images | older images may be pruned with tool results |
| `server_tool_use` / `*_tool_result` | last-round ids | older blocks may be summarized |

## Fields restated from live state (not the summary)

These must remain usable after compact. They live outside free-form summary
text; if the journal copy is missing they are re-read before consequential
work.

| field | where it lives | after compact |
| --- | --- | --- |
| `/anchor` pin | `.codewhale/anchors.md` (legacy `.deepseek/anchors.md`) | restated verbatim on the checkpoint user message |
| compaction path + coverage | `CompactionCoverage` on the result / `/context` inspector | named as `summary` or `prune-only` plus last-round counts |
| compaction receipt | checkpoint `user` message whose text contains the summary marker; TUI `HistoryCell::System` | latest receipt kept; failed compact does not write a replacement |
| billed parent prompt tokens | `TurnContext.latest_parent_input_tokens` / session-carried copy | survive the turn boundary; cleared on history rewrite |
| branch / worktree identity | live git / workspace | re-read from live state |
| background-work / WorkRef handles | session + worker registry | survive or re-read from live state |
| child usage / cumulative tool steps | turn billing totals | must not create false compaction pressure |

## Enforcement

Rust now:

- `last_round::build_replacement_history` keeps the bounded last user round
  (assistant + tool results) and appends one checkpoint receipt. A trailing
  toolless user/assistant tail still walks back to the last tool-bearing
  round; chat-only sessions keep only the latest user turn.
- `validate_last_round_coverage` refuses the rewrite if any of that round's
  user texts, tool-call ids, tool-result ids, or assistant texts would vanish.
  Every user turn in the round is checked, not just the first: the round spans
  a tool-bearing turn plus the toolless tail after it, so checking one would
  let the latest turn -- the one this contract exists for -- be dropped. The
  assistant check compares text, because "some assistant message survived" is
  satisfied by the summary the rewrite itself just wrote.
- `validate_survival_contract` also refuses a missing checkpoint receipt,
  duplicated prior summaries, dropped `/anchor` text, or a placeholder-only
  checkpoint.
- `compact_messages_safe` never mutates the caller's live history; the host
  commits only after `Ok`.

The language-invariant fixture matrix is
`crates/tui/src/compaction/fixtures/matrix.json`. Rust loads it in
`last_round` tests. `validate_survival_contract.mjs` is the same coverage
floor for a later TypeScript strategy (`node validate_survival_contract.mjs`).
There is no Go runtime in this repository yet; a Go validator remains a
follow-up when a Go strategy exists.

`/context` names the compaction path and `/anchor` survival.
