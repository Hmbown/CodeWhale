export type FeedKind = "issue" | "pull" | "release" | "discussion";

export interface FeedItem {
  kind: FeedKind;
  number: number;
  title: string;
  url: string;
  state: "open" | "closed" | "merged" | "draft" | "published";
  author: string;
  authorAvatar: string;
  createdAt: string; // ISO
  updatedAt: string; // ISO
  comments: number;
  labels: { name: string; color: string }[];
  body?: string;
}

export interface RepoStats {
  stars: number;
  forks: number;
  openIssues: number;
  openPulls: number;
  contributors: number;
  latestRelease?: { tag: string; publishedAt: string; url: string };
  fetchedAt: string;
}

export interface CuratedDispatch {
  generatedAt: string;
  /** English — always present (backward compat). */
  headline: string;
  summary: string;
  highlights: { title: string; href: string; tag: string; blurb: string }[];
  movers: { number: number; title: string; href: string; reason: string }[];
  /** zh-CN — populated by cron curate since ~May 2026. Falls back to English fields when absent. */
  headlineZh?: string;
  summaryZh?: string;
  highlightsZh?: { title: string; href: string; tag: string; blurb: string }[];
  moversZh?: { number: number; title: string; href: string; reason: string }[];
}
