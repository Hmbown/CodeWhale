import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { APP_LOGIN_URL, APP_SIGNUP_URL, APP_URL } from "./i18n/links";
import {
  APP_AUTH_CALLBACK_URL,
  CANONICAL_MARK_SHA256,
  CANONICAL_MARK_SRC,
  canonicalPublicAuthPath,
  publicAuthAppDestination,
  publicAuthCallbackDestination,
  publicAuthKind,
  publicAuthRemainder,
} from "./public-auth-routes";

describe("public auth remainders and kinds", () => {
  it("recognizes the observed 404 paths with and without a locale prefix", () => {
    expect(publicAuthRemainder("/signin")).toBe("signin");
    expect(publicAuthRemainder("/en/signin")).toBe("signin");
    expect(publicAuthRemainder("/pt-BR/signup")).toBe("signup");
    expect(publicAuthRemainder("/zh/auth/callback")).toBe("auth/callback");
    expect(publicAuthKind("/signin")).toBe("sign-in");
    expect(publicAuthKind("/en/signin")).toBe("sign-in");
    expect(publicAuthKind("/signup")).toBe("sign-up");
    expect(publicAuthKind("/ja/signup")).toBe("sign-up");
    expect(publicAuthKind("/auth/callback")).toBe("callback");
    expect(publicAuthKind("/en/auth/callback")).toBe("callback");
    expect(publicAuthKind("/install")).toBeNull();
    expect(publicAuthKind("/en/docs")).toBeNull();
  });

  it("folds login/register aliases onto the public sign-in and create-account routes", () => {
    expect(canonicalPublicAuthPath("/login")).toBe("/signin");
    expect(canonicalPublicAuthPath("/en/login")).toBe("/en/signin");
    expect(canonicalPublicAuthPath("/register")).toBe("/signup");
    expect(canonicalPublicAuthPath("/zh/register")).toBe("/zh/signup");
    expect(canonicalPublicAuthPath("/create-account")).toBe("/signup");
    expect(canonicalPublicAuthPath("/signin")).toBeNull();
    expect(canonicalPublicAuthPath("/en/signup")).toBeNull();
  });
});

describe("CWC destinations", () => {
  it("sends English (and other) public pages to the default app entry", () => {
    expect(publicAuthAppDestination("sign-in", "en")).toBe(APP_LOGIN_URL);
    expect(publicAuthAppDestination("sign-up", "ja")).toBe(APP_SIGNUP_URL);
    expect(publicAuthAppDestination("sign-in", "zh")).toBe(`${APP_URL}/zh/login`);
    expect(publicAuthAppDestination("sign-up", "zh")).toBe(`${APP_URL}/zh/signup`);
  });

  it("hops auth callbacks to the CWC app and preserves the query string", () => {
    expect(APP_AUTH_CALLBACK_URL).toBe(`${APP_URL}/auth/callback`);
    expect(
      publicAuthCallbackDestination(new URL("https://codewhale.net/auth/callback?code=abc&state=1")),
    ).toBe(`${APP_AUTH_CALLBACK_URL}?code=abc&state=1`);
    expect(
      publicAuthCallbackDestination(new URL("https://codewhale.net/en/auth/callback?code=abc")),
    ).toBe(`${APP_AUTH_CALLBACK_URL}?code=abc`);
    expect(
      publicAuthCallbackDestination(new URL("https://codewhale.net/en/signin")),
    ).toBeNull();
  });
});

describe("canonical mark", () => {
  it("ships the pinned raster generated from the canonical vector", () => {
    const bytes = readFileSync(new URL("../public/brand/codewhale-mark.png", import.meta.url));
    expect(createHash("sha256").update(bytes).digest("hex")).toBe(CANONICAL_MARK_SHA256);
    expect(CANONICAL_MARK_SRC).toBe("/brand/codewhale-mark.png");
  });
});
