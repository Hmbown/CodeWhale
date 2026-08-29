//! Ambient ocean scene for the DSH bundle: whales and Codewhale glyph fish
//! painted on a canvas behind the DSH web UI.
//!
//! dsh-client-modules serves exactly one file per client plugin
//! (`/plugins/<id>/client.js`), so the scene ships as a plain script fragment
//! (`scene.js`, checked in next to this module) that `skin::bundle_client_js`
//! splices into the client half. This module owns the fragment, the palette
//! the scene reads, and the "veil" — the handful of DSH background tokens the
//! client re-issues as translucent rgba so the canvas shows through faintly
//! while text stays on an opaque-enough surface.

use std::collections::BTreeMap;

use ratatui::style::Color;

use super::identity::sha256_hex;
use super::skin::SkinTokens;
use crate::palette::UiTheme;

/// `localStorage` key that turns the scene off in the browser (`"off"`).
pub(crate) const OCEAN_STORAGE_KEY: &str = "codewhale.ocean";
/// Body class that turns the scene off in the browser.
pub(crate) const OCEAN_OFF_CLASS: &str = "codewhale-ocean-off";
/// `window` handle the scene exposes (`start`, `stop`, `setIntensity`, `setScheme`).
pub(crate) const OCEAN_WINDOW_HANDLE: &str = "__codewhaleOcean";

/// Alpha of `--dsw-alias-bg-base` while the scene is on. The frame and the
/// centre column both paint this token, so the effective veil over the main
/// area is `1 - (1 - a)^2` (~0.66 at 0.42); dialogs that paint it once sit
/// at 0.42 over an opaque canvas that is itself the same base colour. This
/// keeps text surfaces calm while letting the Codewhale scene read clearly.
const BASE_VEIL_ALPHA: &str = "0.42";
/// Alpha of `--dsw-specific-sidebar-fill` (stacked over the frame).
const SIDEBAR_VEIL_ALPHA: &str = "0.78";

/// The scene script fragment: defines `createOcean(palette)`; no module
/// syntax, no top-level side effects.
pub(crate) fn bundle_scene_js() -> &'static str {
    include_str!("scene.js")
}

pub(crate) fn scene_sha256() -> String {
    sha256_hex(bundle_scene_js().as_bytes())
}

fn rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

fn hex(color: Color) -> String {
    crate::palette::hex_rgb_string(color).unwrap_or_else(|| "#808080".to_string())
}

fn rgba(color: Color, alpha: &str) -> String {
    match rgb(color) {
        Some((r, g, b)) => format!("rgba({r},{g},{b},{alpha})"),
        None => "inherit".to_string(),
    }
}

/// Palette the scene reads: `{ light: {base, accent, ink, dim}, dark: {...} }`,
/// rendered from the same TUI palettes as the skin tokens.
pub(crate) fn ocean_palette() -> BTreeMap<&'static str, BTreeMap<&'static str, String>> {
    fn one(theme: &UiTheme) -> BTreeMap<&'static str, String> {
        let mut m = BTreeMap::new();
        m.insert("base", hex(theme.surface_bg));
        m.insert("accent", hex(theme.accent_primary));
        m.insert("ink", hex(theme.text_body));
        m.insert("dim", hex(theme.text_dim));
        m
    }
    let (light, dark) = super::skin::browser_themes();
    let mut map = BTreeMap::new();
    map.insert("light", one(&light));
    map.insert("dark", one(&dark));
    map
}

pub(crate) fn ocean_palette_json() -> String {
    serde_json::to_string_pretty(&ocean_palette()).expect("ocean palette is json")
}

/// Background tokens re-issued as translucent rgba while the scene is on.
/// Same colours as the skin table (`bg-base` = `surface_bg`; the sidebar
/// takes `panel_bg`), so with the scene off nothing shifts.
pub(crate) fn ocean_veil_tokens() -> BTreeMap<String, SkinTokens> {
    let (light, dark) = super::skin::browser_themes();
    let mut map = BTreeMap::new();
    map.insert(
        "--dsw-alias-bg-base".to_string(),
        SkinTokens {
            light: rgba(light.surface_bg, BASE_VEIL_ALPHA),
            dark: rgba(dark.surface_bg, BASE_VEIL_ALPHA),
        },
    );
    map.insert(
        "--dsw-specific-sidebar-fill".to_string(),
        SkinTokens {
            light: rgba(light.panel_bg, SIDEBAR_VEIL_ALPHA),
            dark: rgba(dark.panel_bg, SIDEBAR_VEIL_ALPHA),
        },
    );
    map
}

pub(crate) fn ocean_veil_json() -> String {
    serde_json::to_string_pretty(&ocean_veil_tokens()).expect("ocean veil is json")
}
