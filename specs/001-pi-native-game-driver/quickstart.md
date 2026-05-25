# Quickstart: Pi-Native GENmicon Game Driver

## 1. Load The Local Package

Ensure `.pi/settings.json` points to the reviewed local package:

```json
{
  "packages": [
    {
      "source": "./packages/genmicon-pi",
      "extensions": ["extensions/index.ts"],
      "skills": ["skills/**/SKILL.md"],
      "prompts": ["prompts/*.md"],
      "themes": ["themes/genmicon.json"]
    }
  ]
}
```

## 2. Validate A Fixture Cartridge

Run Pi from the repository root and use:

```text
/genmicon:validate examples/games/reconciliation-demo
```

Expected result:

- package source is local and reviewed
- game id is `reconciliation-demo`
- driver is resolved
- default save is readable
- player active-tool allowlist is shown

## 3. Start Player Mode

```text
/genmicon:play examples/games/reconciliation-demo --save default
```

Expected result:

- the first visible surface is the game console
- raw tool output and coding-agent chrome are hidden
- the action composer is focused for player input

## 4. Submit A Player Action

Enter a natural action such as:

```text
I admit I was scared and ask her to wait.
```

Expected result:

- the driver resolves the action
- `game_commit_turn` writes exactly one turn
- the view refreshes from the updated save

## 5. Inspect Diagnostics

```text
/genmicon:dev on
```

Expected result:

- package source, loaded resources, save revision, driver id/version, render
  snapshot, and warnings are visible
- player active tools are unchanged

Turn diagnostics off:

```text
/genmicon:dev off
```

Check current diagnostic visibility without changing player mode:

```text
/genmicon:dev status
```

Expected result:

- diagnostic visibility is reported as on or off
- the player active-tool allowlist remains unchanged
- no save state is written

## 6. Resume

List available saves:

```text
/genmicon:saves examples/games/reconciliation-demo
```

Expected result:

- save ids are listed from the runtime helper
- each valid save reports revision and driver id
- malformed saves produce warnings instead of transcript-derived state

Restart Pi and run:

```text
/genmicon:play examples/games/reconciliation-demo --save default
```

Expected result:

- the game resumes from `STATE.json` and `TURN_LOG.jsonl`
- previous transcript history is not required for correctness
- the Pi session receives fresh derived context from runtime `status` and
  `render` snapshots

## 7. Validation Commands

Run focused checks after implementation:

```bash
cargo test -p deepseek-game --all-features
```

Latest evidence captured during implementation:

- `cargo test -p deepseek-game --all-features`: passed, including runtime
  helper JSON envelope, fixture validation, commit-once, and resume tests.
- `npm run typecheck` from `packages/genmicon-pi`: passed.
- `npm test` from `packages/genmicon-pi`: passed, 51 package tests.
- `cargo metadata --no-deps --format-version 1`: passed after removing
  `crates/kernel` from the workspace.
