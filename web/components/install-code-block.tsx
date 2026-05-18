"use client";

import { useState } from "react";

interface Props {
  cmd: string;
  copyLabel?: string;
  copiedLabel?: string;
}

export function InstallCodeBlock({ cmd, copyLabel = "Copy", copiedLabel = "Copied ✓" }: Props) {
  const [copied, setCopied] = useState(false);

  const copy = () => {
    if (typeof navigator !== "undefined" && navigator.clipboard) {
      navigator.clipboard.writeText(cmd);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    }
  };

  return (
    <div className="relative">
      <button
        onClick={copy}
        aria-label={copied ? copiedLabel : copyLabel}
        className="absolute top-3 right-3 z-10 px-3 py-1 bg-paper hairline-t hairline-b hairline-l hairline-r font-mono text-[0.7rem] uppercase tracking-wider hover:bg-indigo hover:text-paper transition-colors"
      >
        {copied ? copiedLabel : copyLabel}
      </button>
      <pre className="code-block text-[0.78rem] m-0 max-w-full pr-20">{cmd}</pre>
    </div>
  );
}
