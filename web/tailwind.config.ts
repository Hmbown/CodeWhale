import type { Config } from "tailwindcss";

export default {
  content: ["./app/**/*.{ts,tsx}", "./components/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // The surface and ink tokens resolve through CSS custom properties so
        // docs can still re-theme, while the shared accent palette stays oceanic.
        paper: "rgb(var(--c-paper) / <alpha-value>)",
        "paper-deep": "rgb(var(--c-paper-deep) / <alpha-value>)",
        "paper-edge": "rgb(var(--c-paper-edge) / <alpha-value>)",
        "paper-line": "#1B2230",
        "paper-line-soft": "#CBD3DF",
        ink: "rgb(var(--c-ink) / <alpha-value>)",
        "ink-soft": "rgb(var(--c-ink-soft) / <alpha-value>)",
        "ink-mute": "rgb(var(--c-ink-mute) / <alpha-value>)",
        indigo: "#355E70",
        "indigo-deep": "#274A5A",
        "indigo-pale": "#E2ECEF",
        ochre: "#B58B59",
        jade: "#18A78B",
        cobalt: "#1A2944",
      },
      fontFamily: {
        display: ['"Space Grotesk"', '"Noto Sans SC"', "ui-sans-serif", "system-ui", "sans-serif"],
        body: ['"IBM Plex Sans"', '"Noto Sans SC"', "ui-sans-serif", "system-ui", "sans-serif"],
        cjk: ['"Noto Serif SC"', '"Source Han Serif SC"', "serif"],
        mono: ['"JetBrains Mono"', "ui-monospace", "Menlo", "monospace"],
      },
      letterSpacing: {
        crisp: "-0.018em",
        wider: "0.08em",
        widest: "0.18em",
      },
    },
  },
  plugins: [],
} satisfies Config;
