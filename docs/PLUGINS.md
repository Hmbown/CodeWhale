# Installing plugins

This is the walkthrough for the `/plugin install` on-ramp (v0.9.4, #5182).
[PLUGIN_BUNDLES.md](PLUGIN_BUNDLES.md) remains the contract for the bundle
format (`plugin.json`, compatible `kimi.plugin.json`, or legacy
`plugin.toml`), discovery, validation, and
the trust/enable lifecycle — this document covers how bits get onto disk in
the first place.

`/plugin suggest <task>` is a local, read-only companion: it ranks already
installed bundles (name, keywords, description, bundled skill names, declared
hosts) and any marketplace catalogs you have added with `/plugin marketplace add`.
It explains the match and gives the next review, enable, or catalog-install
step, but never installs, trusts, or enables a bundle on its own.

Sending a task also surfaces one quiet toast when the prompt strongly matches
an installed-but-idle plugin or a catalog candidate you do not have yet — for
example a prompt about Supabase suggesting `/plugin trust supabase` or
`/plugin marketplace install <catalog> supabase`. Description-only matches do
not toast. While you type, a one-line composer CTA (`Install {name} plugin?`)
offers the same review command after a short debounce; it never auto-installs,
hides when the plugin is already active, and stays dismissed for that name
this session. Matching idle or catalog plugins are also appended on send as
an `<recommended_plugins>` user-turn block (not the pinned system prefix);
the model can call `request_plugin_install` to surface review for the human
without changing disk. Codewhale does not invent a remote plugin URL; missing
plugins are suggested only from catalogs you added. On-disk bundle changes
still toast `/plugin reload` on send and between turns.

## Sources

`/plugin install <spec>` accepts three source kinds:

```text
/plugin install ./path/to/bundle            # local directory (copied)
/plugin install github:owner/repo           # GitHub archive of the default branch
/plugin install https://example.com/x.tar.gz  # direct tarball URL
```

There is no registry index and no `git clone` in v1 — tarball-only fetching
keeps the size cap and no-symlink guarantees of the installer. Downloads are
gated by the per-domain network policy: an unknown host returns a
"needs approval" error naming the host (`/network allow <host>`, then retry),
a denied host aborts without touching disk.

The fetched tree must contain **exactly one** bundle root — a directory
holding a `plugin.json`, compatible `kimi.plugin.json`, or legacy `plugin.toml`
manifest. Kimi bundles are accepted when they use Codewhale-compatible Skills,
commands, agents, and MCP declarations; unsupported Kimi runtime fields fail
closed instead of being silently ignored. Bundles land in
the user plugins root at `~/.codewhale/plugins/<name>/`, where `<name>` is the
manifest's plugin name.

## The guided flow

Installing never activates anything. The command places the bits, then drops
you straight into the standard capability review:

```text
/plugin install github:someone/neat-plugin
→ Installed plugin 'neat-plugin' to ~/.codewhale/plugins/neat-plugin.
  It is disabled and untrusted. Review its requested authority below…
  <full inventory, permissions, MCP authority render>
  /plugin trust neat-plugin <content-hash>.<capability-hash>

/plugin trust neat-plugin <paste the token>   # records the hash-bound receipt
/plugin enable neat-plugin                    # activates for this workspace
```

This is the same review render and confirmation token as `/plugin trust
<name>` — trust is the strict hash-bound receipt flow, not an advisory marker.
If the bundle's content or declared capabilities change, the receipt stops
matching and the plugin goes inactive until you review again.

## Update and uninstall

```text
/plugin update <name>      # re-download, byte-compare, atomic swap if changed
/plugin disable <name>     # required before uninstall
/plugin uninstall <name>   # deletes the bundle and prunes its state entry
```

- `update` re-downloads the recorded source. Identical bytes are a no-op; a
  changed bundle is swapped atomically and its trust receipt is automatically
  invalidated (the hash no longer matches), so re-review is forced before the
  plugin can activate again. Plugins installed from a local path cannot be
  re-downloaded — reinstall them with `/plugin install <path>`.
- `uninstall` refuses enabled plugins (disable first), deletes the bundle
  directory, and removes its persisted trust/enablement entry.

## Safety rules

- Every install carries an `.installed-from` provenance marker. The installer
  **refuses to overwrite or delete** a bundle that lacks it — hand-placed
  bundles under `~/.codewhale/plugins/` are never clobbered.
- Tarballs are size-capped and extracted into a private staging directory
  first; path traversal (`..`, absolute paths) and symlinks/hard links inside
  the bundle are rejected, and the destination only appears via an atomic
  rename after every check passes.
- Install pre-checks the name against builtin and workspace bundles so a
  higher-precedence bundle cannot silently shadow (or be shadowed by) the
  install.
- Newly installed bits are always **disabled and untrusted**; enablement only
  ever happens through the explicit trust review above.
