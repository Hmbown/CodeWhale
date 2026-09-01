/* codewhale-brand: an explicit identity lockup for the DSH skin.
 *
 * DSH renders this component through its additive shell.overlay Slot. The
 * lockup is pointer-inert, compact on narrow viewports, uses the active skin
 * tokens, and leaves DSH-owned chrome untouched.
 */
function CodewhaleBrand() {
	var css =
		"#codewhale-brand-lockup{" +
		"position:fixed;top:18px;right:20px;z-index:40;display:flex;align-items:center;gap:10px;min-width:272px;padding:10px 12px;box-sizing:border-box;pointer-events:none;user-select:none;border:1px solid var(--dsw-alias-brand-primary,#6aaef2);border-left:3px solid var(--dsw-alias-state-business-primary,#f6c453);border-radius:12px;background:var(--dsw-alias-bg-layer-1,#0e1729);color:var(--dsw-alias-label-primary,#f6f2e8);box-shadow:0 18px 44px rgba(0,0,0,.38),inset 0 1px 0 rgba(255,255,255,.07);font-family:Inter,ui-sans-serif,-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif;line-height:1;" +
		"}" +
		"#codewhale-brand-mark{" +
		"display:grid;place-items:center;width:34px;height:34px;flex:0 0 auto;border-radius:7px;background:#052366;box-shadow:0 8px 22px rgba(0,0,0,.42);" +
		"}" +
		"#codewhale-brand-mark svg{display:block;width:34px;height:34px;}" +
		"#codewhale-brand-copy{display:flex;min-width:0;flex:1;flex-direction:column;gap:5px;}" +
		"#codewhale-brand-kicker{display:block;color:var(--dsw-alias-state-business-primary,#f6c453);font-size:9px;font-weight:800;letter-spacing:2.1px;white-space:nowrap;}" +
		"#codewhale-brand-name{display:block;color:var(--dsw-alias-label-primary,#f6f2e8);font-size:15px;font-weight:850;letter-spacing:1.7px;white-space:nowrap;}" +
		"#codewhale-brand-bridge{display:block;margin-left:auto;color:var(--dsw-alias-brand-primary,#6aaef2);font-family:ui-monospace,\"SFMono-Regular\",Menlo,Consolas,monospace;font-size:9px;font-weight:750;letter-spacing:.8px;white-space:nowrap;}" +
		"@media(max-width:759px){" +
		"#codewhale-brand-lockup{top:8px;right:8px;min-width:0;padding:7px 9px;gap:7px;}" +
		"#codewhale-brand-mark{width:28px;height:28px;border-radius:6px;}" +
		"#codewhale-brand-mark svg{width:28px;height:28px;}" +
		"#codewhale-brand-kicker,#codewhale-brand-bridge{display:none;}" +
		"}";

	return React.createElement(
		React.Fragment,
		null,
		React.createElement("style", { "data-codewhale-brand-style": true }, css),
		React.createElement(
			"aside",
			{
				id: "codewhale-brand-lockup",
				"aria-label": "Whale Brothers. Codewhale connected to DeepSeek Harness.",
			},
			React.createElement(
				"span",
				{ id: "codewhale-brand-mark", "aria-hidden": true },
				// The Codewhale mark: white whale silhouette on the deep-blue tile.
				// Same gradient and traced path as web/app/icon.svg; keep in step.
				// No emoji — the identity is a real inline SVG.
				React.createElement(
					"svg",
					{ viewBox: "0 0 1254 1254", width: "34", height: "34" },
					React.createElement(
						"defs",
						null,
						React.createElement(
							"linearGradient",
							{ id: "codewhale-brand-tile", x1: "0", y1: "0", x2: "0.55", y2: "1" },
							React.createElement("stop", { offset: "0", stopColor: "#1D408A" }),
							React.createElement("stop", { offset: "1", stopColor: "#052366" }),
						),
					),
					React.createElement("rect", {
						width: "1254",
						height: "1254",
						rx: "247",
						fill: "url(#codewhale-brand-tile)",
					}),
					React.createElement("path", {
						fill: "#ffffff",
						transform: "translate(197 173) translate(0 904) scale(0.1 -0.1)",
						d: "M5351 8953 c-174 -358 -474 -544 -1148 -713 -707 -176 -1031 -417 -1164 -862 -11 -40 -23 -77 -26 -82 -3 -4 -42 29 -87 76 -318 327 -591 401 -1321 353 -711 -47 -1078 48 -1421 367 -93 87 -93 87 -120 57 -28 -31 -24 -169 10 -334 143 -690 584 -1142 1240 -1270 165 -33 322 -43 721 -50 467 -7 597 -24 753 -101 252 -123 333 -343 298 -814 -39 -532 -41 -696 -15 -1050 55 -732 286 -1413 681 -2002 70 -104 69 -95 11 -102 -912 -117 -927 -123 -767 -332 258 -338 747 -544 1302 -550 l252 -2 103 -94 c1220 -1119 2384 -1604 3220 -1342 326 102 482 241 577 509 312 887 49 1756 -785 2591 -514 516 -1014 849 -2035 1357 -910 452 -1205 640 -1540 976 -426 427 -512 802 -248 1072 89 92 205 165 483 306 771 390 1070 709 1186 1263 75 362 -55 990 -160 773z",
					}),
				),
			),
			React.createElement(
				"span",
				{ id: "codewhale-brand-copy" },
				React.createElement("span", { id: "codewhale-brand-kicker" }, "WHALE BROTHERS"),
				React.createElement("span", { id: "codewhale-brand-name" }, "CODEWHALE"),
			),
			React.createElement("span", { id: "codewhale-brand-bridge" }, "\u00d7 DEEPSEEK HARNESS"),
		),
	);
}
