# syntax=docker/dockerfile:1
# codewhale-cloud-agent — Daytona snapshot image for Codewhale cloud agents.
#
# Product truth first: this image installs the RELEASED v0.9.11 Linux x86_64
# binary (static musl, no glibc floor) from the GitHub release by exact URL,
# verifies its sha256 against codewhale-artifacts-sha256.txt, and records the
# commit + digest as OCI labels (PRD 4.5: commit- and digest-pinned Linux
# binary inside the Computer). No secrets are baked in. Daytona create-time
# environment is server-visible, so a provider key must never be injected at
# create time. A future dispatcher must deliver provider secrets only after
# creation, over a post-create execution channel from stdin (never argv), and
# remove them during teardown. `CODEWHALE_API_KEY` is an account/machine token,
# not an inference-provider credential; current cloud dispatch has no
# server-side account-token-to-provider-key resolution.
#
# Build (Daytona, amd64 only; daytona snapshot create has no --build-arg, so
# every pin is inline):
#   daytona snapshot create codewhale-cloud-agent \
#     -f Dockerfile --cpu 4 --memory 8 --disk 10   (plan max; resources bind to the snapshot)
#
# Current dispatcher boundary: this is an image definition, not a wired cloud
# execution path. `crates/tui/src/cloud_dispatch.rs` currently creates a
# Daytona sandbox with a name and labels only; it does not select this snapshot,
# clone a repository, inject any sandbox environment, or execute `codewhale`.
# A manual image build or Daytona probe is therefore not Cloud Agent launch
# evidence. Do not add a create-time provider-key shortcut while that product
# wiring is built.

FROM debian:bookworm-slim

ARG DEBIAN_FRONTEND=noninteractive

# ---- pins (release v0.9.11, tag commit 96d13a0bc3f40280ea3865280ad5ccf0e2845e6f)
ENV CODEWHALE_VERSION=0.9.11 \
    CODEWHALE_COMMIT=96d13a0bc3f40280ea3865280ad5ccf0e2845e6f \
    CODEWHALE_ASSET_URL=https://github.com/Hmbown/CodeWhale/releases/download/v0.9.11/codewhale-linux-x64 \
    CODEWHALE_ASSET_SHA256=c02969556e51e138afa3fe9c97a1359878cd3d1986b1ce1f5fa96c93c6909416 \
    NODE_MAJOR=22

# Base toolchain for an agent doing code work: git, curl, TLS roots, ripgrep,
# python3, node LTS (22), build-essential. procps for `ps`/`pkill` used by the
# engine's process tooling; sudo is deliberately NOT installed.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git gnupg ripgrep procps \
      python3 python3-pip python3-venv \
      build-essential pkg-config \
      jq unzip xz-utils less \
 && mkdir -p /etc/apt/keyrings \
 && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
      | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
 && echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_${NODE_MAJOR}.x nodistro main" \
      > /etc/apt/sources.list.d/nodesource.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends nodejs \
 && rm -rf /var/lib/apt/lists/*

# Download the released binary by exact URL and refuse anything whose sha256
# does not match the release checksum. Static musl: no interpreter, no glibc.
RUN set -eu; \
    curl -fsSL --retry 3 -o /tmp/codewhale "${CODEWHALE_ASSET_URL}"; \
    echo "${CODEWHALE_ASSET_SHA256}  /tmp/codewhale" | sha256sum -c -; \
    install -m 0755 -o root -g root /tmp/codewhale /usr/local/bin/codewhale; \
    ln -s /usr/local/bin/codewhale /usr/local/bin/codew; \
    rm -f /tmp/codewhale; \
    codewhale --version | tee /etc/codewhale-version; \
    grep -qx "codewhale 0.9.11 (96d13a0bc3f4)" /etc/codewhale-version

# Non-root agent user. /work is the generic Computer mount; /workspace is the
# path #5712's runner clones into and runs the harness from — both owned by
# the agent so a non-root toolbox user can write them.
RUN groupadd --gid 1000 agent \
 && useradd --uid 1000 --gid 1000 --create-home --home-dir /home/agent --shell /bin/bash agent \
 && mkdir -p /work /workspace /home/agent/.codewhale \
 && chown -R agent:agent /work /workspace /home/agent

ENV HOME=/home/agent \
    CODEWHALE_HOME=/home/agent/.codewhale \
    PATH=/home/agent/.local/bin:/usr/local/bin:/usr/bin:/bin \
    GIT_TERMINAL_PROMPT=0 \
    CI=1 \
    TERM=xterm-256color

LABEL org.opencontainers.image.title="codewhale-cloud-agent" \
      org.opencontainers.image.description="Codewhale cloud agent Computer: released codewhale CLI preinstalled for dispatched turns" \
      org.opencontainers.image.version="0.9.11" \
      org.opencontainers.image.revision="96d13a0bc3f40280ea3865280ad5ccf0e2845e6f" \
      org.opencontainers.image.source="https://github.com/Hmbown/CodeWhale" \
      net.codewhale.binary.asset="codewhale-linux-x64" \
      net.codewhale.binary.sha256="c02969556e51e138afa3fe9c97a1359878cd3d1986b1ce1f5fa96c93c6909416" \
      net.codewhale.binary.commit="96d13a0bc3f40280ea3865280ad5ccf0e2845e6f" \
      net.codewhale.binary.version="0.9.11" \
      net.codewhale.release.image="ghcr.io/hmbown/codewhale:0.9.11@sha256:6de13fe5e62fb3cb815c423bcb17455bef4d9f7db2107888beb88fe4b7c9ac14"

USER agent
WORKDIR /work

# Daytona injects its own toolbox daemon; keep the container alive for it.
ENTRYPOINT ["sleep", "infinity"]
