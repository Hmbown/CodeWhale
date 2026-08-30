/**
 * The Codewhale brand mark — the white whale silhouette traced from the
 * 1254×1254 logo master (rounded-square tile lives in app/icon.svg; this is
 * the bare silhouette for in-page chrome). The path keeps the master's
 * coordinate frame: potrace units, y-up, offset by the silhouette's bounding
 * origin, framed by the viewBox below.
 */
const WHALE_MARK =
  "M5351 8953 c-174 -358 -474 -544 -1148 -713 -707 -176 -1031 -417 -1164 -862 -11 -40 -23 -77 -26 -82 -3 -4 -42 29 -87 76 -318 327 -591 401 -1321 353 -711 -47 -1078 48 -1421 367 -93 87 -93 87 -120 57 -28 -31 -24 -169 10 -334 143 -690 584 -1142 1240 -1270 165 -33 322 -43 721 -50 467 -7 597 -24 753 -101 252 -123 333 -343 298 -814 -39 -532 -41 -696 -15 -1050 55 -732 286 -1413 681 -2002 70 -104 69 -95 11 -102 -912 -117 -927 -123 -767 -332 258 -338 747 -544 1302 -550 l252 -2 103 -94 c1220 -1119 2384 -1604 3220 -1342 326 102 482 241 577 509 312 887 49 1756 -785 2591 -514 516 -1014 849 -2035 1357 -910 452 -1205 640 -1540 976 -426 427 -512 802 -248 1072 89 92 205 165 483 306 771 390 1070 709 1186 1263 75 362 -55 990 -160 773z";

/* Silhouette bounds in master coordinates (x 201–1054, y 177–1072), with a
   small margin so the fluke never kisses the frame at favicon sizes. */
const VIEW_BOX = "170 148 915 950";
const MASTER_TRANSFORM = "translate(197 173) translate(0 904) scale(0.1 -0.1)";

export function Whale({
  size = 36,
  className = "",
  caustic = false,
}: {
  size?: number;
  className?: string;
  /**
   * Ambient light passing over the mark — `ambient_life.rs`'s caustic at its
   * literal amplitude and cadence. Exactly one whale on the page may carry it
   * (the footer's); two caustics is chrome. The fixed gradient/clip ids are
   * safe for the same reason.
   */
  caustic?: boolean;
}) {
  return (
    <svg
      viewBox={VIEW_BOX}
      width={size}
      height={size}
      className={`codewhale-mark ${className}`}
      aria-hidden="true"
      fill="none"
    >
      {caustic ? (
        <defs>
          <clipPath id="codewhale-caustic-clip">
            <g transform={MASTER_TRANSFORM}>
              <path d={WHALE_MARK} />
            </g>
          </clipPath>
          <linearGradient id="codewhale-caustic-light" x1="0" y1="0" x2="1" y2="0">
            <stop offset="0%" stopColor="#d1ebf4" stopOpacity="0" />
            <stop offset="50%" stopColor="#d1ebf4" stopOpacity="0.33" />
            <stop offset="100%" stopColor="#d1ebf4" stopOpacity="0" />
          </linearGradient>
        </defs>
      ) : null}

      <g transform={MASTER_TRANSFORM}>
        <path className="codewhale-mark-primary" d={WHALE_MARK} />
      </g>

      {/* The light is clipped to the mark itself, so it lands on the whale and
          never on a rectangle of footer behind it. The clip lives on a STATIC
          group and the highlight moves inside it — put the clip on the moving
          rect and the whale-shaped window slides along with the light, which
          is a very quiet way to render nothing at all. Parked off-canvas at
          rest, which is also where reduced motion leaves it. */}
      {caustic ? (
        <g clipPath="url(#codewhale-caustic-clip)">
          <rect
            className="codewhale-caustic"
            x="-460"
            y="148"
            width="380"
            height="950"
            fill="url(#codewhale-caustic-light)"
          />
        </g>
      ) : null}
    </svg>
  );
}
