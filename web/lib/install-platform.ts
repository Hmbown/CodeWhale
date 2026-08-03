export type Arch =
  | "macos-arm64"
  | "macos-x64"
  | "linux-x64"
  | "linux-arm64"
  | "windows-x64"
  | "windows-arm64";

export interface UserAgentArchitecture {
  architecture?: string;
  bitness?: string;
}

export function detectFromBrowserSignals(
  userAgent: string,
  userAgentArchitecture?: UserAgentArchitecture,
): Arch {
  const ua = userAgent.toLowerCase();
  const architecture = userAgentArchitecture?.architecture?.toLowerCase();
  const bitness = userAgentArchitecture?.bitness;
  if (ua.includes("win")) {
    if (
      architecture === "arm64" ||
      (architecture === "arm" && bitness === "64") ||
      ua.includes("aarch64") ||
      ua.includes("arm64")
    ) {
      return "windows-arm64";
    }
    return "windows-x64";
  }
  if (ua.includes("linux")) {
    if (ua.includes("aarch64") || ua.includes("arm64")) return "linux-arm64";
    return "linux-x64";
  }
  // macOS. Since Big Sur the UA reports "Intel Mac OS X" on Apple Silicon
  // too, so the UA string cannot distinguish architectures — only
  // User-Agent Client Hints can (#5168). Without hints we default to arm64
  // (every Mac sold since late 2020); the arch chooser on the install page
  // stays the honest fallback for Intel users.
  if (architecture === "x86") return "macos-x64";
  return "macos-arm64";
}
