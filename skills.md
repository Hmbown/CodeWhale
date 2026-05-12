# TriadMind Session SOP

Bootstrap version: 1.0

## 1) New window startup (mandatory)

Run one of:

- Linux/macOS: `bash .triadmind/session-bootstrap.sh`
- Windows PowerShell: `.\.triadmind\session-bootstrap.ps1`
- Windows CMD: `.triadmind\session-bootstrap.cmd`

## 2) Read artifacts before coding

- `.triadmind/triad-map.json`
- `.triadmind/runtime-map.json`
- `.triadmind/runtime-diagnostics.json`
- `.triadmind/view-map.json`
- `.triadmind/bootstrap-verify.json` (latest session gate result)
- `.triadmind/profile.json`

## 3) Fail-closed rule

If `triadmind verify --strict --json` fails, stop implementation and fix diagnostics/topology first.

## 4) Common commands

- `triadmind sync --force`
- `triadmind runtime --visualize --view full`
- `triadmind coverage --json`
- `triadmind view-map --json`
- `triadmind plan --no-open --view architecture`
- `triadmind apply`
- `triadmind verify --strict --json`
- `triadmind govern ci --policy .triadmind/govern-policy.json --json`
