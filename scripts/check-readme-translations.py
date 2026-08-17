#!/usr/bin/env python3
"""Keep the localized READMEs in lockstep with README.md.

Every translated README must:
  1. carry a source stamp `<!-- source: README.md sha256:<12-hex> -->`
     matching the current hash of README.md — so any English edit fails CI
     until the translations are refreshed;
  2. contain exactly the same fenced code blocks, in the same order
     (commands are never translated);
  3. contain every non-language-switcher URL the English README contains;
  4. have the same number of `##` sections.

Run: python3 scripts/check-readme-translations.py
"""

from __future__ import annotations

import hashlib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "README.md"
TRANSLATIONS = [
    "README.zh-CN.md",
    "README.ja-JP.md",
    "README.vi.md",
    "README.id.md",
    "README.ko-KR.md",
    "README.es-419.md",
    "README.pt-BR.md",
    "README.ru.md",
    "README.uk.md",
    "README.fr.md",
    "README.de.md",
    "README.zh-TW.md",
    "README.hi.md",
    "README.tr.md",
    "README.it.md",
    "README.pl.md",
    "README.ar.md",
    "README.ca.md",
]
STAMP_RE = re.compile(r"<!--\s*source:\s*README\.md\s+sha256:([0-9a-f]{12})\s*-->")
FENCE_RE = re.compile(r"```[a-z]*\n(.*?)```", re.DOTALL)
URL_RE = re.compile(r"\((https?://[^)\s]+|docs/[^)\s]+|[A-Za-z0-9_./-]+\.md[^)\s]*)\)")
# The language-switcher line legitimately differs per translation (each file
# links the *other* languages), so its links are exempt from the URL check.
LANGUAGE_LINKS = {
    "README.md",
    "README.zh-CN.md",
    "README.ja-JP.md",
    "README.vi.md",
    "README.id.md",
    "README.ko-KR.md",
    "README.es-419.md",
    "README.pt-BR.md",
    "README.ru.md",
    "README.uk.md",
    "README.fr.md",
    "README.de.md",
    "README.zh-TW.md",
    "README.hi.md",
    "README.tr.md",
    "README.it.md",
    "README.pl.md",
    "README.ar.md",
    "README.ca.md",
}


def source_stamp() -> str:
    return hashlib.sha256(SOURCE.read_bytes()).hexdigest()[:12]


def fences(text: str) -> list[str]:
    return [m.strip() for m in FENCE_RE.findall(text)]


def urls(text: str) -> set[str]:
    return {u for u in URL_RE.findall(text) if u not in LANGUAGE_LINKS}


def sections(text: str) -> int:
    return len(re.findall(r"^## ", text, re.MULTILINE))


def main() -> int:
    expected = source_stamp()
    en = SOURCE.read_text()
    en_fences = fences(en)
    en_urls = urls(en)
    en_sections = sections(en)
    failures: list[str] = []

    for name in TRANSLATIONS:
        path = ROOT / name
        if not path.exists():
            failures.append(f"{name}: missing")
            continue
        text = path.read_text()

        stamp = STAMP_RE.search(text)
        if not stamp:
            failures.append(
                f"{name}: no source stamp — add "
                f"'<!-- source: README.md sha256:{expected} -->'"
            )
        elif stamp.group(1) != expected:
            failures.append(
                f"{name}: stale (stamped {stamp.group(1)}, README.md is now "
                f"{expected}) — retranslate, then update the stamp"
            )

        tr_fences = fences(text)
        if tr_fences != en_fences:
            failures.append(
                f"{name}: code blocks differ from README.md "
                f"({len(tr_fences)} vs {len(en_fences)}; commands must never "
                f"be translated or reordered)"
            )

        missing = en_urls - urls(text)
        if missing:
            failures.append(f"{name}: missing links: {sorted(missing)[:5]}")

        if sections(text) != en_sections:
            failures.append(
                f"{name}: {sections(text)} '##' sections vs README.md's "
                f"{en_sections}"
            )

    if failures:
        print("README translation check FAILED:")
        for f in failures:
            print(f"  - {f}")
        print(f"\nCurrent README.md stamp: sha256:{expected}")
        return 1

    print(
        f"README translation check OK — {len(TRANSLATIONS)} translations in "
        f"sync with README.md (sha256:{expected})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
