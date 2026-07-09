#!/usr/bin/env python3
"""Regression tests for scripts/check-coauthor-trailers.py."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "check-coauthor-trailers.py"
AUTHOR_MAP = ROOT / ".github" / "AUTHOR_MAP"
FIXTURES = Path(__file__).resolve().parent / "fixtures" / "coauthor-trailers"

SPEC = importlib.util.spec_from_file_location("check_coauthor_trailers", SCRIPT)
assert SPEC and SPEC.loader
mod = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = mod
SPEC.loader.exec_module(mod)


def commit(subject: str, body: str, *, harvested: bool = False) -> mod.Commit:
    if harvested and "Harvested from PR" not in body:
        body = f"{body}\n\nHarvested from PR #1 by @contributor"
    return mod.Commit(
        sha="deadbeef" * 5,
        parents="",
        author_name="Maintainer",
        author_email="1+maintainer@users.noreply.github.com",
        subject=subject,
        body=body,
    )


class CheckCoauthorTrailersTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.aliases = mod.load_author_map(AUTHOR_MAP)

    def test_rejects_cursor_trailer_on_non_harvested_commit(self) -> None:
        body = (FIXTURES / "cursor-non-harvested.txt").read_text(encoding="utf-8")
        errors = mod.validate([commit("direct change", body)], self.aliases, False)
        self.assertTrue(errors)
        self.assertIn("cursoragent@cursor.com", errors[0])

    def test_rejects_cursor_trailer_on_harvested_commit(self) -> None:
        body = (FIXTURES / "cursor-harvested.txt").read_text(encoding="utf-8")
        errors = mod.validate([commit("harvested change", body, harvested=True)], self.aliases, False)
        self.assertTrue(errors)

    def test_allows_human_canonical_trailer(self) -> None:
        body = (FIXTURES / "human-canonical.txt").read_text(encoding="utf-8")
        errors = mod.validate([commit("human credit", body)], self.aliases, False)
        self.assertEqual(errors, [])

    def test_allows_merge_commit_with_bot_trailer(self) -> None:
        merge = mod.Commit(
            sha="cafebabe" * 5,
            parents="aaa bbb",
            author_name="Maintainer",
            author_email="1+maintainer@users.noreply.github.com",
            subject="Merge branch",
            body="Co-authored-by: Cursor <cursoragent@cursor.com>",
        )
        errors = mod.validate([merge], self.aliases, False)
        self.assertEqual(errors, [])


if __name__ == "__main__":
    raise SystemExit(unittest.main())
