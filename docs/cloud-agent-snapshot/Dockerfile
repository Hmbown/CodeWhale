# The Codewhale cloud-agent snapshot image.
#
# Founder decision 2026-08-29: every dispatch sandbox ships Codewhale
# itself, so `codewhale exec --auto` exists inside the sandbox and the
# self-hosting loop closes: dispatch -> sandbox (own codewhale) ->
# account identity (CODEWHALE_API_KEY injected at create time) -> forge
# PR. Build this into a Daytona snapshot named
# `codewhale-cloud-agent` (or set CODEWHALE_DISPATCH_SNAPSHOT).
#
# Build inputs are pinned on purpose: the snapshot is an artifact, and an
# artifact must be reproducible from a rev, not from a moving branch.

FROM rust:1.91-slim-bookworm AS build
ARG CODEWHALE_GIT_REV=8cd2006a01d16ce833085c11cbbaa53c18c720b1
RUN apt-get update \
    && apt-get install -y --no-install-recommends git pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# Pin by rev; cargo verifies the tree against Cargo.lock (--locked).
RUN git init . \
    && git remote add origin https://github.com/Hmbown/CodeWhale.git \
    && git fetch --depth 1 origin "${CODEWHALE_GIT_REV}" \
    && git checkout FETCH_HEAD
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p codewhale-cli \
    && cp target/release/codewhale /usr/local/bin/codewhale

FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="codewhale-cloud-agent" \
      org.opencontainers.image.description="Codewhale cloud agent: the codewhale CLI preinstalled for dispatched turns"
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       git ca-certificates curl ripgrep procps \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/codewhale /usr/local/bin/codewhale
# The runner clones the target repository into /workspace and executes
# `codewhale exec --auto` there (SANDBOX_WORKSPACE in cloud_dispatch.rs).
RUN mkdir -p /workspace
RUN codewhale --version
WORKDIR /workspace
