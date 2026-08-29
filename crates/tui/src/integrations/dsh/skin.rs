//! Codewhale palette for the DeepSeek Harness web surface.
//!
//! DSH 0.1.0-rc.6 applies alias tokens as inline `body.style.setProperty`
//! values, so a stylesheet cannot win. The documented token-level override is
//! `ctx.theme.overrideTokens`. This module is the single source of truth for
//! that layer: alias name → (light, dark), both rendered from the TUI Blue
//! Stage palettes. The generated bundle also mounts a plugin-owned Whale
//! Brothers / Codewhale identity lockup; it leaves DSH-owned branding intact.

use std::collections::BTreeMap;

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use crate::palette::{
    LIGHT_PANEL, LIGHT_SURFACE, LIGHT_UI_THEME, UI_THEME, UiTheme, WHALE_BG, WHALE_CHROME,
    WHALE_COMPOSER, WHALE_PANEL, hex_rgb_string,
};

/// Source id passed to `overrideTokens` and used as the client module id.
pub(crate) const SKIN_SOURCE: &str = "codewhale-dsh-bundle";

/// One alias token's light/dark CSS values, rendered from the TUI palette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SkinTokens {
    pub(crate) light: String,
    pub(crate) dark: String,
}

fn hex(color: Color) -> String {
    hex_rgb_string(color).unwrap_or_else(|| "inherit".to_string())
}

/// Opaque Blue Stage palettes for the browser-owned DSH surface.
///
/// The native TUI's Whale presets deliberately leave ordinary Flat shell
/// surfaces as `Color::Reset` so the terminal owns them. DSH is a browser
/// surface with no terminal background to inherit, so its token override and
/// optional ocean veil must restore the same concrete palette colors instead
/// of serializing `Reset` as CSS `inherit`.
pub(super) fn browser_themes() -> (UiTheme, UiTheme) {
    let mut light = LIGHT_UI_THEME;
    light.surface_bg = LIGHT_SURFACE;
    light.panel_bg = LIGHT_PANEL;
    light.composer_bg = LIGHT_PANEL;
    light.header_bg = LIGHT_SURFACE;
    light.footer_bg = LIGHT_SURFACE;

    let mut dark = UI_THEME;
    dark.surface_bg = WHALE_BG;
    dark.panel_bg = WHALE_PANEL;
    dark.composer_bg = WHALE_COMPOSER;
    dark.header_bg = WHALE_CHROME;
    dark.footer_bg = WHALE_CHROME;
    (light, dark)
}

/// Alias name → (light, dark). Keys are `--dsw-alias-*`; values are palette
/// hex constants only — no secrets, no user data, no environment.
pub(crate) fn skin_tokens() -> BTreeMap<String, (String, String)> {
    let (light, dark) = browser_themes();
    let mut map = BTreeMap::new();
    let mut add = |name: &str, pick: fn(&UiTheme) -> Color| {
        map.insert(name.to_string(), (hex(pick(&light)), hex(pick(&dark))));
    };
    // Bounded mapping of DSH `--dsw-alias-*` variables onto Codewhale tokens.
    // Names come from dsh-client-ui-theme/lib/styles/design-platform.css.
    add("--dsw-alias-bg-base", |t| t.surface_bg);
    add("--dsw-alias-bg-layer-1", |t| t.panel_bg);
    add("--dsw-alias-bg-layer-2", |t| t.composer_bg);
    add("--dsw-alias-bg-layer-3", |t| t.elevated_bg);
    add("--dsw-alias-bg-overlay", |t| t.elevated_bg);
    add("--dsw-alias-bg-module-platform", |t| t.panel_bg);
    add("--dsw-alias-border-l1", |t| t.border);
    add("--dsw-alias-border-l2", |t| t.border);
    add("--dsw-alias-border-l3", |t| t.selection_bg);
    add("--dsw-alias-border-l4", |t| t.selection_bg);
    add("--dsw-alias-brand-primary", |t| t.accent_primary);
    add("--dsw-alias-brand-text", |t| t.accent_primary);
    add("--dsw-alias-button-primary-fill", |t| t.accent_primary);
    add("--dsw-alias-button-primary-hover", |t| t.info);
    add("--dsw-alias-button-primary-dimmed", |t| t.selection_bg);
    add("--dsw-alias-interactive-bg-hover", |t| t.selection_bg);
    add("--dsw-alias-interactive-bg-active", |t| t.selection_bg);
    add("--dsw-alias-interactive-bg-hover-danger", |t| {
        t.error_surface
    });
    add("--dsw-alias-label-primary", |t| t.text_body);
    add("--dsw-alias-label-secondary", |t| t.text_soft);
    add("--dsw-alias-label-tertiary", |t| t.text_muted);
    add("--dsw-alias-label-caption", |t| t.text_hint);
    add("--dsw-alias-label-dimmed", |t| t.text_dim);
    add("--dsw-alias-label-primary-bluish", |t| t.accent_primary);
    add("--dsw-alias-state-error-primary", |t| t.error_fg);
    add("--dsw-alias-state-error-secondary", |t| t.error_surface);
    add("--dsw-alias-state-success-primary", |t| t.success);
    add("--dsw-alias-state-success-secondary", |t| t.diff_added_bg);
    add("--dsw-alias-state-warn-primary", |t| t.warning);
    add("--dsw-alias-state-warn-label", |t| t.warning);
    add("--dsw-alias-state-business-primary", |t| t.accent_action);
    add("--dsw-alias-markdown-code-block", |t| t.panel_bg);
    add("--dsw-alias-markdown-inline-code", |t| t.composer_bg);
    add("--dsw-alias-scrollbar-bg-l1", |t| t.border);
    add("--dsw-alias-scrollbar-hover-l1", |t| t.selection_bg);
    add("--dsw-alias-toast-bg", |t| t.elevated_bg);
    add("--dsw-alias-tooltip-bg", |t| t.elevated_bg);
    map
}

pub(crate) fn skin_token_objects() -> BTreeMap<String, SkinTokens> {
    skin_tokens()
        .into_iter()
        .map(|(name, (light, dark))| (name, SkinTokens { light, dark }))
        .collect()
}

/// Deterministic TOKENS JSON object (alias → `{light, dark}`).
pub(crate) fn skin_tokens_json() -> String {
    serde_json::to_string_pretty(&skin_token_objects()).expect("skin tokens are json")
}

pub(crate) fn skin_tokens_sha256() -> String {
    super::identity::sha256_hex(skin_tokens_json().as_bytes())
}

fn indent_json_block(json: &str, indent: &str) -> String {
    json.lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                line.to_string()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Client half: `__ModuleLoader__.load` wrapper + factory that applies
/// `overrideTokens` inside `ctx.effect` and returns the disposer. `inject:
/// ["theme", "slots"]` is required: cordis 4 only exposes injected (or
/// self/ancestor provided) services on `ctx`; reading either sibling service
/// without it fails the whole web boot.
///
/// The Whale Brothers / Codewhale lockup is spliced in for every skin and
/// registered into DSH's additive `shell.overlay` slot. With `ocean` the
/// ambient scene (`scene::bundle_scene_js`) is also spliced in: the module
/// mounts the canvas inside another `ctx.effect`, follows `theme/change` for
/// light/dark, and re-issues the veil tokens (`scene::ocean_veil_tokens`) as
/// translucent rgba over the opaque table. The browser-side off switch
/// (`localStorage["codewhale.ocean"] = "off"` or body class
/// `codewhale-ocean-off`) skips both the canvas and the veil.
pub(crate) fn bundle_client_js(ocean: bool) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let tokens = indent_json_block(&skin_tokens_json(), "\t\t");
    let brand = indent_json_block(super::brand::bundle_brand_js().trim_end(), "\t\t");
    let ocean_block = if ocean {
        let veil = indent_json_block(&super::scene::ocean_veil_json(), "\t\t");
        let palette = indent_json_block(&super::scene::ocean_palette_json(), "\t\t");
        let scene = indent_json_block(super::scene::bundle_scene_js().trim_end(), "\t\t");
        format!(
            "\t\tconst OCEAN = true;\n\
\t\tconst OCEAN_VEIL = {veil};\n\
\t\tconst OCEAN_PALETTE = {palette};\n\
\t\t{scene}\n"
        )
    } else {
        "\t\tconst OCEAN = false;\n\
\t\tconst OCEAN_VEIL = null;\n\
\t\tconst OCEAN_PALETTE = null;\n\
\t\tfunction createOcean() { return null; }\n"
            .to_string()
    };
    format!(
        "/* codewhale-skin/{version} */\n\
window.__ModuleLoader__.load({{\n\
\tid: \"{SKIN_SOURCE}\",\n\
\tfactory: (require) => {{\n\
\t\tvar module = {{ exports: {{}} }};\n\
\t\tvar exports = module.exports;\n\
\t\tObject.defineProperty(exports, Symbol.toStringTag, {{ value: \"Module\" }});\n\
\t\tlet React = require(\"react\");\n\
\t\tconst TOKENS = {tokens};\n\
{ocean_block}\
		{brand}\n\
\t\tfunction apply(ctx) {{\n\
\t\t\tif (!ctx.theme || !ctx.slots) return;\n\
\t\t\tvar ocean = OCEAN ? createOcean(OCEAN_PALETTE) : null;\n\
\t\t\tvar oceanOn = ocean !== null && !ocean.isOff();\n\
\t\t\tvar tokens = oceanOn ? Object.assign({{}}, TOKENS, OCEAN_VEIL) : TOKENS;\n\
\t\t\tctx.effect(() => ctx.theme?.overrideTokens(\"{SKIN_SOURCE}\", tokens));\n\
\t\t\tctx.slots.inject(\"shell.overlay\", () => ctx.slots.register(\n\
\t\t\t\t{{ name: \"shell.overlay\", id: \"codewhale-brand-lockup\", order: 100, label: \"Codewhale\" }},\n\
\t\t\t\t() => React.createElement(CodewhaleBrand),\n\
\t\t\t));\n\
\t\t\tif (oceanOn) {{\n\
\t\t\t\tctx.effect(() => {{\n\
\t\t\t\t\tocean.setScheme(ctx.theme.getTheme().active.colorScheme);\n\
\t\t\t\t\tocean.start();\n\
\t\t\t\t\tvar off = ctx.on(\"theme/change\", (snapshot) => ocean.setScheme(snapshot.active.colorScheme));\n\
\t\t\t\t\treturn () => {{ off(); ocean.stop(); }};\n\
\t\t\t\t}});\n\
\t\t\t}}\n\
\t\t}}\n\
\t\texports.apply = apply;\n\
\t\texports.inject = [\"theme\", \"slots\"];\n\
\t\treturn module.exports;\n\
\t}}\n\
}});\n"
    )
}

/// Trivial Node cordis plugin: `apply` is a no-op so the insert-row entry mounts.
pub(crate) fn bundle_index_js() -> String {
    "function apply() {}\nexport { apply };\n".to_string()
}
