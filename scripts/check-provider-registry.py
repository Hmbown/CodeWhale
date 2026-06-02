#!/usr/bin/env python3
"""Check that docs/PROVIDERS.md tracks the shipped provider registry.

This is intentionally lightweight. It does not try to generate prose; it checks
the stable identifiers and default strings that are easy for docs to drift from:

- canonical ProviderKind IDs
- provider TOML tables
- live TUI ApiProvider IDs
- shipped-provider table rows
- static ModelRegistry provider rows
- default provider model/base URL constants
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CONFIG_RS = ROOT / "crates" / "config" / "src" / "lib.rs"
TUI_CONFIG_RS = ROOT / "crates" / "tui" / "src" / "config.rs"
MODEL_REGISTRY_RS = ROOT / "crates" / "config" / "src" / "model_registry.rs"
PROVIDERS_MD = ROOT / "docs" / "PROVIDERS.md"


API_PROVIDER_ONLY_IDS = {"deepseek-cn"}


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def require_index(source: str, needle: str, context: str, start: int = 0) -> int:
    try:
        return source.index(needle, start)
    except ValueError:
        raise ValueError(f"{context}: missing {needle!r}") from None


def markdown_section(source: str, heading: str) -> str:
    start = require_index(source, heading, "docs/PROVIDERS.md")
    next_heading = source.find("\n## ", start + len(heading))
    end = len(source) if next_heading == -1 else next_heading
    return source[start:end]


def extract_match_block(
    source: str, signature: str, context: str, start: int = 0
) -> str:
    start = require_index(source, signature, context, start)
    match_start = require_index(source, "match", f"match block after {signature!r}", start)
    brace_start = require_index(source, "{", f"match block after {signature!r}", match_start)
    depth = 0
    for index in range(brace_start, len(source)):
        char = source[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[brace_start + 1 : index]
    raise ValueError(f"could not parse match block after {signature!r}")


def provider_kind_ids(config_rs: str) -> dict[str, str]:
    impl_start = require_index(
        config_rs, "impl ProviderKind", "crates/config/src/lib.rs"
    )
    block = extract_match_block(
        config_rs,
        "pub fn as_str(self) -> &'static str",
        "crates/config/src/lib.rs",
        impl_start,
    )
    pairs = re.findall(r"Self::(\w+)\s*=>\s*\"([^\"]+)\"", block)
    if not pairs:
        raise ValueError("ProviderKind::as_str returned no providers")
    return {variant: provider_id for variant, provider_id in pairs}


def api_provider_ids(tui_config_rs: str) -> dict[str, str]:
    impl_start = require_index(
        tui_config_rs, "impl ApiProvider", "crates/tui/src/config.rs"
    )
    block = extract_match_block(
        tui_config_rs,
        "pub fn as_str(self) -> &'static str",
        "crates/tui/src/config.rs",
        impl_start,
    )
    pairs = re.findall(r"Self::(\w+)\s*=>\s*\"([^\"]+)\"", block)
    if not pairs:
        raise ValueError("ApiProvider::as_str returned no providers")
    return {variant: provider_id for variant, provider_id in pairs}


def provider_tables(config_rs: str) -> set[str]:
    struct_start = require_index(
        config_rs, "pub struct ProvidersToml", "crates/config/src/lib.rs"
    )
    struct_end = require_index(config_rs, "\n}", "ProvidersToml struct", struct_start)
    fields = re.findall(
        r"pub\s+([a-z0-9_]+)\s*:\s*ProviderConfigToml",
        config_rs[struct_start:struct_end],
    )
    if not fields:
        raise ValueError("ProvidersToml returned no provider tables")
    return set(fields)


def shipped_provider_rows(providers_md: str) -> set[str]:
    table = markdown_section(providers_md, "## Shipped Providers")
    return set(re.findall(r"^\|\s*`([^`]+)`\s*\|", table, flags=re.MULTILINE))


def shipped_provider_tables(providers_md: str) -> set[str]:
    table = markdown_section(providers_md, "## Shipped Providers")
    return set(re.findall(r"\|\s*`\[providers\.([a-z0-9_]+)\]`\s*\|", table))


def static_registry_provider_rows(providers_md: str) -> set[str]:
    table = markdown_section(providers_md, "## Static Model Registry")
    return set(re.findall(r"^\|\s*`([^`]+)`\s*\|", table, flags=re.MULTILINE))


def model_registry_providers(model_registry_rs: str, variant_to_id: dict[str, str]) -> set[str]:
    variants = set(re.findall(r"provider:\s*ProviderKind::(\w+)", model_registry_rs))
    missing = variants - set(variant_to_id)
    if missing:
        raise ValueError(f"ModelRegistry uses unknown provider variants: {sorted(missing)}")
    return {variant_to_id[variant] for variant in variants}


def default_strings(tui_config_rs: str) -> set[str]:
    defaults = set()
    for name, value in re.findall(
        r'const\s+(DEFAULT_[A-Z0-9_]+(?:MODEL|BASE_URL)):\s*&str\s*=\s*"([^"]+)"',
        tui_config_rs,
    ):
        if name == "DEFAULT_DEEPSEEKCN_BASE_URL":
            continue
        defaults.add(value)
    if not defaults:
        raise ValueError("no default provider model/base URL constants found")
    return defaults


def missing_default_strings(providers_md: str, defaults: set[str]) -> list[str]:
    # Inline-code validation should not let fenced TOML/bash examples pair a
    # stray backtick with later prose; strip fenced blocks before scanning.
    inline_source = re.sub(r"```.*?```", "", providers_md, flags=re.DOTALL)
    code_spans = set(re.findall(r"`([^`]+)`", inline_source))
    return sorted(defaults - code_spans)


def report_set(label: str, expected: set[str], actual: set[str]) -> list[str]:
    errors = []
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    if missing:
        errors.append(f"{label} missing: {', '.join(missing)}")
    if extra:
        errors.append(f"{label} extra: {', '.join(extra)}")
    return errors


def report_provider_enum_drift(
    provider_kind_ids: set[str], api_provider_ids: set[str]
) -> list[str]:
    errors = []
    missing_from_api_provider = sorted(provider_kind_ids - api_provider_ids)
    unexpected_api_provider_ids = sorted(
        api_provider_ids - provider_kind_ids - API_PROVIDER_ONLY_IDS
    )
    missing_allowlisted_ids = sorted(API_PROVIDER_ONLY_IDS - api_provider_ids)

    if missing_from_api_provider:
        errors.append(
            "ApiProvider missing ProviderKind IDs: "
            + ", ".join(missing_from_api_provider)
        )
    if unexpected_api_provider_ids:
        errors.append(
            "ApiProvider has non-whitelisted IDs absent from ProviderKind: "
            + ", ".join(unexpected_api_provider_ids)
        )
    if missing_allowlisted_ids:
        errors.append(
            "ApiProvider-only whitelist entries are absent from ApiProvider: "
            + ", ".join(missing_allowlisted_ids)
        )
    return errors


def main() -> int:
    try:
        config_rs = read(CONFIG_RS)
        tui_config_rs = read(TUI_CONFIG_RS)
        model_registry_rs = read(MODEL_REGISTRY_RS)
        providers_md = read(PROVIDERS_MD)

        variant_to_id = provider_kind_ids(config_rs)
        canonical_ids = set(variant_to_id.values())
        live_api_provider_ids = set(api_provider_ids(tui_config_rs).values())
        expected_tables = {provider_id.replace("-", "_") for provider_id in canonical_ids}

        errors: list[str] = []
        errors += report_provider_enum_drift(canonical_ids, live_api_provider_ids)
        errors += report_set(
            "shipped provider rows",
            canonical_ids,
            shipped_provider_rows(providers_md),
        )
        errors += report_set("provider TOML tables", expected_tables, provider_tables(config_rs))
        errors += report_set(
            "documented provider TOML tables",
            expected_tables,
            shipped_provider_tables(providers_md),
        )
        errors += report_set(
            "static ModelRegistry rows",
            model_registry_providers(model_registry_rs, variant_to_id),
            static_registry_provider_rows(providers_md),
        )

        missing_defaults = missing_default_strings(providers_md, default_strings(tui_config_rs))
        if missing_defaults:
            errors.append(
                "docs/PROVIDERS.md does not mention default strings as Markdown code spans: "
                + ", ".join(missing_defaults)
            )
    except ValueError as err:
        errors = [str(err)]

    if errors:
        print("Provider registry drift check failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print("Provider registry drift check passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
