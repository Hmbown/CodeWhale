import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { ImageResponse } from "next/og";
import { OG_ALT } from "@/lib/page-meta";

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
          gap: 36,
          background: "#142352",
        }}
      >
        <img src={markDataUrl} width={220} height={220} alt="Codewhale whale mark" />
        <img src={wordmarkDataUrl} width={520} height={74} alt="Codewhale" />
      </div>
    ),
    { ...size },
  );
}
