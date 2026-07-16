# Claude Plugin Compatibility

Codewhale treats Claude Code skill folders as instruction bundles when they are
plain `SKILL.md` directories. It does not run Claude Code plugin runtimes.

## Supported

- Workspace or global `.claude/skills/<name>/SKILL.md` directories discovered by
  the normal skill registry.
- GitHub or tarball installs that contain one selected skill directory such as
  `skills/<name>/SKILL.md`, `.agents/skills/<name>/SKILL.md`,
  `.claude/skills/<name>/SKILL.md`, or a nested package layout ending in
  `skills/<name>/SKILL.md`.
- Companion files inside the selected skill directory, such as `references/`,
  `examples/`, or scripts that are only used after the skill is explicitly
  loaded and trusted.

## Not Supported As A Plugin Runtime

Claude Code plugin features remain outside the v0.8.60 compatibility boundary:

- `.claude-plugin/plugin.json` metadata and activation semantics.
- Custom slash-command bundles.
- Plugin build steps, compiled TypeScript agents, dashboard servers, shared
  plugin state, or token-gated service processes.
- Frontmatter fields that require Claude-specific runtime behavior, such as
  `model: inherit`.

If a Claude Code plugin repository contains multiple skills, install or migrate
one `skills/<name>` directory at a time. `/skill install` rejects multi-skill
plugin archives with a clear message so it never silently chooses one skill and
drops the plugin runtime behavior.

For richer integrations, wrap the plugin's executable surface as MCP, hooks, or
a Codewhale skill that names the external command explicitly.
