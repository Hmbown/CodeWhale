import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { ImageResponse } from "next/og";
import { IDENTITY_PHRASE, OG_ALT } from "@/lib/page-meta";

export const alt = OG_ALT;
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

// The social card is the new brand mark on its own water: the deep-blue tile
// gradient from the logo master, the white whale, and the wordmark. The tile
// is the shipped 512px icon itself, read at build time so the card can never
// drift from the favicon set.
export default async function OpengraphImage() {
  const icon = await readFile(join(process.cwd(), "public/icon-512.png"));
  const iconDataUrl = `data:image/png;base64,${icon.toString("base64")}`;

  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          flexDirection: "column",
          justifyContent: "space-between",
          background: "linear-gradient(115deg, #1D408A 0%, #062C7F 48%, #052366 100%)",
          padding: "72px 84px",
          fontFamily: "sans-serif",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 28 }}>
          <img
            src={iconDataUrl}
            width={124}
            height={124}
            style={{ borderRadius: 26 }}
            alt=""
          />
          <div
            style={{
              display: "flex",
              fontSize: 34,
              fontWeight: 700,
              letterSpacing: 10,
              textTransform: "uppercase",
              color: "#FFFFFF",
            }}
          >
            Codewhale
          </div>
        </div>
        <div
          style={{
            display: "flex",
            fontSize: 58,
            fontWeight: 650,
            lineHeight: 1.22,
            letterSpacing: -1,
            color: "#F6F2E8",
            maxWidth: 960,
          }}
        >
          {IDENTITY_PHRASE}
        </div>
        <div
          style={{
            display: "flex",
            fontSize: 24,
            color: "#48D7FF",
            letterSpacing: 2,
          }}
        >
          codewhale.net
        </div>
      </div>
    ),
    { ...size },
  );
}
