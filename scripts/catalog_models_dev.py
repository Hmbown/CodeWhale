#!/usr/bin/env python3
"""Models.dev catalog refresh / snapshot automation for CodeWhale (#4117).

Fetches the public Models.dev combined catalog, validates offline bundled seed
shape, and supports OpenRouter public-listing inspection. The automation is
intentionally validate/dry-run only: it never accepts, prints, or persists API
keys / auth headers, and it does not write fetched JSON to disk.

Usage examples:

  # Dry-run: fetch + validate, print counts (no write)
  scripts/catalog_models_dev.py refresh

  # Validate the in-repo offline seed still parses as Models.dev-shaped JSON
  scripts/catalog_models_dev.py snapshot --check \\
      crates/config/assets/models_dev.bundled.json

  # OpenRouter public /models listing (no key), dry-run only
  scripts/catalog_models_dev.py refresh --provider openrouter \\
      --sort newest --limit 100

Environment:
  CODEWHALE_MODELS_DEV_URL   Override Models.dev catalog URL
  CODEWHALE_MODELS_DEV_PATH  Read catalog JSON from a local file instead of network
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

DEFAULT_MODELS_DEV_URL = "https://models.dev/catalog.json"
DEFAULT_OPENROUTER_MODELS_URL = "https://openrouter.ai/api/v1/models"
USER_AGENT = "CodeWhale-catalog-automation/0.8.68 (+https://github.com/Hmbown/CodeWhale)"
FETCH_TIMEOUT_SECS = 60


def die(msg: str, code: int = 1) -> None:
    print(f"error: {msg}", file=sys.stderr)
    raise SystemExit(code)


def load_json_bytes(raw: bytes, source: str) -> Any:
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        die(f"{source}: not utf-8 ({exc})")
    try:
        return json.loads(text)
    except json.JSONDecodeError as exc:
        die(f"{source}: invalid JSON ({exc})")


def fetch_url(url: str) -> bytes:
    req = urllib.request.Request(
        url,
        headers={
            "User-Agent": USER_AGENT,
            "Accept": "application/json",
            # Explicitly no Authorization header — public endpoints only.
        },
        method="GET",
    )
    try:
        with urllib.request.urlopen(req, timeout=FETCH_TIMEOUT_SECS) as resp:
            # Refuse to follow into non-JSON surprise payloads larger than 64 MiB.
            data = resp.read(64 * 1024 * 1024 + 1)
            if len(data) > 64 * 1024 * 1024:
                die(f"{url}: response exceeds 64 MiB safety cap")
            ctype = resp.headers.get("Content-Type", "")
            if "json" not in ctype.lower() and not data.lstrip().startswith((b"{", b"[")):
                die(f"{url}: unexpected Content-Type {ctype!r}")
            return data
    except urllib.error.HTTPError as exc:
        die(f"{url}: HTTP {exc.code} {exc.reason}")
    except urllib.error.URLError as exc:
        die(f"{url}: {exc.reason}")


def load_models_dev_catalog() -> tuple[dict[str, Any], str, bool]:
    """Return (document, source_label, is_local_file).

    Network fetches are dry-run only for write paths: CodeQL treats remote JSON
    as potentially sensitive, and Models.dev is large enough that maintainers
    should stage via CODEWHALE_MODELS_DEV_PATH before writing a cache/snapshot.
    """
    path_override = os.environ.get("CODEWHALE_MODELS_DEV_PATH", "").strip()
    if path_override:
        p = Path(path_override)
        if not p.is_file():
            die(f"CODEWHALE_MODELS_DEV_PATH not a file: {p}")
        raw = p.read_bytes()
        data = load_json_bytes(raw, str(p))
        return ensure_models_dev_shape(data, str(p)), f"file:{p}", True

    url = os.environ.get("CODEWHALE_MODELS_DEV_URL", DEFAULT_MODELS_DEV_URL).strip()
    if not url:
        url = DEFAULT_MODELS_DEV_URL
    raw = fetch_url(url)
    data = load_json_bytes(raw, url)
    return ensure_models_dev_shape(data, url), f"url:{url}", False


def ensure_models_dev_shape(data: Any, source: str) -> dict[str, Any]:
    if not isinstance(data, dict):
        die(f"{source}: expected object root")
    # Allow optional _meta (CodeWhale offline seed) and require models+providers
    # when present so we never write a partial secret leak document.
    models = data.get("models")
    providers = data.get("providers")
    if models is None and providers is None:
        die(f"{source}: missing both 'models' and 'providers'")
    if models is not None and not isinstance(models, dict):
        die(f"{source}: 'models' must be an object")
    if providers is not None and not isinstance(providers, dict):
        die(f"{source}: 'providers' must be an object")
    # Rebuild a public document from allowlisted top-level keys only so we never
    # persist credential-shaped fields even if a future Models.dev field adds them.
    return public_models_dev_document(data)


def is_credential_key(key: str) -> bool:
    banned_exact = {
        "api_key",
        "apikey",
        "authorization",
        "token",
        "access_token",
        "refresh_token",
        "secret",
        "password",
        "client_secret",
    }
    lowered = key.lower()
    return lowered in banned_exact or lowered.endswith("_api_key") or lowered.endswith("_secret")


def scrub_secrets(node: Any) -> Any:
    """Drop keys that look like credentials; never persist auth material."""
    if isinstance(node, dict):
        out: dict[str, Any] = {}
        for key, value in node.items():
            if not isinstance(key, str) or is_credential_key(key):
                continue
            out[key] = scrub_secrets(value)
        return out
    if isinstance(node, list):
        return [scrub_secrets(item) for item in node]
    if isinstance(node, (str, int, float, bool)) or node is None:
        return node
    # Drop non-JSON-scalar oddities rather than serializing them.
    return None


def public_models_dev_document(data: dict[str, Any]) -> dict[str, Any]:
    """Construct a write-safe Models.dev-shaped document (public metadata only)."""
    out: dict[str, Any] = {}
    if isinstance(data.get("_meta"), dict):
        out["_meta"] = scrub_secrets(data["_meta"])
    if isinstance(data.get("models"), dict):
        out["models"] = scrub_secrets(data["models"])
    if isinstance(data.get("providers"), dict):
        out["providers"] = scrub_secrets(data["providers"])
    return out


def catalog_stats(data: dict[str, Any]) -> str:
    models = data.get("models") or {}
    providers = data.get("providers") or {}
    offerings = 0
    if isinstance(providers, dict):
        for prov in providers.values():
            if isinstance(prov, dict):
                models_map = prov.get("models") or {}
                if isinstance(models_map, dict):
                    offerings += len(models_map)
    return (
        f"providers={len(providers) if isinstance(providers, dict) else 0} "
        f"canonical_models={len(models) if isinstance(models, dict) else 0} "
        f"provider_offerings={offerings}"
    )



def cmd_refresh(args: argparse.Namespace) -> None:
    if args.provider and args.provider.lower() == "openrouter":
        refresh_openrouter(args)
        return
    if args.provider:
        die(
            f"unsupported --provider {args.provider!r} "
            "(supported: openrouter, or omit for Models.dev)"
        )

    data, source, _is_local = load_models_dev_catalog()
    print(f"loaded Models.dev catalog from {source}")
    print(catalog_stats(data))
    if args.write_cache or args.write:
        die(
            "disk writes are intentionally unsupported (secret-free by design); "
            "use `snapshot --check PATH` to validate a local Models.dev-shaped file, "
            "or `curl`/`CODEWHALE_MODELS_DEV_PATH` for staging"
        )
    print("dry-run complete (no secrets; no disk write)")


def refresh_openrouter(args: argparse.Namespace) -> None:
    url = DEFAULT_OPENROUTER_MODELS_URL
    raw = fetch_url(url)
    data = load_json_bytes(raw, url)
    if not isinstance(data, dict) or "data" not in data:
        die(f"{url}: expected {{ data: [...] }} envelope")
    rows = data["data"]
    if not isinstance(rows, list):
        die(f"{url}: data is not a list")

    # Optional sort / limit for local inspection — never secrets.
    if args.sort == "newest":
        def created_key(row: Any) -> float:
            if not isinstance(row, dict):
                return 0.0
            created = row.get("created")
            try:
                return float(created)
            except (TypeError, ValueError):
                return 0.0

        rows = sorted(rows, key=created_key, reverse=True)
    if args.limit is not None and args.limit > 0:
        rows = rows[: args.limit]

    # Project only public catalog fields — never the raw response object —
    # so credential-shaped keys cannot reach disk even if OpenRouter adds them.
    public_rows: list[dict[str, Any]] = []
    allowed = {
        "id",
        "name",
        "created",
        "description",
        "context_length",
        "architecture",
        "pricing",
        "top_provider",
        "per_request_limits",
        "supported_parameters",
    }
    for row in rows:
        if not isinstance(row, dict):
            continue
        projected: dict[str, Any] = {}
        for key in allowed:
            if key in row and not is_credential_key(key):
                projected[key] = scrub_secrets(row[key])
        if projected.get("id"):
            public_rows.append(projected)
    payload = {
        "_meta": {
            "source": "openrouter.ai/api/v1/models",
            "note": "Public model listing for cache dogfood; not the Models.dev SoT.",
            "count": len(public_rows),
            "sort": args.sort,
            "limit": args.limit,
        },
        "data": public_rows,
    }
    print(f"loaded OpenRouter models: {len(public_rows)} rows (sort={args.sort}, limit={args.limit})")
    if args.write_cache:
        # OpenRouter listing is always network-sourced; avoid disk write of remote JSON.
        die(
            "OpenRouter refresh is dry-run only (no disk write). "
            "Use Models.dev with CODEWHALE_MODELS_DEV_PATH for offline snapshots."
        )
    else:
        print("dry-run complete (OpenRouter writes disabled; use Models.dev local path for caches)")
    _ = payload  # keep payload construction for future offline path


def cmd_snapshot(args: argparse.Namespace) -> None:
    target = Path(args.path)
    if args.check:
        if not target.is_file():
            die(f"--check: missing {target}")
        raw = target.read_bytes()
        data = load_json_bytes(raw, str(target))
        ensure_models_dev_shape(data, str(target))
        print(f"ok: {target} is Models.dev-shaped ({catalog_stats(data)})")
        return

    data, source, _is_local = load_models_dev_catalog()
    print(f"loaded Models.dev catalog from {source}")
    print(catalog_stats(data))
    if args.write or args.force_full:
        die(
            "disk writes are intentionally unsupported for this automation; "
            "validate with --check, or stage a file outside this tool"
        )
    print("dry-run complete (use --check PATH to validate an existing snapshot)")


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Secret-free Models.dev / OpenRouter catalog automation (#4117)"
    )
    sub = p.add_subparsers(dest="cmd", required=True)

    refresh = sub.add_parser("refresh", help="Fetch live catalog / provider models")
    refresh.add_argument(
        "--provider",
        default=None,
        help="Optional provider id (currently: openrouter). Omit for Models.dev.",
    )
    refresh.add_argument(
        "--sort",
        default="newest",
        choices=["newest", "none"],
        help="OpenRouter sort order (default: newest)",
    )
    refresh.add_argument(
        "--limit",
        type=int,
        default=100,
        help="OpenRouter row cap (default: 100; 0 = no cap)",
    )
    refresh.add_argument(
        "--write-cache",
        metavar="PATH",
        help="Deprecated/unsupported: validate-only automation never writes fetched JSON",
    )
    refresh.add_argument(
        "--write",
        metavar="PATH",
        help="Deprecated/unsupported alias of --write-cache",
    )
    refresh.set_defaults(func=cmd_refresh)

    snapshot = sub.add_parser(
        "snapshot",
        help="Validate or write a Models.dev-shaped snapshot document",
    )
    snapshot.add_argument(
        "path",
        nargs="?",
        default="crates/config/assets/models_dev.bundled.json",
        help="Snapshot path (default: offline seed asset)",
    )
    snapshot.add_argument(
        "--check",
        action="store_true",
        help="Validate existing file only (no network)",
    )
    snapshot.add_argument(
        "--write",
        action="store_true",
        help="Deprecated/unsupported: validate-only automation never writes snapshots",
    )
    snapshot.add_argument(
        "--force-full",
        action="store_true",
        help="Deprecated/unsupported with --write; retained for clear failure messages",
    )
    snapshot.set_defaults(func=cmd_snapshot)
    return p


def main(argv: list[str] | None = None) -> None:
    parser = build_parser()
    args = parser.parse_args(argv)
    args.func(args)


if __name__ == "__main__":
    main()
