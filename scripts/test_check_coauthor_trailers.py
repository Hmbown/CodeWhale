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

    def test_allows_only_the_exact_immutable_automation_trailer(self) -> None:
        legacy = mod.Commit(
            sha="9a74825cd182a62465943bcbbcbcf591d1ce99ee",
            parents="parent",
            author_name="Maintainer",
            author_email="1+maintainer@users.noreply.github.com",
            subject="legacy automation commit",
            body="Co-authored-by: CodeWhale Agent <codewhale-agent@hmbown.local>",
        )
        errors = mod.validate([legacy], self.aliases, False)
        self.assertEqual(errors, [])

        other_commit = mod.Commit(
            sha="deadbeef" * 5,
            parents=legacy.parents,
            author_name=legacy.author_name,
            author_email=legacy.author_email,
            subject=legacy.subject,
            body=legacy.body,
        )
        errors = mod.validate([other_commit], self.aliases, False)
        self.assertTrue(errors)
        self.assertIn("codewhale-agent@hmbown.local", errors[0])

    def test_legacy_exception_does_not_hide_another_bad_trailer(self) -> None:
        legacy_with_extra = mod.Commit(
            sha="9a74825cd182a62465943bcbbcbcf591d1ce99ee",
            parents="parent",
            author_name="Maintainer",
            author_email="1+maintainer@users.noreply.github.com",
            subject="legacy automation commit",
            body=(
                "Co-authored-by: CodeWhale Agent <codewhale-agent@hmbown.local>\n"
                "Co-authored-by: Unknown Person <unknown@example.com>"
            ),
        )
        errors = mod.validate([legacy_with_extra], self.aliases, False)
        self.assertEqual(len(errors), 1)
        self.assertIn("unknown@example.com", errors[0])

    def test_preserved_hunter_credit_resolves_only_for_exact_commit(self) -> None:
        preserved = mod.Commit(
            sha="5087269606fc8847487b0a8b51ef6adffa8eb2ca",
            parents="parent",
            author_name="Hunter B",
            author_email="hmbown@gmail.com",
            subject="docs(public): refresh v0.9.1 product truth",
            body=(
                "Harvested from PR #4508 by @Hmbown.\n\n"
                "Co-authored-by: Hunter Bown <hmbown@gmail.com>"
            ),
        )
        errors = mod.validate([preserved], self.aliases, True)
        self.assertEqual(errors, [])

        rewritten = mod.Commit(
            sha="deadbeef" * 5,
            parents=preserved.parents,
            author_name=preserved.author_name,
            author_email=preserved.author_email,
            subject=preserved.subject,
            body=preserved.body,
        )
        errors = mod.validate([rewritten], self.aliases, True)
        self.assertTrue(errors)
        self.assertTrue(any("not GitHub-mappable" in error for error in errors))

    def test_preserved_web_credit_resolves_without_rewriting_commit(self) -> None:
        preserved = mod.Commit(
            sha="e37df06caeb3064b2bb9263c1c98a903738f3a0a",
            parents="parent",
            author_name="Hunter B",
            author_email="hmbown@gmail.com",
            subject="docs(web): make the homepage product first",
            body="Co-authored-by: Hunter Bown <hmbown@gmail.com>",
        )
        errors = mod.validate([preserved], self.aliases, True)
        self.assertEqual(errors, [])


if __name__ == "__main__":
    raise SystemExit(unittest.main())
