#!/usr/bin/env python3
"""check-tui-locale-parity.py — CI gate against TUI locale pack drift.

`en.json` is the reference pack. Every pack that claims completeness must
hold exact raw key-set parity with it in both directions, and every
`{named}` placeholder must survive translation (call sites substitute with
`.replace()`, so a dropped placeholder renders literal braces at runtime).

Declared partial packs (PARTIAL_PACKS, mirroring `Locale::is_partial_pack()`
in `crates/tui/src/localization.rs`) are exempt from completeness but must
not define keys English lacks — an extra key in a partial pack is drift the
English fallback can never surface.

This gate is the CI-visible half of the Rust parity tests
(`shipped_complete_packs_have_raw_key_parity_with_english`,
`message_id_list_english_pack_stay_in_exact_sync`). It exists so that pack
files on disk — including packs whose `Locale` wiring has not landed yet —
are held to the same contract, and so the failure is attributable to a file
rather than buried in a test binary.

Exits non-zero on any parity violation.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCALES_DIR = ROOT / "crates" / "tui" / "locales"
REFERENCE = "en"

# Packs that ship deliberately incomplete, with English fallback for the
# missing keys. Mirrors `Locale::is_partial_pack()`. Every entry needs an
# issue reference; a partial pack without a tracking issue is silent drift.
PARTIAL_PACKS = {
    "zh-Hant": "#4057",  # Setup core only; English fallback for the rest.
}

PLACEHOLDER_RE = re.compile(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}")


def load_pack(path: Path) -> dict:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"[tui-locale-parity] FAIL — {path.name}: unreadable JSON: {exc}")
        sys.exit(1)
    if not isinstance(data, dict) or not all(
        isinstance(k, str) and isinstance(v, str) for k, v in data.items()
    ):
        print(f"[tui-locale-parity] FAIL — {path.name}: must be a flat string map")
        sys.exit(1)
    return data


def placeholders(value: str) -> set:
    return set(PLACEHOLDER_RE.findall(value))


def main() -> int:
    ref_path = LOCALES_DIR / f"{REFERENCE}.json"
    if not ref_path.is_file():
        print(f"[tui-locale-parity] FAIL — reference pack {ref_path} missing")
        return 1
    reference = load_pack(ref_path)
    ref_keys = set(reference)
    print(f"[tui-locale-parity] reference {REFERENCE}.json: {len(ref_keys)} keys")

    failures = []
    pack_files = sorted(
        p for p in LOCALES_DIR.glob("*.json") if p.stem != REFERENCE
    )
    for path in pack_files:
        tag = path.stem
        pack = load_pack(path)
        keys = set(pack)
        partial_issue = PARTIAL_PACKS.get(tag)

        missing = sorted(ref_keys - keys)
        extra = sorted(keys - ref_keys)

        if extra:
            failures.append(
                f"{tag}: defines {len(extra)} key(s) {REFERENCE}.json lacks: {extra[:10]}"
            )
        if partial_issue:
            print(
                f"[tui-locale-parity] {tag}: {len(keys)}/{len(ref_keys)} keys "
                f"(declared partial, {partial_issue})"
            )
        else:
            if missing:
                failures.append(
                    f"{tag}: claims completeness but lacks {len(missing)} key(s); "
                    f"the English fallback hides these at runtime: {missing[:10]}"
                )
            # Placeholder parity only makes sense on complete packs: a
            # partial pack legitimately omits keys wholesale.
            for key in sorted(ref_keys & keys):
                ref_ph = placeholders(reference[key])
                if placeholders(pack[key]) != ref_ph:
                    failures.append(
                        f"{tag}: {key} changed placeholders "
                        f"(expected {sorted(ref_ph)}, got {sorted(placeholders(pack[key]))})"
                    )
            if not missing:
                print(f"[tui-locale-parity] {tag}: {len(keys)}/{len(ref_keys)} keys — complete")

    if failures:
        print("[tui-locale-parity] FAIL")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print("[tui-locale-parity] PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
