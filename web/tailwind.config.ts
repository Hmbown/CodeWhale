import type { Config } from "tailwindcss";

export default {
  content: ["./app/**/*.{ts,tsx}", "./components/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // DeepSeek-aligned palette: cool white + soft gray, indigo accents.
        // (Previous warm cream `#F4F1E8` read too "Anthropic-like".)
        //
        // The surface/ink tokens resolve through CSS custom properties (RGB
        // channel triples) so they can be re-themed per subtree. In :root the
        // channels hold the exact light values below, so light-mode output is
        // unchanged; the docs routes override them for dark mode (globals.css).
        paper: "rgb(var(--c-paper) / <alpha-value>)",
        "paper-deep": "rgb(var(--c-paper-deep) / <alpha-value>)",
        "paper-edge": "rgb(var(--c-paper-edge) / <alpha-value>)",
        "paper-line": "#0E0E10",
        "paper-line-soft": "#D4D8E2",
        ink: "rgb(var(--c-ink) / <alpha-value>)",
        "ink-soft": "rgb(var(--c-ink-soft) / <alpha-value>)",
        "ink-mute": "rgb(var(--c-ink-mute) / <alpha-value>)",
        indigo: "#4D6BFE",
        "indigo-deep": "#3A52CC",
        "indigo-pale": "#E9EEFE",
        ochre: "#9C7A3F",
        jade: "#0AB68B",
        cobalt: "#1F3A8A",
      },
      fontFamily: {
        display: ['"Fraunces"', '"Noto Serif SC"', "ui-serif", "Georgia", "serif"],
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
