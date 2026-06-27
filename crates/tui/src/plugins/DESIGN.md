# Plugin System Design (v0.1)

> Inspired by: Claude Code 2.1.88 plugin architecture  
> Built on: CodeWhale skills system + MCP config + tool registry  
> Status: design draft — pending review

---

## 1. Motivation

CodeWhale currently has:
- Skills system — SKILL.md discovery/registration/SkillTool
- MCP config — `config.toml` `[mcp_servers]` section
- Tool system — built-in tool registry
- Missing: **Plugin container** — no way to bundle "skills + MCP + hooks" as a toggleable unit

Claude Code's plugin system bundles **skills + MCP servers + hooks + commands + agents** into a `BuiltinPluginDefinition`, managed via `/plugin` toggle.

Value for CodeWhale:
- Bundle skills and MCP servers as **named functional units**
- Users can **enable/disable per plugin** (unused plugins don't consume context window)
- Built-in plugins work out of the box; user plugins extend freely
- Foundation for future marketplace

---

## 2. Design Overview

```
Plugin Registry (plugins/registry.rs)
  built-in (assets/plugins/)     user (~/.codewhale/plugins/)
       |                               |
       v                               v
  Plugin { name, description, version
           skills: Vec<Skill>
           mcp_servers: McpConfig
           enabled: bool }
       |
       v
  System Prompt Injection
  (render enabled plugins as available capabilities)
```

**Core Principles**:
- Plugin is a container for skills — does NOT modify the skills system
- One `plugin.toml` defines one plugin
- Built-in plugins in `assets/plugins/`, user plugins in `~/.codewhale/plugins/`
- Does NOT conflict with existing `tools/plugin.rs` (script tools) — different module, different purpose

---

## 3. Plugin Manifest Format (`plugin.toml`)

```toml
[plugin]
name = "rust-toolkit"
description = "Rust development tools: cargo check, clippy, test runner"
version = "1.0.0"
default_enabled = true

[skills]
paths = [
    "skills/rust-check/",
    "skills/rust-test/",
]

[mcp_servers.crates-io]
command = "crates-mcp"
args = ["--stdio"]
description = "Search crates.io for Rust packages"

[when]
os = ["windows", "linux", "macos"]
required_binaries = ["cargo"]
```

---

## 4. Implementation Plan

### 4.1 File Map

```
crates/tui/src/plugins/
  DESIGN.md              this file
  mod.rs                 module entry + re-exports
  manifest.rs            PluginManifest + plugin.toml parsing
  registry.rs            PluginRegistry (load/enable/disable/list)
  discovery.rs           scan assets/plugins/ + ~/.codewhale/plugins/
  injection.rs           system prompt injection per enabled plugins
```

### 4.2 Data Types (`manifest.rs`)

```rust
#[derive(Debug, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginMeta,
    pub skills: Option<PluginSkills>,
    pub mcp_servers: Option<HashMap<String, McpServerConfig>>,
    pub when: Option<PluginWhen>,
}

#[derive(Debug, Deserialize)]
pub struct PluginMeta {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    #[serde(default = "default_true")]
    pub default_enabled: bool,
}

fn default_true() -> bool { true }

#[derive(Debug, Deserialize)]
pub struct PluginSkills {
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PluginWhen {
    pub os: Option<Vec<String>>,
    pub required_binaries: Option<Vec<String>>,
}

pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub source: PluginSource,
    pub enabled: bool,
    pub skills: Vec<Skill>,
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

pub enum PluginSource {
    Builtin { path: PathBuf },
    User { path: PathBuf },
}
```

### 4.3 Plugin Registry (`registry.rs`)

```rust
pub struct PluginRegistry {
    builtins: HashMap<String, LoadedPlugin>,
    users: HashMap<String, LoadedPlugin>,
    user_overrides: HashMap<String, bool>,
}

impl PluginRegistry {
    pub fn load_all() -> Self;
    pub fn enable(&mut self, name: &str) -> Result<(), String>;
    pub fn disable(&mut self, name: &str) -> Result<(), String>;
    pub fn list(&self) -> Vec<PluginSummary>;
    pub fn enabled_skills(&self) -> Vec<Skill>;
    pub fn enabled_mcp_servers(&self) -> HashMap<String, McpServerConfig>;
    pub fn is_available(&self, name: &str) -> bool;
}
```

### 4.4 CLI Commands

In `commands/groups/plugins/`:

```
/plugin list              list all plugins + status
/plugin enable <name>     enable a plugin
/plugin disable <name>    disable a plugin
/plugin info <name>       show plugin details
```

### 4.5 Integration with Existing Modules

| Module | Relationship | Changes |
|--------|-------------|---------|
| `skills/mod.rs` | Referenced by plugin | **No change** — plugins load SKILL.md via skill paths |
| `tools/plugin.rs` | Same name, different concept | **No change** — script tool system, independent |
| `tools/skill.rs` | SkillTool | **No change** — AI uses skill tool to load individual skills |
| `config.rs` | Store user plugin enable state | **+10 lines** — `[plugins]` section |
| `prompts.rs` | Inject plugin list | **+15 lines** — add enabled plugins section to system prompt |
| `mcp.rs` | Merge plugin MCP servers | **+20 lines** — merge with existing `[mcp_servers]` |

---

## 5. Adoption Path (compatible transition)

```
Phase 1: Plugin registry + manifest (this PR)
  Does not affect existing skill system, plugins are optional

Phase 2: Migrate 4 built-in skills into 1 built-in plugin
  verify / simplify / stuck / batch -> "code-review" plugin

Phase 3: Support marketplace remote install
  Similar to skills/install.rs registry pattern
```

---

## 6. Task Assignment

| # | Task | File | Est. | Owner | Status |
| |------|------|------|-------|--------|
| 6.1 | PluginManifest + TOML parsing + tests | `plugins/manifest.rs` | half-day | cpt-opcd | ✅ |
| 6.2 | PluginRegistry (load/enable/disable/list) | `plugins/registry.rs` | 1 day | cpt-opcd | ✅ |
| 6.3 | Built-in plugin directory scan | `plugins/discovery.rs` | half-day | cpt-opcd | ✅ |
| 6.4 | System prompt injection | `plugins/injection.rs` | half-day | cpt-opcd | ✅ |
| 6.5 | CLI `/plugin` commands | `commands/groups/plugins/` | half-day | mydpsk | ✅ |
| 6.6 | MCP server merge logic | `registry.rs enabled_mcp_servers()` | half-day | mydpsk | ✅ |
| 6.7 | 1 example built-in plugin (`rust-toolkit`) | `assets/plugins/rust-toolkit/` | half-day | mydpsk | ✅ |
| 6.8 | Integration tests + CI verification | `plugins/tests.rs` | half-day | mydpsk | ✅ |

---

## 7. Acceptance Criteria

- [ ] `cargo check -p codewhale-tui` no errors
- [ ] `cargo test -p codewhale-tui -- plugins` all pass
- [ ] Built-in plugin `rust-toolkit` appears in `/plugin list`
- [ ] User can `/plugin disable rust-toolkit`
- [ ] Disabled plugins not injected into system prompt
- [ ] Existing skill system tests all pass (zero regression)
- [ ] `tools/plugin.rs` (script tools) unaffected

---

## 8. References

- Claude Code 2.1.88: `src/plugins/builtinPlugins.ts`, `src/types/plugin.ts`
- CodeWhale existing: `crates/tui/src/skills/DESIGN.md`, `crates/tui/src/tools/plugin.rs`
