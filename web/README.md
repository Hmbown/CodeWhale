# codewhale-web

Community site for [CodeWhale](https://github.com/Hmbown/CodeWhale) — lives at **codewhale.net**.

Next.js 15 (App Router) + Tailwind, deployed to Cloudflare Workers via [`@opennextjs/cloudflare`](https://opennext.js.org/cloudflare). Curated "Today's Dispatch" content is regenerated every 6 hours by a Cloudflare Cron Trigger that calls `deepseek-v4-flash` to summarise recent repo activity, and stored in Workers KV.

## Local dev

```bash
cd web
npm install
cp .env.example .env.local   # fill in the keys you have
npm run dev                  # http://localhost:3000
```

Required env (only for the curator + private-repo rate limits):

| Variable            | What                                              | Required?            |
| ------------------- | ------------------------------------------------- | -------------------- |
| `DEEPSEEK_API_KEY`  | DeepSeek platform key (`sk-...`)                  | only for `/api/cron?task=curate` |
| `GITHUB_TOKEN`      | Fine-grained PAT, public-repo read scope          | optional (raises rate limit) |
| `GITHUB_REPO`       | Defaults to `Hmbown/CodeWhale`                    | optional             |
| `CRON_SECRET`       | Shared secret for manual cron invocation          | optional             |

The site renders fine without any of them — `Today's Dispatch` falls back to a static editorial; the GitHub feed shows "feed not yet loaded".

## Deploy to Cloudflare

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

3. **Point the domain:** in the Cloudflare dashboard, add a Worker route for `codewhale.net/*` → the deployed Worker (currently named `deepseek-tui-web` unless the Worker is renamed during deploy).

The first cron run happens within 6 hours; you can also kick it manually:

```bash
curl -H "x-cron-secret: $CRON_SECRET" "https://codewhale.net/api/cron?task=curate"
```

## What's where

```
web/
├── app/
│   ├── layout.tsx              root layout, font loading
│   ├── page.tsx                home — hero, dispatch, stats, how-it-works, join
│   ├── globals.css             design system: paper grain, hairlines, type, seal
│   ├── install/page.tsx        per-OS install with auto-detection
│   ├── docs/page.tsx           modes / tools / approval / config / mcp / providers
│   ├── feed/page.tsx           live mirror of issues + PRs
│   ├── roadmap/page.tsx        shipped / underway / considered / ruled out
│   ├── contribute/page.tsx     how to PR + house rules + dev loop
│   └── api/
│       ├── cron/route.ts          manual cron trigger: GitHub → DeepSeek → KV
│       └── github/feed/route.ts   cached JSON endpoint
├── components/
│   ├── nav.tsx                 sticky header w/ date strip + CJK accents
│   ├── footer.tsx              dense 5-column footer
│   ├── seal.tsx                red Chinese-seal mark used as section anchor
│   ├── ticker.tsx              animated live activity strip
│   ├── stat-grid.tsx           tabular repo stats row
│   ├── feed-card.tsx           one issue/PR card
│   └── install-tabs.tsx        client component, OS auto-detect + copy
├── lib/
│   ├── types.ts                shared types
│   ├── github.ts               REST client + relative-time formatter
│   ├── deepseek.ts             v4-flash chat client + curate() prompt
│   └── kv.ts                   Cloudflare KV access via OpenNext bindings
├── wrangler.jsonc              CF Worker config + cron + KV binding
├── open-next.config.ts         OpenNext adapter config
└── tailwind.config.ts          design tokens
```

## Aesthetic

"Yamen tech": Qing memorial document × WeChat news feed × Bloomberg terminal.

- **Palette**: cream paper `#FAF6EE`, ink `#0A2540`, cinnabar red `#C8102E`, aged gold, jade green, cobalt blue.
- **Type**: Fraunces (display), IBM Plex Sans (body), JetBrains Mono (UI/code), Noto Serif SC (decorative CJK anchors).
- **Structure**: hairline 1px dividers, multi-column grids, big tabular numbers, surgical use of red for "hot" markers, decorative Chinese-seal squares as section anchors.

If you want to retune the palette, edit `:root` in `app/globals.css` and the `colors` block in `tailwind.config.ts`.
