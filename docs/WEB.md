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
in the installed binary, prints a single-use launch URL, and asks the operating
system to open that URL in the default browser. If the browser does not open,
use the printed URL within ten minutes. Stop the process with `Ctrl+C`; the
browser session ends with it.

## What the browser can do

The rail brand is a **temporary C-whale**. Sky-blue on Ocean is a stand-in,
not final identity. Replace `crates/tui/src/runtime_web/codewhale-mark-*.png`
when a real mark exists.

The embedded client provides a responsive thread and search rail, Runtime-owned
session facts, transcript and tool receipts, and a composer. It can create,
select, rename, and archive threads; start or steer turns; interrupt work;
resolve approvals; and answer Runtime user-input requests.

The browser is another view of the same local Runtime. It does not create a
second cloud account, copy provider credentials into browser storage, or
weaken the configured approval and sandbox policies.

## Authentication boundary

The browser-launch URL contains a random, short-lived, one-time bootstrap
capability. It never contains the Runtime bearer token. A loopback request
exchanges the capability for an `HttpOnly`, `SameSite=Strict`, process-local
session cookie and immediately invalidates the capability.

Reused, expired, malformed, and non-loopback bootstrap attempts fail closed.
The Runtime token is not placed in rendered HTML, browser storage, URL
queries or fragments, or browser-launch arguments. The one-time bootstrap
value is printed in the local terminal and briefly passes through the operating
system's browser launcher. It is single-use and expires after ten minutes, but
a hostile process already running as the same OS user remains inside the local
trust boundary.

Cookie-authenticated state-changing requests must also present the exact
local web origin. Cross-origin browser requests are rejected. Existing
explicit bearer and Runtime-token-header clients retain their normal Runtime
API behavior.

## Local means local

`codewhale web` accepts `--port` and optional `--tailscale`. There is no
`--host` or insecure-auth option on this command. Do not treat it as a public
website or expose its port directly through router forwarding, a public reverse
proxy, or a generic tunnel.

## Tailnet (`--tailscale`)

`codewhale web --tailscale` keeps the HTTP listener on `127.0.0.1` and
publishes the same client onto the current Tailscale tailnet. Reachability is
ACL-gated. This is not Funnel and not ngrok.

This is **not** account remote control. `/rc` pairs a Cloud/account session to
this runtime through your Codewhale identity. `--tailscale` only puts the
loopback web UI on machines already in your tailnet. Offer both: they answer
different trust questions. Opening the tailnet URL in the browser shows a QR
code for the same MagicDNS origin so a phone on that tailnet can scan in.

Preferred path (opt-in Cargo feature `tailscale` on `codewhale-tui` /
`codewhale-cli`): an embedded tsnet node from the official
[`tailscale`](https://docs.rs/tailscale/0.5.0/tailscale/) 0.5.0 crate. The crate
exposes `Device::tcp_listen` and `tailscale::axum::Listener`, and
`Config.requested_hostname` is set to `codewhale`, so MagicDNS is
`codewhale.<tailnet>.ts.net` (`NodeInfo::fqdn`). Auth is
`CODEWHALE_TSNET_AUTHKEY` or `TS_AUTHKEY`. Official 0.5.0 does **not** implement
HTTPS certificates (listed as unsupported in the crate README), so the embedded
listener is HTTP on port 80 over the WireGuard overlay
(`http://codewhale.<tailnet>.ts.net`). The crate also requires
`TS_RS_EXPERIMENT=this_is_unstable_software` (set automatically for this path)
and currently supports Linux (x86_64/ARM64) and macOS ARM64.

Fallback (always compiled; precursor design from PR #5628): if embed is not
compiled, is disabled with `CODEWHALE_TSNET_DISABLE`, or cannot auth, Codewhale
asks the local Tailscale CLI to publish HTTPS:443 to the loopback port:

```bash
codewhale web --tailscale
```

CLI serve uses the **machine** MagicDNS name (`https://<machine>.<tailnet>.ts.net`),
not `codewhale.<tailnet>.ts.net`. Stopping the process turns off only the
HTTPS:443 serve mapping (`tailscale serve --https=443 off`). It does not run
`tailscale serve reset`.

Cookie-authenticated requests must present the advertised tailnet origin
(`*.ts.net`). Bootstrap from a tailnet Host is allowed when that origin was
published; default `codewhale web` without `--tailscale` remains loopback-only.

The separate `codewhale app-server --mobile` and `--http` modes have different
deployment and authentication contracts. Read [RUNTIME_API.md](RUNTIME_API.md)
before operating either one, especially before selecting a non-loopback bind.

## Troubleshooting

- If port `7878` is occupied, pass an unused `--port` value.
- If the browser does not open, copy the printed single-use bootstrap URL into
  a browser on the same machine within ten minutes. Start `codewhale web` again
  if that URL has already been used or expired.
- If the page loads but a provider is unavailable, inspect `codewhale doctor`
  and `/provider`; the web command does not configure or move provider
  credentials.
- If a session expired, stop and restart `codewhale web` to mint a new
  process-local session. Reusing an old bootstrap URL is expected to fail.

For integration endpoints, headers, events, and the complete web-session
contract, see [RUNTIME_API.md](RUNTIME_API.md).
