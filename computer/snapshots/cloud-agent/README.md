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
- No provider credentials are baked in or supplied at sandbox create time.
  Daytona create-time environment is server-visible, so a provider secret
  must never appear in `daytona create -e …` or an SDK `envVars` payload.

The pins are recorded as OCI labels (`org.opencontainers.image.revision`,
`net.codewhale.binary.sha256`, ...) so a running Computer can be audited
against the release it claims to run.

## Current dispatcher state — not a runtime contract

The current product dispatcher wiring for this snapshot is absent.
`crates/tui/src/cloud_dispatch.rs` creates a Daytona sandbox with a generated
name and labels, then records its ID. It does **not** select
`codewhale-cloud-agent`, clone into `/workspace`, inject sandbox environment,
call the Daytona toolbox execution endpoint, or run `codewhale`. It also does
not contain a server-side account-token-to-provider-credential resolution path.

This directory is consequently an image definition and a bounded manual
inspection aid, not an end-to-end Cloud Agent implementation. A snapshot build,
manual `daytona create`, or manual `codewhale exec` proves only the specific
image/operator step observed; none is launch proof for dispatcher, entitlement,
credential custody, Engine execution, lifecycle, metering, or customer use.

## Provider credentials inside the Computer

> **Credential exposure note (verified 2026-08-30).** Daytona persists
> `daytona create -e KEY=VALUE` / SDK `envVars` server-side and returns the
> environment through its API (`GET /sandbox/{id}`). Create-time environment is
> therefore server-visible. Provider secrets must be injected only after the
> Computer is created, through a post-create execution channel from stdin
> (never argv and never create-time environment), then removed at teardown.
> This image and the current dispatcher do not implement that bridge.
>
> `api_key_env` accepts the **name of an environment variable**, not a file
> path or file contents. Do not point it at a `0600` secret file. If a future
> product-owned bridge uses a temporary file, it must separately and explicitly
> map the stdin-delivered secret into the engine process without placing the
> secret in Daytona create-time environment.
>
> The #5712 `CODEWHALE_API_KEY` account/machine-token caveat remains: it is not
> an inference-provider credential and current cloud dispatch does not resolve
> it server-side into one. A machine token alone cannot make this image run a
> provider-backed Engine turn.

The following are configuration references for a future supported post-create
bridge, not current dispatcher wiring and not permission to use create-time
environment:

| Provider (config name)     | Env var                                | Example model identifiers    |
|----------------------------|----------------------------------------|------------------------------|
| `modelstudio-token-plan`   | `MODELSTUDIO_API_KEY` (or `DASHSCOPE_API_KEY`) | `qwen3.8-flash`, `deepseek-v4-pro` |
| `deepseek`                 | `DEEPSEEK_API_KEY`                     | `deepseek-v4-pro`            |

`CODEWHALE_PROVIDER` / `CODEWHALE_MODEL` select an Engine route when the Engine
is launched; they do not make the current dispatcher launch this snapshot or
deliver a provider credential.

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

## Probe a Computer (manual image evidence only)

```sh
daytona create --snapshot codewhale-cloud-agent \
  -l owner=cw-integrator -l lane=cloud-agent-e2e --ttl 30 --auto-delete 0 --name cw-probe
daytona exec cw-probe -- sh -c 'id -u; codewhale --version; git --version; node --version; df -h /; sha256sum /usr/local/bin/codewhale'
daytona delete cw-probe
```

The sha256 printed by the probe must equal the pinned
`c02969556e51e138afa3fe9c97a1359878cd3d1986b1ce1f5fa96c93c6909416`. This is
not product-dispatch or launch acceptance evidence.

## Image build and manual probe receipt (2026-08-30; not launch proof)

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

This shows only that the Daytona toolbox executed the listed manual commands
as the image `USER` (uid 1000) with the image `ENV` honored. It does not prove
current product dispatcher wiring, provider-secret custody, Engine execution,
or any launch acceptance condition.
