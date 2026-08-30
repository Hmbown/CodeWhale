# `codewhale-cloud-agent` — Daytona Computer snapshot

This directory is the product-owned definition of the Daytona snapshot that a
Cloud Agent acquires as its Computer (PRODUCT_PRD §4.5). The Codewhale Engine
is the sole runtime inside the Computer and is installed as a commit- and
digest-pinned Linux binary; nothing else in the image runs agent logic.

## What the image is

- Base: `debian:bookworm-slim` (linux/amd64 — Daytona builds amd64 only).
- Engine: released `codewhale-linux-x64` from GitHub release `v0.9.11`,
  fetched by exact URL and verified against the release checksum before
  install:
  - commit `96d13a0bc3f40280ea3865280ad5ccf0e2845e6f` (tag `v0.9.11`)
  - sha256 `c02969556e51e138afa3fe9c97a1359878cd3d1986b1ce1f5fa96c93c6909416`
  - static musl build (no glibc floor), installed at `/usr/local/bin/codewhale`
    with a `codew` symlink; the build fails if `codewhale --version` does not
    print `codewhale 0.9.11 (96d13a0bc3f4)`.
- Toolchain for agent work: git, curl, CA roots, ripgrep, procps, python3
  (+pip, venv), build-essential, pkg-config, jq, unzip, xz-utils, less,
  Node.js 22 (NodeSource). No sudo.
- User: non-root `agent` (uid/gid 1000), `HOME=/home/agent`,
  `CODEWHALE_HOME=/home/agent/.codewhale`. `/work` and `/workspace` exist and
  are owned by `agent`.
- Entrypoint: `sleep infinity` (Daytona injects its own toolbox daemon).
- No secrets are baked in. Provider keys and the account machine token arrive
  only through sandbox create-time env injection.

The pins are recorded as OCI labels (`org.opencontainers.image.revision`,
`net.codewhale.binary.sha256`, ...) so a running Computer can be audited
against the release it claims to run.

## Contract with the dispatcher (`crates/tui/src/cloud_dispatch.rs`, #5712)

- Snapshot name: `codewhale-cloud-agent` (`DEFAULT_CLOUD_AGENT_SNAPSHOT`);
  override with `CODEWHALE_DISPATCH_SNAPSHOT`.
- The runner clones the repository into `/workspace` (`SANDBOX_WORKSPACE`)
  and runs `codewhale exec --auto "<prompt>"` there through the Daytona
  toolbox `process/execute` endpoint.
- Sandbox resources are bound to the snapshot; `daytona create` passes none.

## Provider credentials inside the Computer

The dispatcher injects only `CODEWHALE_API_KEY` (account machine token). The
engine resolves model-provider keys from ambient env, so an end-to-end run
must also inject provider env at create time, never on a command line:

| Provider (config name)     | Env var                                | Model ids seen live          |
|----------------------------|----------------------------------------|------------------------------|
| `modelstudio-token-plan`   | `MODELSTUDIO_API_KEY` (or `DASHSCOPE_API_KEY`) | `qwen3.8-flash`, `deepseek-v4-pro` |
| `deepseek`                 | `DEEPSEEK_API_KEY`                     | `deepseek-v4-pro`            |

Leave `base_url` unset for both providers: a custom `base_url` disables
ambient-env key lookup. Select the route with `exec --provider ... --model ...`
or `CODEWHALE_PROVIDER` / `CODEWHALE_MODEL`.

## Build

`daytona snapshot create` (CLI v0.205.x) has no `--build-arg`, so every pin is
inline in the Dockerfile. Resources are set at snapshot creation and are the
plan maximum:

```sh
cd computer/snapshots/cloud-agent
daytona snapshot create codewhale-cloud-agent -f Dockerfile --cpu 4 --memory 8 --disk 10
```

To roll the engine forward: bump `CODEWHALE_VERSION`, `CODEWHALE_COMMIT`,
`CODEWHALE_ASSET_URL`, `CODEWHALE_ASSET_SHA256`, the `grep -qx` version
assertion, and the OCI labels together, then rebuild under a new snapshot
name (snapshots are immutable once active).

## Probe a Computer

```sh
daytona create --snapshot codewhale-cloud-agent \
  -l owner=cw-integrator -l lane=cloud-agent-e2e --ttl 30 --auto-delete 0 --name cw-probe
daytona exec cw-probe -- sh -c 'id -u; codewhale --version; git --version; node --version; df -h /; sha256sum /usr/local/bin/codewhale'
daytona delete cw-probe
```

The sha256 printed by the probe must equal the pinned
`c02969556e51e138afa3fe9c97a1359878cd3d1986b1ce1f5fa96c93c6909416`.

## Verified build (2026-08-30)

Built with the command above; snapshot id `b9275f82-0ead-4855-9707-21859aa186b4`,
state ACTIVE, 0.70 GB, cpu 4 / memory 8 / disk 10. Probe sandbox
(`daytona create --snapshot codewhale-cloud-agent`, labels
`owner=cw-integrator,lane=cloud-agent-e2e`, ttl 30, auto-delete 0) reported:

```
uid=1000 user=agent HOME=/home/agent CODEWHALE_HOME=/home/agent/.codewhale PWD=/work
codewhale 0.9.11 (96d13a0bc3f4)
git version 2.39.5
v22.23.2            (node)
Python 3.11.2
ripgrep 13.0.0
overlay 10G used 24K avail 10G   (/ , /work, /workspace)
cpu.max 400000 100000 ; memory.max 8589934592
c02969556e51e138afa3fe9c97a1359878cd3d1986b1ce1f5fa96c93c6909416  /usr/local/bin/codewhale
/workspace writable ; /work writable
```

So the Daytona toolbox executes commands as the image `USER` (uid 1000) with
the image `ENV` honored, which is what #5712's `/workspace` clone relies on.
