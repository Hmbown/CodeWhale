# <Module Or Feature> Spec

Status: Draft
Owner: Maintainer
Last reviewed: 2026-05-18

## Purpose

Describe what this module or feature owns in the product. Keep this section
focused on user-visible responsibility and long-term boundary, not internal
implementation trivia.

## Ownership Boundary

This spec owns:

- The behavior, commands, config, tools, screens, APIs, or data formats covered
  by this module.

This spec does not own:

- Adjacent surfaces that should be governed by another active spec.
- Temporary goal tracking; use `SPEC_files/goals/` for short-lived workstream
  notes and promote durable decisions back into an active spec.

## Source Anchors

Primary code:

- `path/to/main/module.rs`

Related code:

- `path/to/related/module.rs`

Canonical docs:

- `docs/example.md`

Tests and fixtures:

- `path/to/tests`

## Maintainer Prompt

Copy this block when asking the agent to change this area:

```markdown
Spec: SPEC_files/<this-file>.md
Goal:
Why it matters:
Current behavior:
Desired behavior:
Must include:
Must not include:
Affected commands/config/tools/UI:
Backward compatibility requirements:
Acceptance criteria:
Validation I expect:
```

## Current Behavior

- What users can do today.
- Which commands, config keys, tools, screens, or APIs are involved.
- Important compatibility behavior that must not regress.

## Planned Or Reserved Behavior

- Future behavior that is intentionally not shipped yet.
- Reserved config keys, commands, tools, APIs, or file formats.
- Explicit non-goals for this spec.

Do not describe planned behavior as available. Move it to Current Behavior only
after code, docs, tests, and validation evidence exist.

## Design Principles

- Principle 1.
- Principle 2.
- Principle 3.

## Change Workflow

Before implementation:

- Confirm this is the correct spec.
- Check whether a more specific nested spec also applies.
- Identify all touched code, docs, tests, localization, and release notes.
- Write acceptance criteria if the prompt did not include them.

During implementation:

- Keep behavior changes scoped to this module unless the spec names another
  module.
- Preserve compatibility aliases unless the acceptance criteria explicitly
  remove them.
- Update user-facing docs and UI text with the code.

Before completion:

- Run the validation gates below.
- Audit every acceptance criterion against real file or command evidence.
- Update this spec when the module boundary or shipped behavior changes.
- Record any remaining open decision instead of hiding it in prose.

## Acceptance Criteria Checklist

- [ ] User-facing behavior is implemented.
- [ ] Backward compatibility is preserved or documented as intentionally
      changed.
- [ ] Tests cover the changed behavior.
- [ ] Relevant docs and help text are updated.
- [ ] Validation gates pass or known failures are explained.

## Validation Gates

- `cargo build`
- `cargo test --workspace --all-features`
- `cargo clippy --workspace --all-targets --all-features`
- `cargo fmt --all --check`

Narrow changes can use narrower tests first, but completion needs a clear reason
if the full gate is not run.

## Risks

- Risk 1.
- Risk 2.

## Open Decisions

- Decision 1.
- Decision 2.
