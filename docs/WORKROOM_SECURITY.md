# Workroom Security Model

## Scope

This document covers the security boundaries of Codewhale Workrooms — the
durable, addressable containers for threaded agent conversations described
in [RFC 3209](rfcs/3209-workrooms.md).

Workrooms do **not** introduce any new network services, cloud dependencies,
or default-on public sharing. Security responsibility stays with the
operator who controls the Runtime API.

This document describes the intended security contract for the v0.9 workroom
surface. In v0.8.62, only protocol data types and link parsing have landed.
Persistent state, Runtime API endpoints, token scoping, event storage, and
model-visible link resolution remain follow-up work.

## Principles

1. **Local-first.** Future persisted workroom state should live under the
   Codewhale home directory, protected by user-only filesystem permissions.
   No cloud sync and no third-party hosting. Workroom content is never a
   telemetry subject: the anonymous usage counting in `docs/TELEMETRY.md`
   collects counts and closed enums only, and no workroom id, title, link, or
   body may ever be added to its schema. That telemetry now has a live ingest
   endpoint (`https://telemetry.codewhale.net/v1/telemetry`, source in
   `telemetry-ingest/`), which makes the rule enforced rather than merely
   stated: the endpoint validates against a **closed** field set and rejects an
   entire batch carrying any key the published schema does not name, so a
   workroom field added by accident is refused at ingest rather than stored.
   Counting is disclosed and can be disabled immediately. It is on by default,
   but workroom content is still structurally absent from every accepted batch.

2. **No secrets in links.** `codewhale://workroom/wr_...` URLs contain only
   opaque UUIDs. They carry no API keys, bearer tokens, passwords, or file
   paths. An adversary with a workroom link can do nothing without Runtime
   API access.

3. **No public read paths.** Future workroom endpoints must require a valid
   bearer token in the `Authorization` header. There should be no
   unauthenticated `/workroom/...` route.

4. **No secrets in events.** `WorkroomEvent` payloads must never contain
   API keys, auth tokens, or plaintext credentials. The `ArtifactLinked`
   event kind references file paths, not contents. Events are intended for
   indexing/reference, not for replaying agent tool output.

5. **Share is explicit.** A workroom is `Private` by default. The operator
   may mark it `Shared` and list allowed bearer tokens. The operator
   controls which tokens are issued, rotated, and revoked.

## Threat model

| Threat | Mitigation |
|---|---|
| Attacker obtains a workroom link | Link contains only opaque UUID; resolution requires Runtime API auth |
| Attacker brute-forces workroom IDs | UUID v4 (`2^122` space); future APIs should add rate limiting before exposing lookup surfaces |
| Attacker injects a malicious event | Future event writes should flow only through trusted Runtime clients |
| Attacker exfiltrates workroom state | Future filesystem state should be gated by OS user permissions and runtime auth |
| Bearer token leaks | Operator rotates tokens; future sharing rules should be revocable without touching workroom state |

## API auth

Future workroom endpoints should inherit the same auth middleware as other
protected routes (`/thread`, `/app`, `/tool`, etc.):

- `Authorization: Bearer <token>` header required
- Token validated against the runtime's configured bearer token(s)
- 401 Unauthorized if missing or invalid

## Future work

| Item | Risk | Status |
|---|---|---|
| Event encryption at rest | In scope for Phase 2 if workrooms move to a multi-user model | Not implemented |
| Audit log for shared workrooms | Useful if shared tokens are used across operators | Not implemented |
| Token scoping (read/write/admin) | Currently all tokens have full access | Not planned |
