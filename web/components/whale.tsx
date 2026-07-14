/**
 * Static Codewhale mark shared with the managed product family.
 * The public site keeps it still and uses one Signal Gold instance per surface.
 */
export function Whale({ size = 36, className = "" }: { size?: number; className?: string }) {
  return (
    <svg
      viewBox="0 0 230 135"
      width={size}
      height={(size * 135) / 230}
      className={className}
      aria-hidden="true"
      fill="none"
    >
      <path fill="currentColor" d="M30 47c18-12 51-13 83-7 22 4 40 12 57 19 4 15 0 29-12 38-14 11-43 15-76 12-30-3-54-11-64-24-9-12-5-27 12-38Z" />
      <path fill="currentColor" d="M153 65c16-2 26-10 32-21 6-10 7-21 3-31 12 4 20 13 20 25 7-7 15-10 23-8-3 14-12 24-25 30-7 3-14 3-21 0-6 12-17 20-31 24 4-7 4-13-1-19Z" />
      <path fill="currentColor" d="M86 42c7-16 16-24 29-25-1 11-6 20-16 27Z" />
      <path fill="currentColor" d="M108 99c13 0 23 6 30 17-15 2-26-2-34-12Z" />
      <path d="M101 64c-11-7-29-7-39 0-9 6-8 16 2 21 11 6 28 3 37-5" stroke="#08111c" strokeWidth="7.5" strokeLinecap="round" />
      <path d="m106 67 8 18 9-16 9 16 11-20" stroke="#08111c" strokeWidth="6.5" strokeLinecap="round" strokeLinejoin="round" />
      <circle cx="32" cy="64" r="3.6" fill="#08111c" />
      <path d="M28 81c7 5 16 5 23 1" stroke="#08111c" strokeWidth="2.5" strokeLinecap="round" opacity="0.8" />
      <path d="M39 43V29m9 14 6-11M31 43l-6-9" stroke="currentColor" strokeWidth="3.5" strokeLinecap="round" />
    </svg>
  );
}
