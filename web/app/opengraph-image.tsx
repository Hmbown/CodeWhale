import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { ImageResponse } from "next/og";
import { IDENTITY_PHRASE, OG_ALT } from "@/lib/page-meta";

export const alt = OG_ALT;
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

const mark = readFile(join(process.cwd(), "public/brand/mark.svg")).then((svg) =>
  svg.toString().replace("currentColor", "#ffffff"),
);
const wordmark = readFile(join(process.cwd(), "public/brand/wordmark-inverted.svg")).then(
  (svg) => svg.toString(),
);

export default async function OpengraphImage() {
  const [markSvg, wordmarkSvg] = await Promise.all([mark, wordmark]);
  const markDataUrl = `data:image/svg+xml;base64,${Buffer.from(markSvg).toString("base64")}`;
  const wordmarkDataUrl = `data:image/svg+xml;base64,${Buffer.from(wordmarkSvg).toString("base64")}`;

  // Brand navy ground, the white mark and inverted wordmark, and the identity
  // phrase once — the wordmark is the name, so no second "Codewhale" heading.
  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          gap: 34,
          background: "#142352",
          fontFamily: "sans-serif",
        }}
      >
        <img src={markDataUrl} width={200} height={200} alt="" />
        {/* The traced wordmark is 1874x264 (~7.1:1). */}
        <img src={wordmarkDataUrl} width={532} height={75} alt="Codewhale" />
        <div style={{ display: "flex", fontSize: 30, color: "#F6F2E8", marginTop: 14 }}>
          {IDENTITY_PHRASE}
        </div>
      </div>
    ),
    { ...size },
  );
}
