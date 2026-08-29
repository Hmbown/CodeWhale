# GitHub App Setup (Codewhale Agent reviews)

`codewhale review --pr N` writes an advisory code review of a pull request. With
`--post` (or from CI) the review is published to GitHub. Published reviews can
appear under two identities:

- the default token the CI job already has (`github.token`), or
- a dedicated **GitHub App** so the review shows as a bot — e.g.
  `codewhale-agent[bot]` — instead of a personal account.

The App identity is optional. Nothing below is needed to run
`codewhale review --pr N` locally and print the report to your terminal.

Related docs:

- [Automatic Workflows](AUTOMATIC_WORKFLOWS.md) — the review workflow in context
- [Providers](PROVIDERS.md) — the model/key used to write the review
- [Receipts](RECEIPTS.md) — how posted reviews are anchored to a head SHA

## The review key is a Codewhale key, not a vendor key

The canonical secret is **`CODEWHALE_API_KEY`**. It is the key for *your
Codewhale account*, and the model behind it is whichever one you configure as
your Codewhale agent for GitHub — it is not tied to any single vendor.

`.github/workflows/codewhale-review.yml` maps `CODEWHALE_API_KEY` into
whatever environment variable the configured provider expects (a `case` over
`CODEWHALE_REVIEW_PROVIDER`), so the secret name never has to change when you
change models.

Bring-your-own-key still works: set the provider's own variable instead and
the workflow uses it directly, with no mapping.

If `CODEWHALE_API_KEY` and a provider's own secret are **both** set, the
canonical account key wins: the workflow maps it onto the chosen provider's
variable, overwriting the BYOK value.

| Secret                | Role                                                        |
|-----------------------|-------------------------------------------------------------|
| `CODEWHALE_API_KEY`   | **canonical** — your Codewhale review key; wins over any BYOK secret that is also set |
| `ZAI_API_KEY`         | BYOK fallback (z.ai Coding Plan / GLM)                       |
| `DEEPSEEK_API_KEY`    | BYOK fallback (DeepSeek). This is the DeepSeek *provider* variable — it is not a generic bot key |
| `OPENROUTER_API_KEY`  | BYOK fallback (OpenRouter)                                   |
| `ANTHROPIC_API_KEY`   | BYOK fallback (Anthropic)                                    |

Any **one** of these is enough. Until at least one exists, the workflow skips
itself with a green notice, so it is safe to merge before setup is finished.

## Choosing which agent reviews

Two repository variables (Settings → Secrets and variables → Actions →
*Variables*) pick the route:

| Variable                     | Example    | Effect                                        |
|------------------------------|------------|-----------------------------------------------|
| `CODEWHALE_REVIEW_PROVIDER`  | `zai`      | passed through as `codewhale review --provider zai` |
| `CODEWHALE_REVIEW_MODEL`     | `GLM-5.3`  | passed through as `--model GLM-5.3`           |

Both are optional. With neither set, the provider is inferred from which key is
present (`CODEWHALE_API_KEY` alone defaults to the z.ai Coding Plan route) and
the model is that provider's default — currently `GLM-5.3` against
`https://api.z.ai/api/coding/paas/v4`.

`--provider` matters because a model id can be reachable through more than one
configured route. When it is, route resolution refuses to guess:

```
model `glm-5.3` is available from configured provider route(s): openrouter, zai.
Pass `--provider <provider>` with `--model glm-5.3` to choose one explicitly.
```

In CI with exactly one key configured the ambiguity does not arise, but adding
a second key would break the job. Setting `CODEWHALE_REVIEW_PROVIDER` pins the
route so that never happens.

## Output budget (reasoning models)

GLM-5.3 is a reasoning model: it emits `reasoning_content` before any
`content`, and both are charged against `max_tokens`. An undersized cap
therefore produces an **empty** review rather than an error.

The CLI's automatic cap (64K) already leaves plenty of room, so the workflow
sets no override by default. To change it, set repository variable
`CODEWHALE_REVIEW_MAX_OUTPUT_TOKENS`; the workflow exports it as
`CODEWHALE_MAX_OUTPUT_TOKENS` and **rejects values below 8192** for exactly
this reason. The run step also fails the job if the review comes back
zero-length on a zero exit status, rather than reporting a clean review that
never happened.

## One-time setup, five steps

You need owner access to the GitHub repository once. After these five steps
every non-draft pull request gets a Codewhale review posted as the App.

1. **Create the App.** GitHub → *Settings → Developer settings → GitHub Apps →
   New GitHub App*. Name it (e.g. `Codewhale Agent`), set a homepage URL, and
   **uncheck Webhook → Active** — the review is pulled on PR events by Actions,
   so no webhook is needed.
2. **Grant two repository permissions.**
   - *Pull requests* → **Read & write** (to post the review and inline comments)
   - *Contents* → **Read-only** (to read the diff; read-only is enough — avoid
     write unless you have another reason)
   Choose *Only on this account*, then **Create GitHub App**.
3. **Download the private key.** On the App's page, *Private keys → Generate a
   private key*. Keep the `.pem` file secret; it is the App's credential.
4. **Install the App** on your account (*Install App* on the same page) and
   select the repositories reviews should cover.
5. **Add three repository settings.** GitHub → *Settings → Secrets and
   variables → Actions*:

   | Kind     | Name                        | Value                               |
   |----------|-----------------------------|-------------------------------------|
   | Variable | `CODEWHALE_APP_ID`          | the App ID shown on the App's page  |
   | Secret   | `CODEWHALE_APP_PRIVATE_KEY` | the full `.pem` file contents       |
   | Secret   | `CODEWHALE_API_KEY`         | your Codewhale review key (or a BYOK provider key from the table above) |

   The review key is the only required one. Optional: variables
   `CODEWHALE_REVIEW_PROVIDER`, `CODEWHALE_REVIEW_MODEL`, and
   `CODEWHALE_REVIEW_MAX_OUTPUT_TOKENS`.

## How the pieces connect

`.github/workflows/codewhale-review.yml` runs on every non-draft PR. When
`CODEWHALE_APP_ID` **and** `CODEWHALE_APP_PRIVATE_KEY` are both present, the
job mints a short-lived installation token for the App
(`actions/create-github-app-token`) and hands it to the CLI as `GH_TOKEN`.
Otherwise it falls back to the workflow's own `github.token`. The CLI never
stores the token; each run mints a fresh one.

The key-presence test lives in the job's `env:` block rather than its `if:`
because the `secrets` context is not available in a job-level `if:`. Job-level
`env` can read `secrets`, and step-level `if:` can read `env`, so every step
gates on the non-secret string `env.HAS_ANY_KEY`. Only booleans about presence
live at job scope; the key values are injected into the one step that runs the
review.

The review itself is one **COMMENT** review — a summary body plus inline line
comments anchored to the PR head SHA. It never approves or requests changes;
CODEOWNERS stays the human authority.

## Running a review yourself

```sh
# print a report locally (uses your configured provider key)
codewhale review --pr 1234

# pin the route when a model is reachable through more than one provider
codewhale review --pr 1234 --provider zai --model GLM-5.3

# publish it to GitHub as whichever identity GH_TOKEN carries
codewhale review --pr 1234 --post
```

`GH_TOKEN` may be your `gh` CLI token (posts as you) or an App installation
token (posts as the App). The `--post` flag is always opt-in.

## Troubleshooting

- **Review posts as you, not the bot.** The variable or the private-key secret
  is missing/empty; the job silently falls back to `github.token`. Check both
  names character-for-character.
- **Workflow logs "No Codewhale review key is set — skipping".** Expected until
  `CODEWHALE_API_KEY` (or one of the BYOK provider keys) exists.
- **"available from configured provider route(s): ...".** Two provider keys are
  configured and the model is reachable from both. Set repository variable
  `CODEWHALE_REVIEW_PROVIDER`.
- **Empty review, job green.** A reasoning model spent its whole budget on
  `reasoning_content`. Raise `CODEWHALE_REVIEW_MAX_OUTPUT_TOKENS` (or unset it
  to use the CLI's automatic cap). The workflow now fails instead of passing
  silently in this case.
- **App token step fails.** The `.pem` was regenerated after the secret was
  set — paste the newest key into `CODEWHALE_APP_PRIVATE_KEY` again, and
  confirm the App is actually installed on the repository.
- **Name already taken.** GitHub App names are global; pick another name. The
  bot's display login is `<slug>[bot]`, derived from the name.
