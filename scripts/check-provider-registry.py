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
# ProviderKind's enum + identity impl were split out of lib.rs into this module.
PROVIDER_KIND_RS = ROOT / "crates" / "config" / "src" / "provider_kind.rs"
PROVIDER_RS = ROOT / "crates" / "config" / "src" / "provider.rs"
TUI_CONFIG_RS = ROOT / "crates" / "tui" / "src" / "config.rs"
# Default provider model/base-URL constants were split out of config.rs into
# this leaf module (#3311); read them from there for the default-string check.
TUI_CONFIG_MODELS_RS = ROOT / "crates" / "tui" / "src" / "config" / "models.rs"
AGENT_RS = ROOT / "crates" / "agent" / "src" / "lib.rs"
PROVIDERS_MD = ROOT / "docs" / "PROVIDERS.md"


API_PROVIDER_ONLY_IDS = {"deepseek-cn"}

# `custom` is the dynamic OpenAI-compatible meta-provider (#1519): a single
# catch-all `[providers.custom]` table that backs arbitrary user-defined
# endpoints, not a canonical shipped provider with a docs row. It is excluded
# from the provider-table drift check.
META_PROVIDER_TABLES = {"custom"}
SHARED_PROVIDER_TABLES = {
    "siliconflow-CN": "siliconflow_cn",
}
HUGGINGFACE_ALIASES = {"huggingface", "hugging-face", "hugging_face", "hf"}
HUGGINGFACE_API_KEY_ENV_ORDER = ["HUGGINGFACE_API_KEY", "HF_TOKEN"]
HUGGINGFACE_BASE_URL_ENV_ORDER = ["HUGGINGFACE_BASE_URL", "HF_BASE_URL"]
HUGGINGFACE_MODEL_ENV_ORDER = ["HUGGINGFACE_MODEL", "HF_MODEL"]
SENSITIVE_IDENTIFIER_RE = re.compile(r"(?i)(api[_-]?key|token|secret|password|credential)")
SENSITIVE_BEARER_RE = re.compile(r"(?i)(authorization:\s*bearer\s+)\S+")
SENSITIVE_ASSIGNMENT_RE = re.compile(
    r"(?i)\b(api[_-]?key|token|secret|password|credential)(\s*[:=]\s*)\S+"
)


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def display_public_value(value: str) -> str:
    if SENSITIVE_IDENTIFIER_RE.search(value):
        return "<redacted sensitive identifier>"
    return value


def redact_sensitive_text(value: str) -> str:
    value = SENSITIVE_BEARER_RE.sub(r"\1<redacted>", value)
    value = SENSITIVE_ASSIGNMENT_RE.sub(r"\1\2<redacted>", value)
    return SENSITIVE_IDENTIFIER_RE.sub("<redacted sensitive identifier>", value)


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


def parse_aliases_for_variant(source: str, enum_name: str, variant: str, context: str) -> set[str]:
    # `ProviderKind`'s enum + identity impl (incl. `parse`) live in
    # provider_kind.rs after the config module split; read the impl from there
    # regardless of the file the caller passed for other lookups.
    if enum_name == "ProviderKind":
        source = read(PROVIDER_KIND_RS)
        context = "crates/config/src/provider_kind.rs"
    impl_start = require_index(source, f"impl {enum_name}", context)
    block = extract_match_block(
        source,
        "pub fn parse(value: &str) -> Option<Self>",
        context,
        impl_start,
    )
    match_arm = re.search(
        rf'((?:"[^"]+"\s*\|\s*)*"[^"]+")\s*=>\s*Some\(Self::{variant}\)',
        block,
    )
    if match_arm:
        return set(re.findall(r'"([^"]+)"', match_arm.group(1)))
    if enum_name in {"ProviderKind", "ApiProvider"}:
        provider_rs = read(PROVIDER_RS)
        provider_macro = re.search(
            rf'provider!\(\s*\n\s*\w+,\s*\n\s*{variant},\s*\n\s*"([^"]+)".*?'
            r"aliases:\s*\[(.*?)\]\s*\);",
            provider_rs,
            re.DOTALL,
        )
        if provider_macro:
            return {provider_macro.group(1)} | set(
                re.findall(r'"([^"]+)"', provider_macro.group(2))
            )
    raise ValueError(f"{context}: missing parse arm for {variant}")


def provider_kind_ids(config_rs: str) -> dict[str, str]:
    provider_rs = read(PROVIDER_RS)
    pairs = re.findall(
        r"provider!\(\s*\n\s*\w+,\s*\n\s*(\w+),\s*\n\s*\"([^\"]+)\"",
        provider_rs,
    )
    ids: dict[str, str] = {variant: provider_id for variant, provider_id in pairs}
    # Providers with non-fixed wire policy or custom auth behavior use manual
    # impls rather than the provider!() macro.
    for variant_name, id_literal in [
        ("DeepseekAnthropic", "deepseek-anthropic"),
        ("OpenaiCodex", "openai-codex"),
        ("Anthropic", "anthropic"),
        ("Openmodel", "openmodel"),
        ("MinimaxAnthropic", "minimax-anthropic"),
        ("OpencodeZen", "opencode-zen"),
    ]:
        match = re.search(
            rf'impl\s+Provider\s+for\s+{variant_name}.*?fn\s+id.*?\"({id_literal})\"',
            provider_rs, re.DOTALL,
        )
        if match:
            ids[variant_name] = match.group(1)
    if not ids:
        raise ValueError("provider!() invocations returned no providers")
    return ids


def api_provider_ids(tui_config_rs: str) -> dict[str, str]:
    # ApiProvider ids derive from ProviderKind ids (via delegation to .kind().as_str())
    # plus the legacy "deepseek-cn" variant that exists only in ApiProvider.
    variant_to_id = provider_kind_ids("")
    # ApiProvider::SiliconflowCn maps to ProviderKind::SiliconflowCN
    if "SiliconflowCN" in variant_to_id:
        variant_to_id["SiliconflowCn"] = variant_to_id["SiliconflowCN"]
    variant_to_id["DeepseekCN"] = "deepseek-cn"
    return variant_to_id


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


def model_registry_providers(agent_rs: str, variant_to_id: dict[str, str]) -> set[str]:
    variants = set(re.findall(r"provider:\s*ProviderKind::(\w+)", agent_rs))
    missing = variants - set(variant_to_id)
    if missing:
        raise ValueError(f"ModelRegistry uses unknown provider variants: {sorted(missing)}")
    return {variant_to_id[variant] for variant in variants}


def default_strings(tui_config_rs: str) -> set[str]:
    # Model/base-URL constants now live in config/models.rs (#3311); scan it
    # alongside config.rs so the check follows the leaf split.
    sources = tui_config_rs + "\n" + read(TUI_CONFIG_MODELS_RS)
    defaults = set()
    for name, value in re.findall(
        r'const\s+(DEFAULT_[A-Z0-9_]+(?:MODEL|BASE_URL)):\s*&str\s*=\s*"([^"]+)"',
        sources,
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


def report_huggingface_coverage(
    config_rs: str, tui_config_rs: str, providers_md: str
) -> list[str]:
    errors = []

    config_aliases = parse_aliases_for_variant(
        config_rs, "ProviderKind", "Huggingface", "crates/config/src/lib.rs"
    )
    tui_aliases = parse_aliases_for_variant(
        tui_config_rs, "ApiProvider", "Huggingface", "crates/tui/src/config.rs"
    )
    errors += report_set(
        "ProviderKind Hugging Face aliases",
        HUGGINGFACE_ALIASES,
        config_aliases & HUGGINGFACE_ALIASES,
    )
    errors += report_set(
        "ApiProvider Hugging Face aliases",
        HUGGINGFACE_ALIASES,
        tui_aliases & HUGGINGFACE_ALIASES,
    )

    inline_source = re.sub(r"```.*?```", "", providers_md, flags=re.DOTALL)
    code_spans = set(re.findall(r"`([^`]+)`", inline_source))
    errors += report_set(
        "documented Hugging Face aliases",
        HUGGINGFACE_ALIASES,
        code_spans & HUGGINGFACE_ALIASES,
    )

    for label, env_order in [
        ("Hugging Face auth env precedence", HUGGINGFACE_API_KEY_ENV_ORDER),
        ("Hugging Face base URL env precedence", HUGGINGFACE_BASE_URL_ENV_ORDER),
        ("Hugging Face model env precedence", HUGGINGFACE_MODEL_ENV_ORDER),
    ]:
        errors += report_env_lookup_order(
            label, config_rs, env_order, "crates/config/src/lib.rs"
        )
        errors += report_env_lookup_order(
            label, tui_config_rs, env_order, "crates/tui/src/config.rs"
        )
        errors += report_string_order(label, providers_md, env_order, "docs/PROVIDERS.md")

    return errors


def report_env_lookup_order(
    label: str, source: str, expected_order: list[str], context: str
) -> list[str]:
    lookup_needles = [f'std::env::var("{name}")' for name in expected_order]
    return report_string_order(label, source, lookup_needles, context)


def report_string_order(
    label: str, source: str, expected_order: list[str], context: str
) -> list[str]:
    contains_sensitive_expected_value = any(
        SENSITIVE_IDENTIFIER_RE.search(value) for value in expected_order
    )
    positions = []
    for needle in expected_order:
        index = source.find(needle)
        if index == -1:
            if contains_sensitive_expected_value:
                return [f"{label} missing required entry in {context}"]
            return [f"{label} missing {display_public_value(needle)!r} in {context}"]
        positions.append(index)
    if positions != sorted(positions):
        if contains_sensitive_expected_value:
            return [f"{label} has wrong order in {context}"]
        return [
            f"{label} has wrong order in {context}: expected "
            + " before ".join(display_public_value(value) for value in expected_order)
        ]
    return []


def provider_table_name(provider_id: str) -> str:
    return SHARED_PROVIDER_TABLES.get(provider_id, provider_id.replace("-", "_"))


def main() -> int:
    try:
        config_rs = read(CONFIG_RS)
        tui_config_rs = read(TUI_CONFIG_RS)
        agent_rs = read(AGENT_RS)
        providers_md = read(PROVIDERS_MD)

        variant_to_id = provider_kind_ids(config_rs)
        canonical_ids = set(variant_to_id.values())
        live_api_provider_ids = set(api_provider_ids(tui_config_rs).values())
        expected_tables = {provider_table_name(provider_id) for provider_id in canonical_ids}

        errors: list[str] = []
        errors += report_provider_enum_drift(canonical_ids, live_api_provider_ids)
        errors += report_huggingface_coverage(config_rs, tui_config_rs, providers_md)
        errors += report_set(
            "shipped provider rows",
            canonical_ids,
            shipped_provider_rows(providers_md),
        )
        errors += report_set(
            "provider TOML tables",
            expected_tables,
            provider_tables(config_rs) - META_PROVIDER_TABLES,
        )
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
                "docs/PROVIDERS.md does not mention default strings as Markdown code spans: "
                + ", ".join(missing_defaults)
            )
    except ValueError as err:
        errors = [str(err)]

    if errors:
        print("Provider registry drift check failed:", file=sys.stderr)
        for error in errors:
            print(f"- {redact_sensitive_text(error)}", file=sys.stderr)
        return 1

    print("Provider registry drift check passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
