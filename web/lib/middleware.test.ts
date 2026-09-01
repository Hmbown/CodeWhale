import { describe, expect, it } from "vitest";
import { NextRequest } from "next/server";
import { middleware } from "../middleware";

/**
 * `www.codewhale.net` and `codewhale.net` are both bound to this worker as
 * custom domains, so without a canonical-host redirect the whole site is
 * reachable — and indexable — twice.
 */
function request(url: string, host: string, headers: Record<string, string> = {}) {
  return new NextRequest(new URL(url), {
    headers: new Headers({ host, ...headers }),
  });
}

describe("canonical host", () => {
  it("301s www to the apex, preserving path and query", () => {
    const res = middleware(
      request("https://www.codewhale.net/en/docs/hooks?x=1", "www.codewhale.net"),
    );
    expect(res.status).toBe(301);
    expect(res.headers.get("location")).toBe("https://codewhale.net/en/docs/hooks?x=1");
  });

  it("redirects assets and API routes too, so a moved document stops pulling from www", () => {
    for (const path of ["/_next/static/chunk.js", "/api/curated", "/opengraph-image"]) {
      const res = middleware(request(`https://www.codewhale.net${path}`, "www.codewhale.net"));
      expect(res.status, path).toBe(301);
      expect(res.headers.get("location"), path).toBe(`https://codewhale.net${path}`);
    }
  });

  it("leaves the apex host alone", () => {
    const res = middleware(request("https://codewhale.net/en", "codewhale.net"));
    expect(res.status).not.toBe(301);
  });

  it("leaves localhost and preview hosts alone", () => {
    for (const host of ["localhost:3000", "codewhale-web.pages.dev"]) {
      const res = middleware(request("https://example.test/en", host));
      expect(res.status, host).not.toBe(301);
    }
  });

  it("still applies security headers to the redirect", () => {
    const res = middleware(request("https://www.codewhale.net/en", "www.codewhale.net"));
    expect(res.headers.get("X-Content-Type-Options")).toBe("nosniff");
    expect(res.headers.get("Strict-Transport-Security")).toContain("max-age=");
  });
});

describe("dotted well-known paths", () => {
  it("does not locale-prefix /llms.txt", () => {
    const res = middleware(request("https://codewhale.net/llms.txt", "codewhale.net"));
    expect(res.status).not.toBe(307);
    expect(res.status).not.toBe(308);
    expect(res.headers.get("location")).toBeNull();
  });
});

describe("public auth aliases (#5767)", () => {
  it("does not locale-prefix /auth/callback into a 404", () => {
    const res = middleware(
      request("https://codewhale.net/auth/callback?code=abc&state=1", "codewhale.net"),
    );
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe(
      "https://app.codewhale.net/auth/callback?code=abc&state=1",
    );
  });

  it("hops an already-localized callback to the CWC app", () => {
    const res = middleware(
      request("https://codewhale.net/en/auth/callback?code=abc", "codewhale.net"),
    );
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe(
      "https://app.codewhale.net/auth/callback?code=abc",
    );
  });

  it("folds /login onto /signin before locale prefixing", () => {
    const res = middleware(request("https://codewhale.net/login", "codewhale.net"));
    expect(res.status).toBe(308);
    expect(res.headers.get("location")).toBe("https://codewhale.net/signin");
  });

  it("folds a localized /register onto /signup", () => {
    const res = middleware(request("https://codewhale.net/zh/register", "codewhale.net"));
    expect(res.status).toBe(308);
    expect(res.headers.get("location")).toBe("https://codewhale.net/zh/signup");
  });

  it("locale-prefixes /signin and /signup onto real public pages", () => {
    const signin = middleware(
      request("https://codewhale.net/signin", "codewhale.net", {
        "accept-language": "ja,en;q=0.8",
      }),
    );
    expect(signin.status).toBe(307);
    expect(signin.headers.get("location")).toBe("https://codewhale.net/ja/signin");

    const signup = middleware(request("https://codewhale.net/signup", "codewhale.net"));
    expect(signup.status).toBe(307);
    expect(signup.headers.get("location")).toBe("https://codewhale.net/en/signup");
  });

  it("leaves an already-localized sign-in page unprefixed", () => {
    const res = middleware(request("https://codewhale.net/en/signin", "codewhale.net"));
    expect(res.status).not.toBe(307);
    expect(res.headers.get("location")).toBeNull();
  });
});

describe("locale prefix", () => {
  it("prefixes a bare path with the detected locale", () => {
    const res = middleware(
      request("https://codewhale.net/install", "codewhale.net", {
        "accept-language": "ja,en;q=0.8",
      }),
    );
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("https://codewhale.net/ja/install");
  });

  it("leaves an already-localized path, including pt-BR, unprefixed", () => {
    for (const path of ["/zh/install", "/pt-BR/docs/guide", "/de"]) {
      const res = middleware(request(`https://codewhale.net${path}`, "codewhale.net"));
      expect(res.status, path).not.toBe(307);
      expect(res.headers.get("location"), path).toBeNull();
    }
  });

  it("folds a miscased locale prefix onto its canonical spelling", () => {
    // Without this the segment reads as bare and the request lands on
    // `/en/pt-br/install`, which is a 404 rather than the Portuguese page.
    const res = middleware(
      request("https://codewhale.net/pt-br/install?x=1", "codewhale.net"),
    );
    expect(res.status).toBe(308);
    expect(res.headers.get("location")).toBe("https://codewhale.net/pt-BR/install?x=1");
  });

  it("settles after one canonicalizing redirect", () => {
    const res = middleware(request("https://codewhale.net/pt-BR/install", "codewhale.net"));
    expect(res.status).not.toBe(308);
    expect(res.headers.get("location")).toBeNull();
  });
});
