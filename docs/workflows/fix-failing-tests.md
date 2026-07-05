# Fix failing tests

Paste the real failure — not a summary of it. CodeWhale is built around
evidence: it may never report a fact its tools did not return, so give it the
actual output to work from.

```text
`npm test` fails with the output below. Find the root cause, fix it, and
re-run the failing test to prove the fix. Do not weaken or delete the test.

<paste the failing test output here>
```

CodeWhale reads your repo's `AGENTS.md` for the project's build/test commands,
so keep the canonical test invocation documented there.

Approve the test-run commands as they come up, or set approval to `auto` in
`/config` for a trusted repo.

**Done when:** the previously failing test passes in a fresh run executed in
your session, and no other tests broke.

See also: [GUIDE.md](../GUIDE.md), [CONFIGURATION.md](../CONFIGURATION.md).
