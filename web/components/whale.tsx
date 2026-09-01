/**
 * The Codewhale brand mark — side-view whale facing left, `>` prompt-eye
 * cutout (even-odd), belly crease, upward fluke. Matches `web/brand/mark/whale.svg`.
 */
const WHALE_MARK =
  "M10 50 C10 34 24 26 44 28 C58 29 70 36 76 46 C80 52 78 60 70 66 L86 48 C92 42 100 48 96 62 C92 78 78 86 62 82 C54 88 36 88 20 74 C8 64 10 56 10 50 Z M22 40 L36 50 L22 60 L26 50 Z M30 70 C38 74 48 74 52 70 C46 72 38 72 30 70 Z";

const VIEW_BOX = "0 0 100 100";

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
            <path d={WHALE_MARK} fillRule="evenodd" />
          </clipPath>
          <linearGradient id="codewhale-caustic-light" x1="0" y1="0" x2="1" y2="0">
            <stop offset="0%" stopColor="#d1ebf4" stopOpacity="0" />
            <stop offset="50%" stopColor="#d1ebf4" stopOpacity="0.33" />
            <stop offset="100%" stopColor="#d1ebf4" stopOpacity="0" />
          </linearGradient>
        </defs>
      ) : null}

      <path
        className="codewhale-mark-primary"
        d={WHALE_MARK}
        fill="currentColor"
        fillRule="evenodd"
      />

      {caustic ? (
        <g clipPath="url(#codewhale-caustic-clip)">
          <rect
            className="codewhale-caustic"
            x="-40"
            y="0"
            width="40"
            height="100"
            fill="url(#codewhale-caustic-light)"
          />
        </g>
      ) : null}
    </svg>
  );
}
