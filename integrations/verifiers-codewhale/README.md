# Codewhale harness for Verifiers

This local package runs Codewhale v0.9.4 as a Prime Intellect Verifiers v0.2
harness. Verifiers owns the task, rubric, model interception, and rollout
runtime. Codewhale owns the coding-agent loop and its tools.

The adapter is intentionally pre-publication. It is checked in and tested with
Codewhale, but it is not uploaded to PyPI or the Prime Environments Hub.

## What it guarantees

- Every rollout gets an isolated `CODEWHALE_HOME`; ambient Codewhale sessions,
  project config, memory, and credentials are not reused.
- Model traffic is pinned to Verifiers' OpenAI-compatible interception endpoint
  with the per-rollout session secret. The secret is kept in the child
  environment and never placed in argv or receipt metadata.
- Verifiers toolsets are written as a rollout-local MCP config.
- Codewhale runs non-interactively with telemetry disabled. It never runs setup
  and does not require a telemetry key.
- Local subprocess evaluation stays `workspace-write`; Docker, Prime, and Modal
  use their already-isolated runtime as Codewhale's external sandbox. Neither
  path authorizes Codewhale's sandbox-elevation flag.
- Successful runs must end with the exact Codewhale exec-stream v1 terminal
  receipt. A bounded, non-content receipt is copied to
  `trace.info["codewhale"]`; malformed or incomplete streams fail closed.

## Install locally

From the Codewhale checkout:

```bash
uv pip install -e integrations/verifiers-codewhale
```

Then select the package as a Verifiers v1 harness:

```bash
uv run eval <taskset> \
  --harness.id codewhale-harness \
  --harness.version 0.9.1 \
  --harness.runtime.type docker
```

The default setup downloads all three release runtime companions from
the pinned Codewhale tag and verifies each byte against the release checksum
manifest. Before v0.9.1 is published, use an installed candidate for a local
subprocess rollout:

```bash
uv run eval <taskset> \
  --harness.id codewhale-harness \
  --harness.version 0.9.1 \
  --harness.binary-path /absolute/path/to/codewhale \
  --harness.runtime.type subprocess
```

`binary_path` is a path inside the selected runtime. A host path is therefore
appropriate only for the subprocess runtime unless it has also been mounted or
installed into a container/sandbox.

## Authority boundary

The adapter opts into Codewhale's headless auto-tool path so ordinary coding
work can proceed. Explicitly denied tools, protected actions, and sandbox
elevation remain fail-closed. A headless request that genuinely needs human
input must terminate with a typed input-required failure; it must never wait on
an invisible prompt.

No provider or Prime credentials are required by this repository's tests.
