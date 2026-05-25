# Contract: Pi Package Loading

## Package Manifest

`packages/genmicon-pi/package.json` must declare a Pi package:

```json
{
  "name": "genmicon-pi",
  "keywords": ["pi-package"],
  "pi": {
    "extensions": ["./extensions/index.ts"],
    "skills": ["./skills"],
    "prompts": ["./prompts"],
    "themes": ["./themes"]
  },
  "peerDependencies": {
    "@earendil-works/pi-coding-agent": "*",
    "@earendil-works/pi-tui": "*",
    "typebox": "*"
  }
}
```

## Project Settings

`.pi/settings.json` must load the local package with filters:

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

## Acceptance Rules

- Local package source resolves relative to `.pi/settings.json`.
- Package filters load only reviewed GENmicon resources.
- A package source conflict between project and global settings resolves to the
  project entry.
- Remote npm/git sources are blocked for player mode unless pinned and
  explicitly reviewed.

## Implementation Cross-Check

- `packages/genmicon-pi/package.json` declares the Pi manifest, `pi-package`
  keyword, and Pi core peer dependencies.
- `.pi/settings.json` loads only `./packages/genmicon-pi` with explicit
  extension, skill, prompt, and theme filters.
- `packages/genmicon-pi/tests/package-load.test.ts` verifies manifest/settings
  agreement.
- `packages/genmicon-pi/tests/package-trust.test.ts` verifies local reviewed
  loading and remote-source blocking behavior.
