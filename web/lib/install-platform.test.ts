import { describe, expect, it } from "vitest";
import { detectFromBrowserSignals } from "./install-platform";

describe("install platform detection", () => {
  const frozenWindowsUa =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";

  it("uses User-Agent Client Hints for native Windows ARM64", () => {
    expect(
      detectFromBrowserSignals(frozenWindowsUa, {
        architecture: "arm",
        bitness: "64",
      }),
    ).toBe("windows-arm64");
  });

  it("keeps frozen Windows user agents on x64 without an ARM hint", () => {
    expect(detectFromBrowserSignals(frozenWindowsUa)).toBe("windows-x64");
  });

  it("retains the legacy ARM token fallback", () => {
    expect(detectFromBrowserSignals("Mozilla/5.0 (Windows NT 10.0; ARM64)")).toBe(
      "windows-arm64",
    );
  });

  const frozenMacUa =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";

  it("detects Intel Macs through User-Agent Client Hints (#5168)", () => {
    expect(
      detectFromBrowserSignals(frozenMacUa, {
        architecture: "x86",
        bitness: "64",
      }),
    ).toBe("macos-x64");
  });

  it("detects Apple Silicon through User-Agent Client Hints", () => {
    expect(
      detectFromBrowserSignals(frozenMacUa, {
        architecture: "arm",
        bitness: "64",
      }),
    ).toBe("macos-arm64");
  });

  it("defaults to arm64 without hints because the frozen Mac UA cannot distinguish", () => {
    // Since Big Sur the UA says "Intel Mac OS X" on Apple Silicon too, so
    // UA parsing must not route anyone to x64. The install page's arch
    // chooser is the honest fallback for Intel users without hints.
    expect(detectFromBrowserSignals(frozenMacUa)).toBe("macos-arm64");
  });
});
