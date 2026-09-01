# Maintainer / agent skills

GitHub-stewardship and release-QA workflows for maintaining Codewhale, codified as
`SKILL.md` skills (same format Claude Code and Codewhale both load). They encode the
issue-triage, PR-harvest, credit, and release-QA workflows the maintainers run each
release.

For end-user Skills Manager behavior (ownership, audit, import, trust), see
[../SKILLS.md](../SKILLS.md).

To activate:
- **Claude Code:** copy a skill dir into `.claude/skills/` (project) or your user skills dir.
- **Codewhale:** copy into a Codewhale-owned root (e.g. `~/.codewhale/skills/`), import via
  `/skills`, or bundle into `crates/tui/assets/skills/` + register in
  `crates/tui/src/skills/system.rs` to ship it.

Skills: gh-file-issue, gh-compile-issues, gh-assign-issues, gh-find-prs,
gh-treasure-hunt, gh-close-issues, gh-credit-harvest, codew-release-qa-sweep.
