import type { ChangelogDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/changelog/page.tsx`.
 * Version numbers, tags, dates, and release-note text are content from
 * `lib/facts.generated.ts` and `lib/changelog.generated.ts`, never copy.
 */
export const changelog: ChangelogDict = {
  metaTitle: "Changelog · Codewhale",
  metaDescription:
    "Codewhale release record: the latest published release, the unreleased source candidate, and the notes for each version, drawn from CHANGELOG.md in the repository.",
  kicker: "Release record",
  title: "What changed, and in which version.",
  lead:
    "Two facts sit at the top of this page: the newest published release, and the version the source tree currently declares. Everything below is the repository's own CHANGELOG.md, section by section.",
  publishedLabel: "Latest published release",
  publishedValue: "{tag} · published {date}",
  candidateLabel: "Source candidate",
  candidateValue: "{version} · unreleased",
  candidateMatches: "{version} · matches the published release",
  releasesLink: "GitHub Releases ↗",
  unreleasedHeading: "Unreleased",
  unreleasedNote:
    "Changes merged to the main branch since the last tag. They are part of the source candidate, not of any published package.",
  compareLink: "Compare on GitHub ↗",
  releasePageLink: "Release page ↗",
  moreEntries: "{shown} of {total} entries shown",
  fullNotes: "Full notes in CHANGELOG.md ↗",
  releaseNotesLink: "Full notes for {version} ↗",
  emptyTitle: "No release notes were derived",
  emptyBody:
    "The build did not find a parsable CHANGELOG.md. The GitHub release list is still the record.",
};
