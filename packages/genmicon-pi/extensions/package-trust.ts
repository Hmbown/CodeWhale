import { readdirSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";

import type { LoadedResourceInventory, ReviewStatus } from "./state.js";

export interface PackageSettingsEntry {
  source: string;
  extensions?: readonly string[];
  skills?: readonly string[];
  prompts?: readonly string[];
  themes?: readonly string[];
}

export interface PackageReview {
  source: string;
  status: ReviewStatus;
  warnings: string[];
}

export function canEnablePlayerMode(review: PackageReview): boolean {
  return review.status === "reviewed";
}

export function reviewPackageSource(source: string): PackageReview {
  const warnings: string[] = [];
  let status: ReviewStatus = "unreviewed";

  if (source.startsWith("./") || source.startsWith("../") || source.startsWith("/")) {
    status = "reviewed";
  } else if (source.startsWith("npm:") || source.startsWith("git:") || source.startsWith("http")) {
    status = source.includes("@") ? "unreviewed" : "blocked";
    warnings.push("remote package sources require explicit review before player mode");
  } else {
    status = "blocked";
    warnings.push("unknown package source type");
  }

  return { source, status, warnings };
}

export function validatePackageFilters(entry: PackageSettingsEntry): string[] {
  const warnings: string[] = [];
  if (!entry.extensions || entry.extensions.length === 0) {
    warnings.push("no reviewed extension filter is declared");
  }
  if (entry.extensions?.some((path) => path.includes("**"))) {
    warnings.push("extension filters should avoid broad globs");
  }
  return warnings;
}

export function collectResourceInventory(packageRoot: string): LoadedResourceInventory {
  return {
    extensions: collectFiles(join(packageRoot, "extensions"), [".ts", ".js"]),
    skills: collectFiles(join(packageRoot, "skills"), ["SKILL.md"]),
    prompts: collectFiles(join(packageRoot, "prompts"), [".md"]),
    themes: collectFiles(join(packageRoot, "themes"), [".json"]),
  };
}

function collectFiles(root: string, suffixes: readonly string[]): string[] {
  try {
    return walk(root)
      .filter((path) => suffixes.some((suffix) => path.endsWith(suffix)))
      .map((path) => relative(root, path).split(sep).join("/"))
      .sort();
  } catch {
    return [];
  }
}

function walk(root: string): string[] {
  const entries = readdirSync(root);
  const files: string[] = [];
  for (const entry of entries) {
    const path = join(root, entry);
    const stat = statSync(path);
    if (stat.isDirectory()) {
      files.push(...walk(path));
    } else if (stat.isFile()) {
      files.push(path);
    }
  }
  return files;
}
