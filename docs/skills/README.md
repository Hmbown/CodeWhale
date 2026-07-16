# Maintainer / agent skills

GitHub-stewardship and release-QA workflows for maintaining Codewhale, codified as
`SKILL.md` skills (same format Claude Code and Codewhale both load). They encode the
workflows used to assemble the v0.8.61 release.

To activate:
- **Claude Code:** copy a skill dir into `.claude/skills/` (project) or your user skills dir.
- **Codewhale:** copy into Codewhale's `skills_dir` (e.g. `~/.codewhale/skills/`), or bundle
  into `crates/tui/assets/skills/` + register in `crates/tui/src/skills/system.rs` to ship it.

Skills: gh-file-issue, gh-compile-issues, gh-assign-issues, gh-plan-issues, gh-find-prs,
gh-treasure-hunt, gh-close-issues, gh-credit-harvest, codew-release-qa-sweep.
