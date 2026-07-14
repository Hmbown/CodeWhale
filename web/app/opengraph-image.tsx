import { ImageResponse } from "next/og";
import { IDENTITY_PHRASE, SITE_NAME } from "@/lib/page-meta";

export const alt = `${SITE_NAME} — ${IDENTITY_PHRASE}`;
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

// The social card mirrors the restrained light-to-depth website treatment.
export default function OpengraphImage() {
  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          flexDirection: "column",
          justifyContent: "space-between",
          backgroundColor: "#F7F8FA",
          padding: "72px 84px",
          fontFamily: "sans-serif",
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 16,
            fontSize: 26,
            letterSpacing: 0,
            textTransform: "uppercase",
            color: "#69748A",
          }}
        >
          <div style={{ width: 28, height: 14, borderRadius: "50% 45% 45% 50%", backgroundColor: "#F6C453" }} />
          codewhale.net
        </div>
        <div style={{ display: "flex", flexDirection: "column" }}>
          <div
            style={{
              fontSize: 116,
              fontWeight: 700,
              color: "#1B2230",
              letterSpacing: 0,
            }}
          >
            Codewhale
          </div>
          <div
            style={{
              marginTop: 28,
              fontSize: 38,
              lineHeight: 1.35,
              color: "#4C5567",
              maxWidth: 980,
            }}
          >
            {IDENTITY_PHRASE}
          </div>
        </div>
        <div style={{ display: "flex", width: "100%", height: 14, backgroundColor: "#081221" }}>
          <div style={{ width: "32%", height: "100%", backgroundColor: "#9FC5D2" }} />
        </div>
      </div>
    ),
    { ...size },
  );
}
