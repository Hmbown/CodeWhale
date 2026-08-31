# Cloud facts (facts/v1)

One signed, versioned channel for facts CodeWhale wants to move faster than a
binary release: model catalog deltas, provider defaults, release truth, and
one-line announcements. The bundled facts inside the binary are always the
floor; cloud facts are a verified, additive overlay that is **off by default**.

Related: [`CATALOG_REFRESH.md`](./CATALOG_REFRESH.md) (layer order),
[`PROVIDERS.md`](./PROVIDERS.md), `PRODUCT_PRD.md` §4.4 / §5 / §6.

---

## Invariants

| Rule | Where it is enforced |
|---|---|
| Bundled facts are the floor. Off / unreachable / rejected / inapplicable payloads leave the binary exactly as it ships. | `codewhale_config::cloud_facts` (verify → scope → overlay), `CatalogCompiler` layer 15 |
| Never a startup dependency. Startup does one bounded disk read; the network fetch is a background task. | `codewhale_cloud_facts::maybe_load_persisted_cache` / `spawn_background_refresh` |
| Off by default. `[cloud_facts].enabled = false`; `CODEWHALE_DISABLE_CLOUD_FACTS=1` beats everything; CI markers suppress the fetch. | `crates/tui/src/config.rs` `CloudFactsConfig`, `codewhale_cloud_facts::fetch_suppressed` |
| Nothing is interpreted before the Ed25519 signature verifies under a key pinned in the binary. | `cloud_facts::verify::verify_envelope` |
| A cloud fact can never change an explicitly configured or session-selected model. Provider defaults apply only when CLI/env/config set nothing. | `codewhale_config::Config` model chain (`cloud_default_model` is the last fallback before the compiled constant) |
| A cloud `base_url` is accepted only when `https` and on the provider's official host family. | `cloud_facts::scope::base_url_allowed` |
| The fetch carries only a fixed `User-Agent` (`CodeWhale/<version> (+cloud-facts)`) and `If-None-Match`. No ids, cookies, or query parameters (PRD §5). | `codewhale_cloud_facts::fetch` |
| Provenance is visible: `/status` shows channel, version, key id, age, and origin — or why bundled facts are in use. | `crates/tui/src/commands/groups/config/status.rs` |

## Envelope and signing

Served at `GET https://codewhale.net/api/facts/v1/<channel>`:

```json
{
  "envelope": 1,
  "channel": "stable",
  "facts_version": 1,
  "schema_version": 1,
  "key_id": "cwf-dogfood-2026-08",
  "alg": "ed25519",
  "applies_to": ">=0.9.0, <1.0.0",
  "published_at": "2026-08-30T18:41:08Z",
  "payload_b64": "<exact signed bytes, std base64>",
  "sig_b64": "<64-byte Ed25519 signature, std base64>",
  "sigs": [],
  "sha256": "<hex SHA-256 of the payload bytes>"
}
```

- Message = `"codewhale-facts/v1\0" || key_id || "\0" || payload_bytes`. The
  key id is inside the signed message, so a signature cannot be re-labelled
  under another key.
- `payload_bytes` are canonical JSON at signing time (sorted keys, no
  whitespace, UTF-8). Clients verify the exact bytes in `payload_b64` and never
  re-canonicalize.
- Outer `channel` / `facts_version` / `applies_to` / `schema_version` are hints;
  the signed payload repeats them and the client rejects any mismatch.
- `sigs` carries extra `{key_id, sig_b64}` pairs during rotation; a client
  accepts if **any** signature verifies under a pinned, active key.

Verification order (each step rejects → bundled facts stay in use): size cap →
envelope shape (`envelope == 1`, `alg == "ed25519"`) → key pinned and active →
Ed25519 → payload parse → outer/inner cross-check → channel → `schema_version`
≤ supported → `applies_to` matches `CARGO_PKG_VERSION` → `facts_version` ≥
highest previously accepted (rollback protection) → `not_after` (48 h grace;
only downgrades to *stale*).

## Payload (`schema_version` 1)

```json
{
  "schema_version": 1, "channel": "stable", "facts_version": 1,
  "published_at": "…", "applies_to": ">=0.9.0, <1.0.0", "not_after": null,
  "models": [
    { "provider": "deepseek", "id": "deepseek-v4-pro", "op": "upsert",
      "context_window": 262144, "max_output": 32768,
      "pricing": { "input_per_m": 0.5, "output_per_m": 1.5 },
      "reasoning": true, "note": "…", "applies_to": ">=0.9.11" },
    { "provider": "deepseek", "id": "old-model", "op": "deprecate",
      "deprecated_at": "2026-09-01", "replacement": "deepseek-v4-pro" },
    { "provider": "deepseek", "id": "gone", "op": "hide" }
  ],
  "provider_defaults": { "deepseek": { "default_model": "deepseek-v4-pro" } },
  "release": { "latest": "0.9.11", "yanked": [], "min_supported": null,
               "notice": null, "release_url": "https://github.com/Hmbown/CodeWhale/releases/tag/v0.9.11" },
  "announcements": [
    { "id": "example", "level": "info", "text": "≤ 200 chars", "url": null,
      "surfaces": ["tui"], "applies_to": "*", "starts_at": null, "expires_at": null }
  ]
}
```

Every section is optional. Unknown fields are preserved for debugging and
never acted on. Per-item `applies_to` (Cargo semver requirement) lets one
payload serve the whole install base; prerelease binaries follow semver rules,
so the `beta` channel must set ranges explicitly.

### Catalog patch semantics (layer 15)

```
 0 bundled < 10 models.dev live < 15 cloud facts < 20 provider /v1/models < 30 config.toml < 40 user overrides < policy DENY
```

- `upsert`: only the fields the patch sets shadow the row; a patch for a
  missing row is materialized only when it carries `context_window`.
- `deprecate`: annotates (marker in `reasoning_options`); never removes.
- `hide`: removes the row only if it came from bundled / live models.dev.
  Provider-live, config, and user rows can never be hidden.

Row provenance is `CatalogSource::CloudFacts { facts_version, key_id, fetched_at }`
(pickers show `cloud v<N>`; pricing provenance `cloud_facts`).

## Client behaviour

| Concern | Behaviour |
|---|---|
| Config | `[cloud_facts] enabled=false, channel="stable", ttl_hours=6, url=<optional>` |
| Env | `CODEWHALE_CLOUD_FACTS=1|0`, `CODEWHALE_DISABLE_CLOUD_FACTS=1` (hard kill), `CODEWHALE_CLOUD_FACTS_URL` (`{channel}` placeholder), `CODEWHALE_CLOUD_FACTS_CHANNEL`, `CODEWHALE_CLOUD_FACTS_PATH` (local envelope, no network) |
| Cache | `$CODEWHALE_HOME/facts/cloud-facts.json` — raw envelope, ETag, `fetched_at`, `highest_seen_version`, backoff. Re-verified on every load; a tampered file is deleted. |
| Refresh | Background, every `ttl_hours`, `If-None-Match` → 304 keeps the overlay. Failures keep prior verified facts and back off `min(ttl, 10 min · 2^n)`. `404` = "no facts", not an error. |
| Manual | `/model refresh` also forces the cloud facts fetch when enabled. |
| Inert | With no `Active` key in `TRUSTED_KEYS`, the layer is inert even when enabled (`/status`: `inert (no trusted keys; bundled)`). |

### `/status` vocabulary

```
Catalog:        models.dev live · 1234 offerings · fetched 3h ago
Cloud facts:    off (bundled)
                inert (no trusted keys; bundled)
                enabled, none verified yet (bundled)
                stable v1 · verified cwf-dogfood-2026-08 · fetched 12m ago (network) · 0 patches, 0 defaults, 1 notice
                stable v1 · verified … · fetched 3d ago (disk cache) · stale · …
                rejected: bad signature (2m ago; bundled in use)
                not applicable to this build (>=1.0.0; bundled in use)
                fetch failed 5m ago (HTTP 503); keeping v1
```

## Delivery (website)

`web/app/api/facts/v1/[channel]/route.ts` (GET/HEAD) reads
`public.facts_current` from the Supabase project **CodeWhale Web** over
PostgREST with the **publishable (anon) key only**, rebuilds the envelope,
recomputes the payload SHA-256 (mismatch → `502 facts-digest-mismatch`),
verifies the signature against `web/lib/cloud-facts/keys.ts`
(failure → `503 facts-unverifiable`; the edge never amplifies a bad row), then
serves it with:

```
ETag: "<channel>-v<facts_version>-<sha256[:16]>"
Cache-Control: public, max-age=300, s-maxage=300, stale-while-revalidate=3600, stale-if-error=604800
Access-Control-Allow-Origin: *
X-Facts-Channel / X-Facts-Version / X-Facts-Source (supabase | kv-stale) / X-Facts-Verified
```

`If-None-Match` → `304`. No cookies, no `Vary`, no query parameters. When the
Cloudflare `CURATED_KV` binding is present the last good envelope per channel
is kept (`facts:cloud:<channel>`) and served as `kv-stale` if Supabase is
unreachable; both down → `503` with `Retry-After: 600`. Unknown channel → `404
{"error":"no-facts"}` (clients treat it as "no facts"). `/api/facts` gains an
additive `cloudFacts` block so `check:deployed-facts` can later assert served ==
published.

Env: `SUPABASE_URL` (wrangler var), `SUPABASE_PUBLISHABLE_KEY` (set by the
founder as a Worker var/secret; the publishable key is the only Supabase key
the website may hold). Unset → `503 facts-unavailable`.

**Hosting note.** codewhale.net deploys to a Cloudflare Worker via OpenNext
(`web/wrangler.jsonc`, `.github/workflows/web.yml`); the PRD says Vercel hosts
the web product. The route is host-agnostic (plain `fetch`, standard cache
headers) so it works on either; nothing here claims "Vercel edge cached".

## Storage (Supabase, project CodeWhale Web)

Migrations `facts_001_cloud_facts`, `facts_002_seed_stable_v1`,
`facts_003_function_search_path` (applied 2026-08-30, `public` schema —
a dedicated `facts` schema needs a Dashboard "exposed schemas" change;
`ALTER TABLE … SET SCHEMA` later is cheap):

- `facts_channel (scope global|org, org_id, slug, …)` — partial unique indexes
  per scope; org rows reserved for the org override.
- `facts_key (key_id, scope, org_id, algorithm, public_key, status)` —
  informational registry; **never a trust root**.
- `facts_release (channel_id, facts_version, …, payload_b64, sig_b64, sigs,
  payload jsonb, payload_sha256 generated, status published|revoked, …)` —
  `unique (channel_id, facts_version)`; a `BEFORE INSERT` trigger enforces
  monotonic versions and payload/channel agreement; a `BEFORE UPDATE` trigger
  makes rows immutable except `status` / revocation / `notes`.
- `facts_current` view (`security_invoker`) — latest published release per
  live channel.
- Privileges: explicit `REVOKE ALL` then `GRANT SELECT` to `anon`,
  `authenticated`; RLS **enabled and forced** with read policies limited to
  `scope = 'global'` (+ `status = 'published'`, channel not retired). No
  client-role write policy exists; writes go through `service_role` from the
  founder's machine only.

Receipts (2026-08-30): anon `SELECT` on `facts_current` returns the stable
release; anon `INSERT`/`UPDATE` privilege = false; `UPDATE payload_b64` raises
`facts_release is immutable…`; re-inserting `facts_version 1` raises
`facts_version 1 must exceed…`; security advisor shows no findings on
`facts_*` after `facts_003`. Pre-existing, **not touched**:
`cwc.cwc_cli_device_authorizations` reports RLS disabled (no anon grants on
`cwc`, so unreachable via the Data API today) — founder decision.

## Authoring and custody

Source of truth: [`docs/cloud-facts/stable.json`](./cloud-facts/stable.json)
(human-edited; `npm run check:facts` validates it, cross-checks
`release.latest` against `web/data/latest-published-release.json`, and fails
if `keys.rs` and `keys.ts` diverge or the fixtures stop verifying).

```sh
cd web
# 1. generate a key (private half goes OUTSIDE every repo; store in the password manager)
node scripts/facts-publish.mjs keygen --key-id cwf-2026-09 --out ~/.codewhale-secrets/facts/cwf-2026-09.key
#    → pin public_key_b64 in web/lib/cloud-facts/keys.ts and the bytes in
#      crates/config/src/cloud_facts/keys.rs, ship a release (two-release rule).
# 2. sign the source
CODEWHALE_FACTS_SIGNING_KEY_FILE=~/.codewhale-secrets/facts/cwf-2026-09.key \
  node scripts/facts-publish.mjs sign --source ../docs/cloud-facts/stable.json \
       --channel stable --key-id cwf-2026-09 --facts-version 2 --out /tmp/envelope.json
node scripts/facts-publish.mjs verify /tmp/envelope.json
# 3a. publish via PostgREST (service-role key from env; refuses under CI)
SUPABASE_URL=… SUPABASE_SERVICE_ROLE_KEY=… node scripts/facts-publish.mjs publish /tmp/envelope.json --published-by you
# 3b. or emit SQL for psql / the Supabase MCP `apply_migration`
node scripts/facts-publish.mjs emit-sql /tmp/envelope.json > seed.sql
# revoke
node scripts/facts-publish.mjs revoke --channel stable --version 2 --reason "…"
```

Bump `facts_version` in `stable.json` for every publish (the database refuses
non-monotonic versions). Revocation is eventual: ≤ 5 min at the edge plus the
client TTL (6 h) — and a client only ever moves forward, so ship a corrected
higher version rather than relying on revocation alone.

### The dogfood key

`cwf-dogfood-2026-08` was generated on 2026-08-30 in the founder lane so the
channel could be proven end to end. Its private key was written **only** to
the session scratchpad
(`…/scratchpad/cloud-facts-keys/cwf-dogfood-2026-08.key`, mode 0600) for the
founder to move into custody. Treat it as a dogfood key: before customer-facing
use, generate a custody key, pin it, ship, sign with both, retire the dogfood
key (`status: Retired` in both key tables, `facts_key.status = 'retired'`),
then drop it.

`cwf-test-only` (`docs/cloud-facts/fixtures/`) is a deliberately public
keypair used by Rust and web tests. `check:facts` fails if it is ever pinned.

### Rotation and compromise

- Rotation: pin new key → ship → sign with both (`sigs`) → retire old key →
  ship → drop. Old binaries keep verifying via the primary/extra signature they
  trust.
- Compromise: mark the key `compromised`, revoke every release signed by it,
  ship a binary without the key. There is deliberately no in-band "distrust
  this key" message — trust only shrinks through a release.

## Versioning axes

1. Transport: `/api/facts/v1/` + `envelope: 1`. Breaking → `v2` route + new
   `DOMAIN`.
2. `schema_version` inside the signed payload: additive changes keep `1`;
   breaking changes bump it and older clients reject `SchemaTooNew` → bundled.
3. `facts_version`: monotonic per channel (DB trigger + client rollback check;
   ETag derives from it).
4. `applies_to`: semver ranges, top-level and per item.

Cache file `schema_version` 1; incompatible caches are discarded.

## Org override (schema-ready, not built)

`facts_channel.scope = 'org'` / `facts_key.scope = 'org'` with `org_id` and a
reserved authenticated-only RLS policy (org claim / membership predicate);
delivery `/api/facts/v1/org/<slug>/<channel>` or any self-hosted HTTPS/file
endpoint returning an envelope; client `[cloud_facts.org] url, key_id,
public_key, channel` trusting the org key only for that URL, merged at layer 17
(above CodeWhale cloud, below provider-live/config/user); `/status` shows the
chain. Same `facts-publish.mjs` with `--channel/--key-file/--endpoint`.

## Deferred (slice 2)

Founder custody key + retiring the dogfood key; admin verifying-relay route
and cron KV prewarm; pinned immutable `/v1/<channel>/<facts_version>`;
release-check integration (`StartupVersionCheckSource::CloudFacts`, yanked
notice copy); announcement rendering (max 2 per session, deduped by id) and
picker "cloud" chips; `getFactsWithProvenance` cloud source; `codewhale facts`
CLI; consuming cloud `base_url` defaults (types and allowlist exist; only
`default_model` is consumed today); the org override.
