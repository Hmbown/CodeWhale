import { describe, it, expect } from "vitest";

describe("smoke", () => {
  it("vitest runs", () => {
    expect(1 + 1).toBe(2);
  });

  it("jsdom environment is available", () => {
    expect(typeof window).toBe("object");
    expect(typeof document).toBe("object");
    expect(document.createElement("div")).toBeInstanceOf(HTMLDivElement);
  });
});
