# Data Model: Pi-Native GENmicon Game Driver

## GENmicon Driver Package

**Purpose**: Reviewed Pi package or project-local resource set that makes
GENmicon available inside a Pi session.

**Fields**:

- `name`: package name, initially `genmicon-pi`
- `source`: local path, pinned npm spec, or pinned git spec
- `review_status`: `reviewed`, `unreviewed`, or `blocked`
- `loaded_extensions`: extension paths enabled for this feature
- `loaded_skills`: skill names enabled for game mode
- `loaded_prompts`: prompt template names enabled for game mode
- `loaded_themes`: theme names enabled for game mode
- `filters`: resource filters applied by `.pi/settings.json`

**Validation Rules**:

- Player mode requires `review_status = reviewed`.
- Project-local entries override global entries for the same package identity.
- Resource filters must not accidentally load unreviewed extension code.
- Pi core packages imported by the package must be peer dependencies, not
  bundled runtime dependencies.

## Game Cartridge

**Purpose**: Local game package containing authored content and save roots.

**Fields**:

- `game_id`
- `title`
- `version`
- `root`
- `manifest_path`
- `entry_skill`
- `content_roots`
- `driver_id`
- `driver_requirement`
- `save_root`
- `default_save`
- `optional_assets`

**Validation Rules**:

- All cartridge paths must resolve under the cartridge root.
- Cartridge content is untrusted data and cannot grant Pi tools or policy.
- Missing optional assets produce diagnostics, not launch failure.

## Game Driver

**Purpose**: Reusable genre/runtime contract used by cartridges.

**Fields**:

- `driver_id`
- `driver_version`
- `driver_root`
- `skills`
- `declared_functions`
- `render_templates`
- `agent_roles`

**Validation Rules**:

- Existing saves must reload with their recorded exact driver version.
- Driver functions must be declared before they are callable.
- Driver code cannot write save files directly; commits flow through save
  authority.

## Game Save

**Purpose**: Authoritative game progress.

**Fields**:

- `save_id`
- `state_path`
- `turn_log_path`
- `revision`
- `driver_id`
- `driver_version`
- `state`
- `turns`
- `summary`
- `agent_roster`

**Validation Rules**:

- `revision` must match `expected_revision` before commit.
- Commit writes exactly one turn record and one state update.
- `STATE.json` and `TURN_LOG.jsonl` are authoritative after restart.

**State Transitions**:

- `unloaded` -> `validated` after package and cartridge validation.
- `validated` -> `loaded` after save and driver resolution.
- `loaded` -> `committing` when a player turn requests durable state changes.
- `committing` -> `loaded` after a successful atomic commit and render refresh.
- `committing` -> `error` on revision conflict or validation failure.

## Player Capability Profile

**Purpose**: Exact allowlist of model-callable capabilities available during
player mode.

**Fields**:

- `mode`: `player` or `developer`
- `active_tools`
- `hidden_renderers`
- `developer_only_tools`
- `last_verified_at`

**Validation Rules**:

- Player mode excludes shell, file editing, git, package installation, provider
  configuration, broad external integrations, and raw state mutation.
- Developer mode may show diagnostics but must not silently widen player tools.

## Game View

**Purpose**: Player-facing view derived from the active save.

**Fields**:

- `scene`
- `status`
- `items`
- `tasks`
- `dialogue`
- `choices`
- `action_composer_state`
- `validation`
- `fallback_mode`

**Validation Rules**:

- View data is derived and disposable.
- If rich UI cannot render, compact text view must preserve playability.
- View refresh after commit reads from saved state, not model prose.

## Developer Diagnostic View

**Purpose**: Author-facing inspection surface.

**Fields**:

- `package_source`
- `loaded_resources`
- `active_tools`
- `save_revision`
- `driver_identity`
- `render_snapshot`
- `warnings`
- `last_runtime_command`

**Validation Rules**:

- Diagnostics are explicit and reversible.
- Diagnostics do not alter active player tools or save authority.
