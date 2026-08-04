/** Static Signal Current mark from the managed Codewhale product contract. */
const WHALE_BODY =
  "M7 57c9-13 21-15 25-25 3-7-1-12-10-16 6-1 11 1 14 6 3-6 10-11 19-13-1 10-5 17-12 21-4 3-5 8-8 14-5 9-15 14-28 13Z";
const WHALE_CURRENT = "M28 58c10-8 15-15 17-26 4 8 2 18-3 26H28Z";

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
      viewBox="0 0 64 64"
      width={size}
      height={size}
      className={`codewhale-mark ${className}`}
      aria-hidden="true"
      fill="none"
    >
      {caustic ? (
        <defs>
          <clipPath id="codewhale-caustic-clip">
            <path d={WHALE_BODY} />
            <path d={WHALE_CURRENT} />
          </clipPath>
          <linearGradient id="codewhale-caustic-light" x1="0" y1="0" x2="1" y2="0">
            <stop offset="0%" stopColor="#d1ebf4" stopOpacity="0" />
            <stop offset="50%" stopColor="#d1ebf4" stopOpacity="0.33" />
            <stop offset="100%" stopColor="#d1ebf4" stopOpacity="0" />
          </linearGradient>
        </defs>
      ) : null}

      <path className="codewhale-mark-primary" d={WHALE_BODY} />
      <path className="codewhale-mark-current" d={WHALE_CURRENT} />

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
            x="-56"
            y="0"
            width="48"
            height="64"
            fill="url(#codewhale-caustic-light)"
          />
        </g>
      ) : null}
    </svg>
  );
}
