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

   | Kind     | Name                     | Value                                  |
   |----------|--------------------------|----------------------------------------|
   | Variable | `CODEWHALE_APP_ID`       | the App ID shown on the App's page     |
   | Secret   | `CODEWHALE_APP_PRIVATE_KEY` | the full `.pem` file contents       |
   | Secret   | `DEEPSEEK_API_KEY`       | provider key for the review model (or the env var matching your provider; see [Providers](PROVIDERS.md)) |

   `DEEPSEEK_API_KEY` is the only required one. Until it exists, the workflow
   skips itself with a green notice — it is safe to merge the workflow before
   finishing setup. Optional: variable `CODEWHALE_REVIEW_MODEL` overrides the
   model (e.g. `deepseek-chat`).

## How the pieces connect

`.github/workflows/codewhale-review.yml` runs on every non-draft PR. When
`CODEWHALE_APP_ID` **and** `CODEWHALE_APP_PRIVATE_KEY` are both present, the
job mints a short-lived installation token for the App
(`actions/create-github-app-token`) and hands it to the CLI as `GH_TOKEN`.
Otherwise it falls back to the workflow's own `github.token`. The CLI never
stores the token; each run mints a fresh one.

The review itself is one **COMMENT** review — a summary body plus inline line
comments anchored to the PR head SHA. It never approves or requests changes;
CODEOWNERS stays the human authority.

## Running a review yourself

```sh
# print a report locally (uses your configured provider key)
codewhale review --pr 1234

# publish it to GitHub as whichever identity GH_TOKEN carries
codewhale review --pr 1234 --post
```

`GH_TOKEN` may be your `gh` CLI token (posts as you) or an App installation
token (posts as the App). The `--post` flag is always opt-in.

## Troubleshooting

- **Review posts as you, not the bot.** The variable or the private-key secret
  is missing/empty; the job silently falls back to `github.token`. Check both
  names character-for-character.
- **Workflow logs "DEEPSEEK_API_KEY is not set — skipping".** Expected until
  the provider secret exists.
- **App token step fails.** The `.pem` was regenerated after the secret was
  set — paste the newest key into `CODEWHALE_APP_PRIVATE_KEY` again, and
  confirm the App is actually installed on the repository.
- **Name already taken.** GitHub App names are global; pick another name. The
  bot's display login is `<slug>[bot]`, derived from the name.
