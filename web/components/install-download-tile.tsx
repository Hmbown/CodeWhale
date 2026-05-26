"use client";

import { useEffect, useState } from "react";

type Arch = "macos-arm64" | "macos-x64" | "linux-x64" | "linux-arm64" | "windows-x64";

const BASE =
  "https://github.com/Hmbown/CodeWhale/releases/latest/download";

const ASSETS: Record<Arch, { zip: string; sha: string }> = {
  "macos-arm64": {
    zip: `${BASE}/codewhale-macos-arm64.zip`,
    sha: `${BASE}/codewhale-artifacts-sha256.txt`,
  },
  "macos-x64": {
    zip: `${BASE}/codewhale-macos-x64.zip`,
    sha: `${BASE}/codewhale-artifacts-sha256.txt`,
  },
  "linux-x64": {
    zip: `${BASE}/codewhale-linux-x64.zip`,
    sha: `${BASE}/codewhale-artifacts-sha256.txt`,
  },
  "linux-arm64": {
    zip: `${BASE}/codewhale-linux-arm64.zip`,
    sha: `${BASE}/codewhale-artifacts-sha256.txt`,
  },
  "windows-x64": {
    zip: `${BASE}/codewhale-windows-x64.zip`,
    sha: `${BASE}/codewhale-artifacts-sha256.txt`,
  },
};

const LABELS: Record<Arch, string> = {
  "macos-arm64": "macOS · Apple Silicon",
  "macos-x64": "macOS · Intel",
  "linux-x64": "Linux · x64",
  "linux-arm64": "Linux · arm64",
  "windows-x64": "Windows · x64",
};

function detect(): Arch {
  if (typeof navigator === "undefined") return "macos-arm64";
  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes("win")) return "windows-x64";
  if (ua.includes("linux")) {
    if (ua.includes("aarch64") || ua.includes("arm64")) return "linux-arm64";
    return "linux-x64";
  }
  return "macos-arm64";
}

interface Props {
  heading: string;
  downloadLabel: string;
  sha256Label: string;
  mirrorHeading: string;
  mirrorGhproxy: string;
  mirrorJsdelivr: string;
  offlineCallout: string;
}

export function InstallDownloadTile({
  heading,
  downloadLabel,
  sha256Label,
  mirrorHeading,
  mirrorGhproxy,
  mirrorJsdelivr,
  offlineCallout,
}: Props) {
  const [arch, setArch] = useState<Arch>("macos-arm64");

  useEffect(() => {
    setArch(detect());
  }, []);

  const { zip, sha } = ASSETS[arch];
  const ghproxy = `https://ghproxy.com/${zip}`;

  return (
    <div>
      {/* Arch selector tabs */}
      <div className="flex flex-wrap gap-0 mb-6 hairline-t hairline-b hairline-l hairline-r">
        {(Object.keys(LABELS) as Arch[]).map((a, i) => (
          <button
            key={a}
            onClick={() => setArch(a)}
            className={`px-3 py-1.5 font-mono text-[0.7rem] tracking-wider transition-colors ${
              i > 0 ? "hairline-l" : ""
            } ${arch === a ? "bg-ink text-paper" : "bg-paper hover:bg-paper-deep"}`}
          >
            {LABELS[a]}
          </button>
        ))}
      </div>

      <h2 className="font-display text-3xl mb-2">{heading}</h2>

      {/* Download button */}
      <div className="flex flex-wrap items-center gap-4 mt-6 mb-4">
        <a
          href={zip}
          className="inline-flex items-center gap-2 px-5 py-3 bg-ink text-paper font-mono text-sm tracking-wide hover:bg-indigo transition-colors"
          download
        >
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden>
            <path
              d="M8 1v9M4 7l4 4 4-4M2 12v2h12v-2"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
          {downloadLabel} (.zip)
        </a>

        <a
          href={sha}
          className="font-mono text-[0.7rem] uppercase tracking-wider text-ink-mute hover:text-indigo transition-colors"
        >
          {sha256Label} →
        </a>
      </div>

      {/* China mirror links */}
      <div className="mt-6">
        <div className="eyebrow mb-2">{mirrorHeading}</div>
        <div className="flex flex-wrap gap-3">
          <a
            href={ghproxy}
            className="inline-flex items-center gap-1.5 px-3 py-2 text-xs font-mono hairline-t hairline-b hairline-l hairline-r hover:bg-paper-deep transition-colors"
            rel="noopener noreferrer"
            target="_blank"
          >
            {mirrorGhproxy}
          </a>
          <span className="text-xs text-ink-mute self-center">
            {/* jsdelivr doesn't directly proxy GitHub Release assets; link to the release page instead */}
            <a
              href={`https://github.com/Hmbown/CodeWhale/releases/latest`}
              className="inline-flex items-center gap-1.5 px-3 py-2 text-xs font-mono hairline-t hairline-b hairline-l hairline-r hover:bg-paper-deep transition-colors"
              rel="noopener noreferrer"
              target="_blank"
            >
              {mirrorJsdelivr}
            </a>
          </span>
        </div>
      </div>

      {/* Offline callout */}
      <div className="mt-6 px-4 py-3 bg-indigo-pale text-sm leading-relaxed">
        <span className="font-display text-indigo mr-2">💡</span>
        {offlineCallout}
      </div>
    </div>
  );
}
