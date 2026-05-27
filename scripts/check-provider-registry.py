#!/usr/bin/env python3
"""Check that docs/PROVIDERS.md tracks the shipped provider registry.

This is intentionally lightweight. It does not try to generate prose; it checks
the stable identifiers and default strings that are easy for docs to drift from:

- canonical ProviderKind IDs
- provider TOML tables
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
AGENT_RS = ROOT / "crates" / "agent" / "src" / "lib.rs"
PROVIDERS_MD = ROOT / "docs" / "PROVIDERS.md"

PROVIDER_VARIANT_TO_TABLE = {
    "Deepseek": "deepseek",
    "NvidiaNim": "nvidia_nim",
    "Openai": "openai",
    "Atlascloud": "atlascloud",
    "WanjieArk": "wanjie_ark",
    "Openrouter": "openrouter",
    "Novita": "novita",
    "Fireworks": "fireworks",
    "Moonshot": "moonshot",
    "Sglang": "sglang",
    "Vllm": "vllm",
    "Ollama": "ollama",
}


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def extract_match_block(source: str, signature: str) -> str:
    start = source.index(signature)
    match_start = source.index("match", start)
    brace_start = source.index("{", match_start)
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
    block = extract_match_block(config_rs, "pub fn as_str(self) -> &'static str")
    pairs = re.findall(r"Self::(\w+)\s*=>\s*\"([^\"]+)\"", block)
    if not pairs:
        raise ValueError("ProviderKind::as_str returned no providers")
    return {variant: provider_id for variant, provider_id in pairs}


def provider_tables(config_rs: str) -> set[str]:
    struct_start = config_rs.index("pub struct ProvidersToml")
    struct_end = config_rs.index("\n}", struct_start)
    fields = re.findall(
        r"pub\s+([a-z0-9_]+)\s*:\s*ProviderConfigToml",
        config_rs[struct_start:struct_end],
    )
    if not fields:
        raise ValueError("ProvidersToml returned no provider tables")
    return set(fields)


def shipped_provider_rows(providers_md: str) -> set[str]:
    heading = providers_md.index("## Shipped Providers")
    next_heading = providers_md.index("\n## ", heading + 1)
    table = providers_md[heading:next_heading]
    return set(re.findall(r"^\|\s*`([^`]+)`\s*\|", table, flags=re.MULTILINE))


def shipped_provider_tables(providers_md: str) -> set[str]:
    heading = providers_md.index("## Shipped Providers")
    next_heading = providers_md.index("\n## ", heading + 1)
    table = providers_md[heading:next_heading]
    return set(re.findall(r"\|\s*`\[providers\.([a-z0-9_]+)\]`\s*\|", table))


def static_registry_provider_rows(providers_md: str) -> set[str]:
    heading = providers_md.index("## Static Model Registry")
    next_heading = providers_md.index("\n## ", heading + 1)
    table = providers_md[heading:next_heading]
    return set(re.findall(r"^\|\s*`([^`]+)`\s*\|", table, flags=re.MULTILINE))


def model_registry_providers(agent_rs: str, variant_to_id: dict[str, str]) -> set[str]:
    variants = set(re.findall(r"provider:\s*ProviderKind::(\w+)", agent_rs))
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
    return sorted(value for value in defaults if value not in providers_md)


def report_set(label: str, expected: set[str], actual: set[str]) -> list[str]:
    errors = []
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    if missing:
        errors.append(f"{label} missing: {', '.join(missing)}")
    if extra:
        errors.append(f"{label} extra: {', '.join(extra)}")
    return errors


def main() -> int:
    config_rs = read(CONFIG_RS)
    tui_config_rs = read(TUI_CONFIG_RS)
    agent_rs = read(AGENT_RS)
    providers_md = read(PROVIDERS_MD)

    variant_to_id = provider_kind_ids(config_rs)
    canonical_ids = set(variant_to_id.values())
    missing_table_mappings = sorted(set(variant_to_id) - set(PROVIDER_VARIANT_TO_TABLE))
    if missing_table_mappings:
        raise ValueError(
            "PROVIDER_VARIANT_TO_TABLE is missing variants: "
            + ", ".join(missing_table_mappings)
        )
    expected_tables = {
        PROVIDER_VARIANT_TO_TABLE[variant] for variant in variant_to_id
    }

    errors: list[str] = []
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
        model_registry_providers(agent_rs, variant_to_id),
        static_registry_provider_rows(providers_md),
    )

    missing_defaults = missing_default_strings(providers_md, default_strings(tui_config_rs))
    if missing_defaults:
        errors.append(
            "docs/PROVIDERS.md does not mention default strings: "
            + ", ".join(missing_defaults)
        )

    if errors:
        print("Provider registry drift check failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print("Provider registry drift check passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
