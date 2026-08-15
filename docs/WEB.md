# Local browser client

`codewhale web` opens Codewhale's embedded browser client over the canonical
Runtime API. It is a local surface: the server always binds to
`127.0.0.1`, cannot be rebound to a LAN address, and cannot run with Runtime
authentication disabled.

## Start it

From the workspace Codewhale should operate in, run:

```bash
codewhale web
```

The default address is `http://127.0.0.1:7878`. To avoid a local port
collision, choose another loopback port:

```bash
codewhale web --port 8788
```

Codewhale starts the Runtime API, serves the dependency-free client embedded
in the installed binary, and asks the operating system to open the launch URL
in the default browser. Stop the process with `Ctrl+C`; the browser session
ends with it.

## What the browser can do

The embedded client provides a responsive thread and search rail, Runtime-owned
session facts, transcript and tool receipts, and a composer. It can create,
select, rename, and archive threads; start or steer turns; interrupt work;
resolve approvals; and answer Runtime user-input requests.

The browser is another view of the same local Runtime. It does not create a
second cloud account, copy provider credentials into browser storage, or
weaken the configured approval and sandbox policies.

## Authentication boundary

The browser-launch URL contains a random, ten-minute, one-time bootstrap
capability. It never contains the Runtime bearer token. A loopback request
exchanges the capability for an `HttpOnly`, `SameSite=Strict`, process-local
session cookie and immediately invalidates the capability.

Reused, expired, malformed, and non-loopback bootstrap attempts fail closed.
The Runtime token is not placed in rendered HTML, browser storage, URL
queries or fragments, or browser-launch arguments. The one-time bootstrap
value briefly passes through the operating system's browser launcher; it is
single-use and short-lived, but a hostile process already running as the same
OS user remains inside the local trust boundary.

Cookie-authenticated state-changing requests must also present the exact
local web origin. Cross-origin browser requests are rejected. Existing
explicit bearer and Runtime-token-header clients retain their normal Runtime
API behavior.

## Local means local

`codewhale web` accepts only `--port`; there is no `--host` or insecure-auth
option on this command. Do not treat it as a public website or expose its port
directly through router forwarding, a public reverse proxy, or a tunnel.

The separate `codewhale app-server --mobile` and `--http` modes have different
deployment and authentication contracts. Read [RUNTIME_API.md](RUNTIME_API.md)
before operating either one, especially before selecting a non-loopback bind.

## Troubleshooting

- If port `7878` is occupied, pass an unused `--port` value.
- If the browser cannot be opened, the Runtime keeps serving and prints a
  single-use bootstrap URL (valid for ten minutes) for manual recovery. Open
  that URL in any local browser; do not share it. Check the operating system's
  default-browser setup if you want auto-open next time.
- If the page loads but a provider is unavailable, inspect `codewhale doctor`
  and `/provider`; the web command does not configure or move provider
  credentials.
- If a session expired, stop and restart `codewhale web` to mint a new
  process-local session. Reusing an old bootstrap URL is expected to fail.

For integration endpoints, headers, events, and the complete web-session
contract, see [RUNTIME_API.md](RUNTIME_API.md).
