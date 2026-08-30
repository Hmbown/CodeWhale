"use client";

import { useEffect, useState } from "react";
import {
  detectFromBrowserSignals,
  type Arch,
  type UserAgentArchitecture,
} from "@/lib/install-platform";
import { SNIPPETS, VERIFY } from "@/lib/install-binary-snippets";
import { InstallCodeBlock } from "./install-code-block";

const LABELS: Record<Arch, string> = {
  "macos-arm64": "macOS · Apple Silicon",
  "macos-x64": "macOS · Intel",
  "linux-x64": "Linux · x64",
  "linux-arm64": "Linux · arm64",
  "windows-x64": "Windows · x64",
  "windows-arm64": "Windows · arm64",
};

interface NavigatorWithUserAgentData extends Navigator {
  userAgentData?: {
    getHighEntropyValues(hints: string[]): Promise<UserAgentArchitecture>;
  };
}

async function detect(): Promise<Arch> {
  if (typeof navigator === "undefined") return "macos-arm64";
  const browserNavigator = navigator as NavigatorWithUserAgentData;
  let architecture: UserAgentArchitecture | undefined;
  if (navigator.userAgent.toLowerCase().includes("win")) {
    try {
      architecture = await browserNavigator.userAgentData?.getHighEntropyValues([
        "architecture",
        "bitness",
      ]);
    } catch {
      // The manual platform buttons and frozen-UA fallback remain available.
    }
  }
  return detectFromBrowserSignals(navigator.userAgent, architecture);
}

interface Props {
  copyLabel?: string;
  copiedLabel?: string;
  verifyHeading?: string;
}

export function InstallBinary({ copyLabel, copiedLabel, verifyHeading = "Verify checksum" }: Props) {
  const [arch, setArch] = useState<Arch>("macos-arm64");

  useEffect(() => {
    let active = true;
    void detect().then((detected) => {
      if (active) setArch(detected);
    });
    return () => {
      active = false;
    };
  }, []);

  return (
    <div>
      <div className="flex flex-wrap gap-0 mb-3 hairline-t hairline-b hairline-l hairline-r">
        {(Object.keys(SNIPPETS) as Arch[]).map((a, i) => (
          <button
            key={a}
            type="button"
            onClick={() => setArch(a)}
            aria-pressed={arch === a}
            className={`px-3 py-1.5 font-mono text-[0.7rem] tracking-wider transition-colors ${
              i > 0 ? "hairline-l" : ""
            } ${arch === a ? "bg-indigo text-paper" : "bg-paper hover:bg-paper-deep"}`}
          >
            {LABELS[a]}
          </button>
        ))}
      </div>

      <InstallCodeBlock cmd={SNIPPETS[arch]} copyLabel={copyLabel} copiedLabel={copiedLabel} />

      <div className="mt-4">
        <div className="eyebrow mb-2">{verifyHeading}</div>
        <InstallCodeBlock cmd={VERIFY[arch]} copyLabel={copyLabel} copiedLabel={copiedLabel} />
      </div>
    </div>
  );
}
