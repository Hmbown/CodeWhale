import type { Config } from "tailwindcss";

export default {
  content: ["./app/**/*.{ts,tsx}", "./components/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // The surface, ink, and accent tokens all resolve through CSS custom
        // properties so the docs light sheet can re-theme the subtree, while
        // the default values stay the Tideline dark whale palette.
        paper: "rgb(var(--c-paper) / <alpha-value>)",
        "paper-deep": "rgb(var(--c-paper-deep) / <alpha-value>)",
        "paper-edge": "rgb(var(--c-paper-edge) / <alpha-value>)",
        "paper-card": "var(--paper-card)",
        "paper-line": "var(--paper-line)",
        "paper-line-soft": "var(--paper-line-soft)",
        ink: "rgb(var(--c-ink) / <alpha-value>)",
        "ink-soft": "rgb(var(--c-ink-soft) / <alpha-value>)",
        "ink-mute": "rgb(var(--c-ink-mute) / <alpha-value>)",
        indigo: "rgb(var(--c-indigo) / <alpha-value>)",
        "indigo-deep": "rgb(var(--c-indigo-deep) / <alpha-value>)",
        "indigo-pale": "var(--indigo-pale)",
        ochre: "var(--ochre)",
        jade: "var(--jade)",
        cobalt: "var(--cobalt)",
      },
      fontFamily: {
        // Display and body are one instrument voice — system sans at heading
        // weight and tracking (globals.css resolves --font-display to
        // --font-body). Mono stays JetBrains Mono, loaded by next/font in
        // app/[locale]/layout.tsx.
        display: ["var(--font-display)", "ui-sans-serif", "system-ui", "sans-serif"],
        body: ["var(--font-body)", '"IBM Plex Sans"', '"Noto Sans SC"', "ui-sans-serif", "system-ui", "sans-serif"],
        cjk: ["var(--font-cjk)", '"PingFang SC"', '"Source Han Serif SC"', "serif"],
        mono: ["var(--font-mono)", '"JetBrains Mono"', "ui-monospace", "Menlo", "monospace"],
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
