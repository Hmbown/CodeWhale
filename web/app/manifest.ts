import type { MetadataRoute } from "next";
import { SITE_NAME } from "@/lib/page-meta";

/**
 * Web app manifest. Icons are the founder's whale mark in white on the
 * #142352 navy tile, rasterised from app/icon.svg by
 * scripts/brand/trace-brand.py. Static rasters live in public/ next to the
 * other shipped assets; the field naming follows the Next.js metadata-file
 * convention already used by app/icon.svg and app/opengraph-image.tsx.
 */
export default function manifest(): MetadataRoute.Manifest {
  return {
    name: SITE_NAME,
    short_name: SITE_NAME,
    icons: [
      {
        src: "/icon-192.png",
        sizes: "192x192",
        type: "image/png",
      },
      {
        src: "/icon-512.png",
        sizes: "512x512",
        type: "image/png",
      },
    ],
    // Brand navy: the icon tile, so the installed shell's splash and chrome
    // are one surface with the mark.
    theme_color: "#142352",
    background_color: "#142352",
    display: "standalone",
  };
}
