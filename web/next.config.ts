import type { NextConfig } from "next";

// Security headers are set in middleware.ts (more reliable under OpenNext on
// Cloudflare than next.config.ts headers(), which doesn't always apply to
// prerendered/cached responses).
const nextConfig: NextConfig = {
  reactStrictMode: true,
  images: {
    remotePatterns: [
      { protocol: "https", hostname: "avatars.githubusercontent.com" },
    ],
  },
  typedRoutes: false,
};

export default nextConfig;

if (process.env.NODE_ENV === "development") {
  // Initialize Cloudflare bindings (KV, etc.) when running `next dev`.
  // No-op in production builds.
  void import("@opennextjs/cloudflare").then(({ initOpenNextCloudflareForDev }) => {
    initOpenNextCloudflareForDev();
  }).catch(() => { /* dev-only convenience */ });
}
