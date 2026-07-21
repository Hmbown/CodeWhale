#!/usr/bin/env python3
"""Validate that harvested contributor credit is GitHub-mappable.

The check is intentionally scoped to new commits. Historical commits may carry
raw or local emails, but new harvested commits should use GitHub's numeric
`id+login@users.noreply.github.com` address so co-author credit lands in the
contributor graph. Preserved integration commits may resolve a mapped identity
without a history rewrite only through an exact-SHA exception below.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_AUTHOR_MAP = ROOT / ".github" / "AUTHOR_MAP"

IDENTITY_RE = re.compile(r"^\s*(?P<name>.+?)\s*<(?P<email>[^<>]+)>\s*$")
CANONICAL_NOREPLY_RE = re.compile(
    r"^[0-9]+\+[^@\s]+@users\.noreply\.github\.com$", re.IGNORECASE
)
COAUTHOR_RE = re.compile(
    r"^Co-authored-by:\s*(?P<name>.*?)\s*<(?P<email>[^<>]+)>\s*$",
    re.IGNORECASE | re.MULTILINE,
)
HARVEST_RE = re.compile(r"Harvested from PR #[0-9]+ by @([A-Za-z0-9-]+)")

BOT_EMAILS = {
    "codex@local",
    "codex@example.com",
    "cursoragent@cursor.com",
    "noreply@anthropic.com",
}
BOT_NAMES = ("claude", "codex", "cursor")

# This commit is already immutable history on origin/main. Its trailer names a
# local Codewhale automation actor, not a human contributor. It escaped the
# existing gate and is now immutable on origin/main. Rewriting main would
# invalidate every descendant, while mapping the actor to a human would
# manufacture contributor credit. Exempt only the exact full SHA + exact actor
# identity; every other malformed trailer still fails.
LEGACY_AUTOMATION_TRAILER_EXCEPTIONS = {
    (
        "9a74825cd182a62465943bcbbcbcf591d1ce99ee",
        "codewhale agent",
        "codewhale-agent@hmbown.local",
    ),
}

# These public-surface commits were merged into the v0.9.1 integration graph
# before the credit gate ran. Rewriting them would replace the original commits
# and every descendant merge. Resolve only their exact Hunter identities through
# AUTHOR_MAP; a changed SHA, role, name, or email remains a hard failure.
PRESERVED_MAPPED_IDENTITY_EXCEPTIONS = {
    (
        "5087269606fc8847487b0a8b51ef6adffa8eb2ca",
        "author",
        "hunter b",
        "hmbown@gmail.com",
    ),
    (
        "5087269606fc8847487b0a8b51ef6adffa8eb2ca",
        "coauthor",
        "hunter bown",
        "hmbown@gmail.com",
    ),
    (
        "e37df06caeb3064b2bb9263c1c98a903738f3a0a",
        "coauthor",
        "hunter bown",
        "hmbown@gmail.com",
    ),
}


@dataclass(frozen=True)
class Identity:
    name: str
    email: str

    def trailer(self) -> str:
        return f"Co-authored-by: {self.name} <{self.email}>"

    def author(self) -> str:
        return f"{self.name} <{self.email}>"


@dataclass(frozen=True)
class Commit:
    sha: str
    parents: str
    author_name: str
    author_email: str
    subject: str
    body: str

    def is_merge_commit(self) -> bool:
        return len(self.parents.split()) > 1


def norm_key(value: str) -> str:
    return value.strip().lower()


def github_login_from_noreply(email: str) -> str | None:
    if not CANONICAL_NOREPLY_RE.match(email):
        return None
    local = email.split("@", 1)[0]
    return local.split("+", 1)[1]


def parse_identity(raw: str, context: str) -> Identity:
    match = IDENTITY_RE.match(raw)
    if not match:
        raise ValueError(f"{context}: expected 'Name <id+login@users.noreply.github.com>'")
    identity = Identity(match.group("name").strip(), match.group("email").strip())
    if not CANONICAL_NOREPLY_RE.match(identity.email):
        raise ValueError(
            f"{context}: right-hand email must be numeric GitHub noreply, got {identity.email}"
        )
    return identity


def load_author_map(path: Path) -> dict[str, Identity]:
    aliases: dict[str, Identity] = {}
    for lineno, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue
        if "=" not in line:
            raise ValueError(f"{path}:{lineno}: expected 'alias = Name <email>'")
        alias, raw_identity = [part.strip() for part in line.split("=", 1)]
        identity = parse_identity(raw_identity, f"{path}:{lineno}")
        key = norm_key(alias)
        if key in aliases and aliases[key] != identity:
            raise ValueError(f"{path}:{lineno}: duplicate alias {alias!r}")
        aliases[key] = identity
        aliases.setdefault(norm_key(identity.email), identity)
        aliases.setdefault(norm_key(identity.name), identity)
        if login := github_login_from_noreply(identity.email):
            aliases.setdefault(norm_key(login), identity)
    return aliases


def git_log(commit_range: str) -> list[Commit]:
    try:
        raw = subprocess.check_output(
            [
                "git",
                "log",
                "--format=%H%x00%P%x00%an%x00%ae%x00%s%x00%B%x1e",
                commit_range,
            ],
            cwd=ROOT,
            text=True,
        )
    except subprocess.CalledProcessError as exc:
        raise RuntimeError(f"failed to read git range {commit_range!r}: {exc}") from exc

    commits: list[Commit] = []
    for record in raw.split("\x1e"):
        if not record.strip():
            continue
        # `git log` emits a newline after each record separator. Remove only
        # that framing byte so the next record's full SHA remains exact while
        # preserving commit-body whitespace.
        record = record.lstrip("\n")
        parts = record.split("\x00", 5)
        if len(parts) != 6:
            raise RuntimeError("failed to parse git log output")
        commits.append(Commit(*parts))
    return commits


def is_bot_identity(name: str, email: str) -> bool:
    lowered_name = name.strip().lower()
    lowered_email = email.strip().lower()
    return lowered_email in BOT_EMAILS or any(
        lowered_name == bot or lowered_name.startswith(f"{bot} ") for bot in BOT_NAMES
    )


def lookup_identity(aliases: dict[str, Identity], *values: str) -> Identity | None:
    for value in values:
        identity = aliases.get(norm_key(value))
        if identity is not None:
            return identity
    return None


def is_preserved_mapped_identity(commit: Commit, role: str, identity: Identity) -> bool:
    return (
        commit.sha.strip().lower(),
        role,
        norm_key(identity.name),
        norm_key(identity.email),
    ) in PRESERVED_MAPPED_IDENTITY_EXCEPTIONS


def validate(commits: list[Commit], aliases: dict[str, Identity], check_authors: bool) -> list[str]:
    errors: list[str] = []
    for commit in commits:
        prefix = f"{commit.sha[:10]} {commit.subject}"
        coauthors = [
            Identity(match.group("name").strip(), match.group("email").strip())
            for match in COAUTHOR_RE.finditer(commit.body)
        ]
        harvested_logins = HARVEST_RE.findall(commit.body)
        is_harvested_commit = bool(harvested_logins)
        mapped_author = lookup_identity(aliases, commit.author_email, commit.author_name)

        if check_authors:
            if is_harvested_commit and is_bot_identity(commit.author_name, commit.author_email):
                errors.append(
                    f"{prefix}: author {commit.author_name} <{commit.author_email}> is a "
                    "bot/tool identity. Human harvested work should preserve the contributor "
                    "as author or use a human co-author trailer."
                )
            elif (
                is_harvested_commit
                and mapped_author
                and norm_key(commit.author_email) != norm_key(mapped_author.email)
                and not is_preserved_mapped_identity(
                    commit,
                    "author",
                    Identity(commit.author_name, commit.author_email),
                )
            ):
                errors.append(
                    f"{prefix}: author {commit.author_name} <{commit.author_email}> "
                    f"matches AUTHOR_MAP but is not canonical. Use author {mapped_author.author()}."
                )

        for coauthor in coauthors:
            if (
                commit.sha.strip().lower(),
                norm_key(coauthor.name),
                norm_key(coauthor.email),
            ) in LEGACY_AUTOMATION_TRAILER_EXCEPTIONS:
                continue
            if is_bot_identity(coauthor.name, coauthor.email):
                if not commit.is_merge_commit():
                    errors.append(
                        f"{prefix}: remove bot/tool co-author trailer "
                        f"{coauthor.name} <{coauthor.email}>; contributor trailers are for humans."
                    )
                continue
            if CANONICAL_NOREPLY_RE.match(coauthor.email):
                continue
            expected = lookup_identity(aliases, coauthor.email, coauthor.name)
            if expected:
                if not is_preserved_mapped_identity(commit, "coauthor", coauthor):
                    errors.append(
                        f"{prefix}: co-author {coauthor.name} <{coauthor.email}> is not "
                        f"GitHub-mappable. Use `{expected.trailer()}`."
                    )
            else:
                errors.append(
                    f"{prefix}: co-author {coauthor.name} <{coauthor.email}> is not "
                    "numeric GitHub noreply and has no AUTHOR_MAP entry. Add an alias "
                    "or use `gh api users/<login> --jq '\"\\(.id)+\\(.login)@users.noreply.github.com\"'`."
                )

        coauthor_emails: set[str] = set()
        for coauthor in coauthors:
            coauthor_emails.add(norm_key(coauthor.email))
            expected = lookup_identity(aliases, coauthor.email, coauthor.name)
            if expected and is_preserved_mapped_identity(commit, "coauthor", coauthor):
                coauthor_emails.add(norm_key(expected.email))
        for login in harvested_logins:
            expected = lookup_identity(aliases, login)
            if expected is None:
                errors.append(
                    f"{prefix}: harvested contributor @{login} is missing from .github/AUTHOR_MAP."
                )
                continue
            if (
                norm_key(commit.author_email) != norm_key(expected.email)
                and norm_key(expected.email) not in coauthor_emails
            ):
                errors.append(
                    f"{prefix}: `Harvested from PR ... by @{login}` needs machine-readable "
                    f"credit. Add `{expected.trailer()}` or preserve the contributor as author."
                )
    return errors


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--author-map", type=Path, default=DEFAULT_AUTHOR_MAP)
    parser.add_argument("--range", default="origin/main..HEAD", help="git commit range to check")
    parser.add_argument(
        "--check-authors",
        action="store_true",
        help="also reject commit author emails that match known AUTHOR_MAP aliases",
    )
    args = parser.parse_args(argv)

    try:
        aliases = load_author_map(args.author_map)
        commits = git_log(args.range)
        errors = validate(commits, aliases, args.check_authors)
    except Exception as exc:
        print(f"co-author credit check failed to run: {exc}", file=sys.stderr)
        return 2

    if errors:
        print("Co-author credit check failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(f"Co-author credit check passed for {len(commits)} commit(s).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
