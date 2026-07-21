# codewhale-web

Documentation and community site for [Codewhale](https://github.com/Hmbown/CodeWhale) — lives at **codewhale.net**.

Next.js 15 (App Router) + Tailwind, deployed to Cloudflare Workers via [`@opennextjs/cloudflare`](https://opennext.js.org/cloudflare). Curated "Today's Dispatch" content is regenerated every 6 hours by a Cloudflare Cron Trigger that calls `deepseek-v4-flash` to summarise recent repo activity, and stored in Workers KV.

## Local dev

```bash
cd web
npm install
cp .env.example .env.local   # fill in the keys you have
npm run dev                  # http://localhost:3000
```

Env (mirrors `.env.example`):

| Variable                    | What                                                             | Required?            |
| --------------------------- | ---------------------------------------------------------------- | -------------------- |
| `DEEPSEEK_API_KEY`          | DeepSeek platform key (`sk-...`)                                 | only for the `/api/cron` tasks (summarization + community agent) |
| `GITHUB_TOKEN`              | Fine-grained PAT, public-repo read scope                         | optional (raises rate limit 60 → 5000 req/h) |
| `GITHUB_REPO`               | Defaults to `Hmbown/CodeWhale`                                   | optional             |
| `CRON_SECRET`               | Shared secret for manual `/api/cron` invocation                  | optional (Cloudflare cron triggers don't need it) |
| `DEEPSEEK_MODEL`            | Defaults to `deepseek-v4-flash`                                  | optional             |
| `DEEPSEEK_BASE_URL`         | Defaults to `https://api.deepseek.com`                           | optional             |
| `MAINTAINER_TOKEN`          | Admin panel auth; access `/admin?token=<value>`                  | only for `/admin`    |
| `MAINTAINER_GITHUB_PAT`     | PAT with `issues:write`, for posting comments via `/admin`       | only for `/admin` posting |
| `NEXT_PUBLIC_GITEE_ENABLED` | Set to `1` once the Gitee mirror exists; blank hides Gitee links | optional             |

The site renders fine without any of them — `Today's Dispatch` falls back to a static editorial; the GitHub feed shows "feed not yet loaded".

## Deploy to Cloudflare

Ordinary pushes and pull requests run the web checks and production build, but
they do **not** deploy. The `deploy` job in `.github/workflows/web.yml` runs
only for a maintainer-triggered `workflow_dispatch` on `main`. Before approval,
record the exact 40-character `origin/main` SHA and trigger that ref:

```bash
git fetch origin main
git rev-parse origin/main
gh workflow run web.yml --repo Hmbown/CodeWhale --ref main
```

The manual job records the pre-deploy source drift, builds the OpenNext bundle,
deploys only after the protected Cloudflare inputs pass, and then requires the
public `/api/facts` receipt to report the exact workflow SHA. A credential-free
local comparison is available without starting a deployment:

```bash
npm run compare:deployed-facts -- --expected-revision <exact-40-character-sha>
```

You already own `codewhale.net` on Cloudflare and have a Workers Paid plan. The deploy is two steps:

1. **Provision KV namespaces once:**

   ```bash
   npx wrangler kv namespace create CURATED_KV
   npx wrangler kv namespace create NEXT_INC_CACHE_KV
   ```

   Copy the printed `id` values into the matching `wrangler.jsonc` bindings
   (replace each `REPLACE_WITH_KV_ID`).

2. **Set secrets and deploy:**

   ```bash
   npx wrangler secret put DEEPSEEK_API_KEY
   npx wrangler secret put GITHUB_TOKEN     # optional
   npx wrangler secret put CRON_SECRET      # optional, for manual /api/cron?task=curate hits

   npm run deploy                           # builds with OpenNext + uploads
   ```

3. **Point the domain:** in the Cloudflare dashboard, add a Worker route for `codewhale.net/*` → the deployed Worker, named `codewhale-web` (see `wrangler.jsonc`).

The first cron run happens within 6 hours; you can also kick it manually:

```bash
curl -H "x-cron-secret: $CRON_SECRET" "https://codewhale.net/api/cron?task=curate"
```

## What's where

Pages are bilingual: each `app/[locale]/` page renders both English and
Chinese from the same file, keyed by the `[locale]` segment (`en` / `zh`,
see `lib/i18n/config.ts`). Copy changes must update both locales.

```
web/
├── app/
│   ├── globals.css             ocean portal, docs layout, type, and shared surfaces
│   ├── [locale]/               en / zh — every page is bilingual
│   │   ├── layout.tsx          root + locale layout: html shell, fonts, nav, footer
│   │   ├── page.tsx            home — hero, dispatch, stats, how-it-works, join
│   │   ├── install/page.tsx    per-OS install with auto-detection
│   │   ├── docs/page.tsx       modes / tools / approval / config / mcp / providers
│   │   ├── faq/page.tsx        frequently asked questions
│   │   ├── feed/page.tsx       live mirror of issues + PRs
│   │   ├── roadmap/page.tsx    shipped / underway / considered / ruled out
│   │   ├── contribute/page.tsx how to PR + house rules + dev loop
│   │   └── admin/              maintainer panel (page.tsx + admin-client.tsx)
│   └── api/
│       ├── cron/route.ts          cron tasks: curate, triage, facts-drift, …
│       ├── facts/route.ts         public source/deployment receipt
│       ├── github/feed/route.ts   cached JSON endpoint
│       └── admin/                 login, logout, post (MAINTAINER_TOKEN-gated)
├── data/
│   └── latest-published-release.json  manually advanced only after publication
├── components/
│   ├── nav.tsx                 sticky header w/ date strip + CJK accents
│   ├── footer.tsx              dense 5-column footer
│   ├── whale.tsx               shared Codewhale mark
│   ├── ticker.tsx              animated live activity strip
│   ├── stat-grid.tsx           tabular repo stats row
│   ├── feed-card.tsx           one issue/PR card
│   ├── locale-switcher.tsx     EN ↔ ZH toggle
│   └── install-*.tsx           install page blocks (binary, code block, tiles)
├── lib/
│   ├── types.ts                shared types
│   ├── i18n/                   locale config, en/zh dictionaries
│   ├── github.ts               REST client + relative-time formatter
│   ├── deepseek.ts             v4-flash chat client + curate() prompt
│   ├── facts.ts                getFacts(): KV value, else build-time FACTS
│   ├── facts.generated.ts      GENERATED — do not edit by hand
│   ├── facts-drift.ts          runtime re-derivation for the drift cron
│   ├── community-agent.ts      triage / pr-review / digest cron tasks
│   └── kv.ts                   Cloudflare KV access via OpenNext bindings
├── scripts/
│   ├── derive-facts.mjs        prebuild: repo sources → lib/facts.generated.ts
│   ├── compare-deployed-facts.mjs credential-free exact-SHA receipt check
│   └── check-kv-id.mjs         predeploy guard for KV namespace ids
├── wrangler.jsonc              CF Worker config + cron + KV binding
├── open-next.config.ts         OpenNext adapter config
└── tailwind.config.ts          design tokens
```

## Facts pipeline

Mechanical facts (version, provider list, sandbox backends, crate names,
default model, Node engines) are never hand-written into pages:

1. **Build time** — `scripts/derive-facts.mjs` runs as `prebuild` (and before
   `npm run dev`), parses the parent repo (`Cargo.toml`, `crates/tui/src/config.rs`,
   `crates/tui/src/sandbox/`, `npm/codewhale/package.json`) and writes
   `lib/facts.generated.ts`. Never edit that file by hand.
2. **Published release** — `data/latest-published-release.json` records the
   latest GitHub Release separately from the source candidate. Install commands
   use this published tag; they never turn the workspace version into a release
   before publication. The credential-free deployed-facts comparison checks the
   record against the public receipt.
3. **Runtime** — the `/api/cron?task=facts-drift` cron (`lib/facts-drift.ts`)
   resolves an exact `main` revision, derives every source fact from that SHA,
   and writes changes to `CURATED_KV` under `facts:current`. Pages accept that
   snapshot only when its source provenance is the same as or newer than the
   deployed build. Legacy, malformed, or older KV data cannot replace newer
   build facts; published-release metadata is resolved independently. Public
   fact pages revalidate their cached HTML every five minutes.

`/api/facts` exposes only public provenance and counts: deployed/resolved source
revision, version, provider count, tool count, selection reason, and latest
published release. It contains no environment values, tokens, or KV contents.

When a new `ApiProvider` variant lands in `crates/tui/src/config.rs`, it must
be added to the `labelMap` in **both** `scripts/derive-facts.mjs` and
`lib/facts-drift.ts` (or to the `EXCLUDED` set if deliberately hidden). Both
fail loudly on unmapped variants, so the build / cron will tell you.

## Visual direction

The public site is a documentation portal with a restrained underwater atmosphere. Content and navigation come first; ocean depth, currents, and the whale mark provide identity without turning every section into a themed card.

- **Palette**: cool paper and mist for reading surfaces, deep navy for terminal and community sections, muted current blue for links, and small gold/coral signals where status needs contrast.
- **Type**: Space Grotesk for headings, IBM Plex Sans for body copy, and JetBrains Mono for commands and compact interface labels.
- **Structure**: compact documentation rows, quiet hairline dividers, generous but bounded reading widths, and responsive layouts that remove chrome before content.

If you want to retune the palette, edit `:root` in `app/globals.css` and the `colors` block in `tailwind.config.ts`.
