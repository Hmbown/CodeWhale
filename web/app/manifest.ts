import type { MetadataRoute } from "next";
import { SITE_NAME } from "@/lib/page-meta";

/**
 * Web app manifest. Icons are the new Tideline whale mark: the white
 * silhouette on the deep-blue tile, rendered from the same master as
 * app/icon.svg. Static rasters live in public/ next to the other shipped
 * assets; the field naming follows the Next.js metadata-file convention
 * already used by app/icon.svg and app/opengraph-image.tsx.
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
    // The deep field and chrome of the site itself, so an installed shell
    // opens onto the same water as the page.
    theme_color: "#0e1729",
    background_color: "#03070d",
    display: "standalone",
  };
}
