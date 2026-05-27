# File Policy Engine

The file policy engine extends `execpolicy.toml` with path-level allow and
deny rules for built-in file tools. It is controlled by the `file_policy`
feature flag and is enabled by default.

## Configuration

Policies are loaded from `~/.deepseek/execpolicy.toml`. Command rules continue
to use the existing `[rules.<name>]` tables. File rules use `[file.<tool>]`
tables keyed by tool name, with an optional `[file.default]` fallback.

```toml
[file.write_file]
allow = ["src/**/*.rs", "*.md"]
deny = [".env", "src/secrets.rs"]

[file.read_file]
allow = ["docs/**/*.md", "README.md"]
deny = [".env", "**/*.key"]

[file.default]
allow = ["*"]
deny = [".env", "**/*.pem", "**/*.key"]
```

Deny rules are evaluated before allow rules. If no file rules are configured,
file tools keep the existing permissive behavior. If rules are configured and a
path matches neither allow nor deny, the evaluator returns `AskUser`; current
engine integration treats that as non-blocking and only blocks explicit deny
matches.

## Matching

Path patterns are glob-style strings:

- `*` matches within one path component.
- `**` matches across directory boundaries.
- `src/**/*.rs` matches both `src/main.rs` and `src/bin/tool.rs`.

## Enforcement Points

The engine checks file policy in three places:

- Serial tool execution before snapshotting or running a file tool.
- Parallel tool batches before scheduling each tool task.
- `execute_tool_with_lock` as a defensive guard for alternate execution paths.

Denied calls return a `ToolError::permission_denied` and emit a
`tool.file_policy_denied` audit event.

## Covered Tools

The current file-tool set is:

- `read_file`
- `write_file`
- `edit_file`
- `apply_patch`

`apply_patch` is classified as a file tool, but its structured input does not
always expose a single primary path. When `path` is present, that override is
checked. Otherwise, policy extraction scans unified diff headers and checks each
touched file path.
