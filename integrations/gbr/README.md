# Pair a phone with Build Remote Agent

Codewhale can use **Build Remote Agent** as a pairing device: the iOS/Android
app spectates (and can inject into) this desktop session through the free MIT
`gbr-agent`. Phone and PC never open ports to each other.

Website: https://grokbuildremote.com/
Agent: https://github.com/LinespottingOrg/GrokBuildRemote-Agents (MIT)
Protocol: `gbr/1` · need agent **v0.6.0+**

Independent product. Not affiliated with xAI or SpaceX.

This bundle does **not** replace Codewhale Tailscale Serve, Funnel, or the
Telegram/Feishu/Weixin bridges. Pairing stays `gbr-agent pair` + `gbr-agent
run`. Attach is only `http://127.0.0.1:8788` or `gbr-mcp` stdio. Phone is
spectator + veto.

## Install + pair

```bash
# macOS / Linux
curl -fsSL https://grokbuildremote.com/install.sh | bash
gbr-agent version          # must print v0.6.0 or newer
gbr-agent pair             # QR in browser + printed 8-char code
gbr-agent run              # leave running
```

```powershell
# Windows
irm https://grokbuildremote.com/install.ps1 | iex
gbr-agent version
gbr-agent pair
gbr-agent run
```

Phone: open Build Remote Agent → **Scan QR from computer** (or type the 8-char
code). **Unpair** in Settings before changing PCs. Force-close is not enough.

## Enable this bundle

1. Clone and install `gbr-mcp`:

```bash
git clone https://github.com/LinespottingOrg/GrokBuildRemote-Agents.git
cd GrokBuildRemote-Agents/mcp/gbr-mcp && npm install
```

2. Point `mcp.json` `args` at that `bin/gbr-mcp.js` path.

3. Install the bundle (review + enable as usual):

```text
/plugin install ./integrations/gbr
```

or copy this directory to `~/.codewhale/plugins/gbr/`.

## Attach

```bash
curl -sS http://127.0.0.1:8788/health
curl -sS http://127.0.0.1:8788/v1/sessions
node …/gbr-mcp/bin/gbr-mcp.js --diagnose
```

Do not commit mailbox keys. Phone **Settings → Bot API** is the only place the
relay key is copied.
