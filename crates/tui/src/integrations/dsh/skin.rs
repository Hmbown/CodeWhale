//! Codewhale skin for the DeepSeek Harness web surface, exported from the
//! TUI's real palette rather than hand-copied.
//!
//! DSH 0.1.0-rc.6 exposes no supported custom-theme API (only the built-in
//! `ui-theme.preference` light|dark|system). Its README calls third-party
//! themes "an extension point, not a product": overriding the same-named
//! `--dsw-alias-*` CSS variables. This module therefore emits a stylesheet
//! that (1) publishes Codewhale's tokens as `--cw-*` custom properties and
//! (2) maps a bounded set of DSH alias variables onto them. Applying it to a
//! running DSH page is an UNSUPPORTED overlay and is never done automatically.

use ratatui::style::Color;

use crate::palette::{LIGHT_UI_THEME, UI_THEME, UiTheme, hex_rgb_string};
use crate::tui::ocean::OceanRamp;

/// Signal Gold whale body path from `web/app/icon.svg` (viewBox 0 0 64 64
/// after the icon's translate/scale).
const MARK_BODY_PATH: &str = "M7 57c9-13 21-15 25-25 3-7-1-12-10-16 6-1 11 1 14 6 3-6 10-11 19-13-1 10-5 17-12 21-4 3-5 8-8 14-5 9-15 14-28 13Z";
/// The cyan "current" path from the same mark.
const MARK_CURRENT_PATH: &str = "M28 58c10-8 15-15 17-26 4 8 2 18-3 26H28Z";

fn hex(color: Color) -> String {
    hex_rgb_string(color).unwrap_or_else(|| "inherit".to_string())
}

fn mark_data_uri() -> String {
    // Percent-encode only what an SVG data URI needs.
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'><path fill='%23F6C453' d='{MARK_BODY_PATH}'/><path fill='%2348D7FF' d='{MARK_CURRENT_PATH}'/></svg>"
    );
    format!(
        "data:image/svg+xml;utf8,{}",
        svg.replace('#', "%23").replace('"', "'")
    )
}

fn theme_block(selector: &str, theme: &UiTheme, ramp: Option<OceanRamp>) -> String {
    let mut css = String::new();
    css.push_str(selector);
    css.push_str(" {\n");
    let mut var = |name: &str, color: Color| {
        css.push_str(&format!("  --cw-{name}: {};\n", hex(color)));
    };
    var("surface-bg", theme.surface_bg);
    var("panel-bg", theme.panel_bg);
    var("elevated-bg", theme.elevated_bg);
    var("composer-bg", theme.composer_bg);
    var("selection-bg", theme.selection_bg);
    var("header-bg", theme.header_bg);
    var("footer-bg", theme.footer_bg);
    var("text-dim", theme.text_dim);
    var("text-hint", theme.text_hint);
    var("text-muted", theme.text_muted);
    var("text-body", theme.text_body);
    var("text-soft", theme.text_soft);
    var("border", theme.border);
    var("accent-primary", theme.accent_primary);
    var("accent-secondary", theme.accent_secondary);
    var("accent-action", theme.accent_action);
    var("error-fg", theme.error_fg);
    var("error-hover", theme.error_hover);
    var("error-surface", theme.error_surface);
    var("error-border", theme.error_border);
    var("error-text", theme.error_text);
    var("warning", theme.warning);
    var("success", theme.success);
    var("info", theme.info);
    var("mode-agent", theme.mode_agent);
    var("mode-yolo", theme.mode_yolo);
    var("mode-plan", theme.mode_plan);
    var("mode-operate", theme.mode_operate);
    var("permission-ask", theme.permission_ask);
    var("permission-auto-review", theme.permission_auto_review);
    var("permission-full-access", theme.permission_full_access);
    var("status-ready", theme.status_ready);
    var("status-working", theme.status_working);
    var("status-warning", theme.status_warning);
    var("diff-added-fg", theme.diff_added_fg);
    var("diff-deleted-fg", theme.diff_deleted_fg);
    var("diff-added-bg", theme.diff_added_bg);
    var("diff-deleted-bg", theme.diff_deleted_bg);
    var("tool-running", theme.tool_running);
    var("tool-success", theme.tool_success);
    var("tool-failed", theme.tool_failed);
    if let Some(ramp) = ramp {
        var("water-surface", ramp.surface);
        var("water-middle", ramp.middle);
        var("water-deep", ramp.deep);
        var("water-ambient", ramp.ambient);
        css.push_str(
            "  --cw-water-column: linear-gradient(180deg, var(--cw-water-surface) 0%, var(--cw-water-middle) 42%, var(--cw-water-deep) 100%);\n",
        );
    } else {
        css.push_str("  --cw-water-column: var(--cw-surface-bg);\n");
    }
    // Waiting/working/focus roles are semantic aliases of the tokens above so
    // consumers use one vocabulary.
    css.push_str("  --cw-focus-ring: var(--cw-accent-primary);\n");
    css.push_str("  --cw-waiting: var(--cw-accent-action);\n");
    css.push_str("  --cw-working: var(--cw-status-working);\n");
    css.push_str("  --cw-danger: var(--cw-error-fg);\n");
    css.push_str("}\n");
    css
}

fn alias_map() -> &'static str {
    // Bounded mapping of DSH `--dsw-alias-*` variables onto Codewhale tokens.
    // Names come from dsh-client-ui-theme/lib/styles/design-platform.css.
    "  --dsw-alias-bg-base: var(--cw-surface-bg);\n\
  --dsw-alias-bg-layer-1: var(--cw-panel-bg);\n\
  --dsw-alias-bg-layer-2: var(--cw-composer-bg);\n\
  --dsw-alias-bg-layer-3: var(--cw-elevated-bg);\n\
  --dsw-alias-bg-overlay: var(--cw-elevated-bg);\n\
  --dsw-alias-bg-module-platform: var(--cw-panel-bg);\n\
  --dsw-alias-border-l1: var(--cw-border);\n\
  --dsw-alias-border-l2: var(--cw-border);\n\
  --dsw-alias-border-l3: var(--cw-selection-bg);\n\
  --dsw-alias-border-l4: var(--cw-selection-bg);\n\
  --dsw-alias-brand-primary: var(--cw-accent-primary);\n\
  --dsw-alias-brand-text: var(--cw-accent-primary);\n\
  --dsw-alias-button-primary-fill: var(--cw-accent-primary);\n\
  --dsw-alias-button-primary-hover: var(--cw-info);\n\
  --dsw-alias-button-primary-dimmed: var(--cw-selection-bg);\n\
  --dsw-alias-interactive-bg-hover: var(--cw-selection-bg);\n\
  --dsw-alias-interactive-bg-active: var(--cw-selection-bg);\n\
  --dsw-alias-interactive-bg-hover-danger: var(--cw-error-surface);\n\
  --dsw-alias-label-primary: var(--cw-text-body);\n\
  --dsw-alias-label-secondary: var(--cw-text-soft);\n\
  --dsw-alias-label-tertiary: var(--cw-text-muted);\n\
  --dsw-alias-label-caption: var(--cw-text-hint);\n\
  --dsw-alias-label-dimmed: var(--cw-text-dim);\n\
  --dsw-alias-label-primary-bluish: var(--cw-accent-primary);\n\
  --dsw-alias-state-error-primary: var(--cw-error-fg);\n\
  --dsw-alias-state-error-secondary: var(--cw-error-surface);\n\
  --dsw-alias-state-success-primary: var(--cw-success);\n\
  --dsw-alias-state-success-secondary: var(--cw-diff-added-bg);\n\
  --dsw-alias-state-warn-primary: var(--cw-warning);\n\
  --dsw-alias-state-warn-label: var(--cw-warning);\n\
  --dsw-alias-state-business-primary: var(--cw-accent-action);\n\
  --dsw-alias-markdown-code-block: var(--cw-panel-bg);\n\
  --dsw-alias-markdown-inline-code: var(--cw-composer-bg);\n\
  --dsw-alias-scrollbar-bg-l1: var(--cw-border);\n\
  --dsw-alias-scrollbar-hover-l1: var(--cw-selection-bg);\n\
  --dsw-alias-toast-bg: var(--cw-elevated-bg);\n\
  --dsw-alias-tooltip-bg: var(--cw-elevated-bg);\n"
}

/// The full stylesheet. Deterministic for a given palette build.
pub(crate) fn skin_css() -> String {
    let dark_ramp = OceanRamp::for_theme(&UI_THEME);
    let light_ramp = OceanRamp::for_theme(&LIGHT_UI_THEME);
    let mut css = String::new();
    css.push_str("/* Codewhale skin for DeepSeek Harness — generated from crates/tui/src/palette (Blue Stage).\n");
    css.push_str("   DeepSeek Harness connected through Codewhale.\n");
    css.push_str("   UNSUPPORTED OVERLAY: DeepSeek Harness 0.1.0-rc.6 has no custom-theme API; this file overrides\n");
    css.push_str("   same-named --dsw-alias-* variables (its documented 'extension point, not a product'). It is never\n");
    css.push_str("   injected automatically. DeepSeek Harness is MIT licensed, Copyright (c) 2026 DeepSeek. */\n\n");
    css.push_str(&theme_block(
        ":root, body[data-ds-dark-theme]",
        &UI_THEME,
        dark_ramp,
    ));
    css.push('\n');
    css.push_str(&theme_block(
        "body:not([data-ds-dark-theme])",
        &LIGHT_UI_THEME,
        light_ramp,
    ));
    css.push('\n');
    css.push_str(":root, body {\n");
    css.push_str(alias_map());
    css.push_str("  --cw-font-body: \"IBM Plex Sans\", system-ui, sans-serif;\n");
    css.push_str("  --cw-font-mono: \"JetBrains Mono\", ui-monospace, monospace;\n");
    css.push_str("  --cw-font-display: \"Fraunces\", Georgia, serif;\n");
    css.push_str("  --cw-text-size-body: 0.9375rem;\n");
    css.push_str("  --cw-text-size-caption: 0.8125rem;\n");
    css.push_str("  --cw-line-height-body: 1.55;\n");
    css.push_str(&format!("  --cw-mark: url(\"{}\");\n", mark_data_uri()));
    css.push_str("}\n\n");
    css.push_str("body {\n  background: var(--cw-water-column) fixed;\n  color: var(--cw-text-body);\n  font-family: var(--cw-font-body);\n  font-size: var(--cw-text-size-body);\n  line-height: var(--cw-line-height-body);\n}\n\n");
    css.push_str("code, pre, kbd { font-family: var(--cw-font-mono); }\n\n");
    css.push_str(
        ":focus-visible { outline: 2px solid var(--cw-focus-ring); outline-offset: 2px; }\n",
    );
    css.push_str(
        "::selection { background: var(--cw-selection-bg); color: var(--cw-text-body); }\n\n",
    );
    css.push_str("/* Depth breath and caustic sweep only when the user allows motion. */\n");
    css.push_str("@media (prefers-reduced-motion: no-preference) {\n  @keyframes cw-breath { 0% { opacity: 0; } 50% { opacity: 0.045; } 100% { opacity: 0; } }\n  body::after { content: \"\"; position: fixed; inset: 0; pointer-events: none; z-index: 0; background: var(--cw-water-ambient); animation: cw-breath 90s ease-in-out infinite; }\n}\n");
    css.push_str("@media (prefers-reduced-motion: reduce) {\n  *, *::before, *::after { animation-duration: 0.001ms !important; animation-iteration-count: 1 !important; transition-duration: 0.001ms !important; }\n}\n\n");
    css.push_str(
        "/* Attribution chip: the relationship stays visible wherever the skin is applied. */\n",
    );
    css.push_str(".cw-dsh-attribution { position: fixed; right: 12px; bottom: 12px; z-index: 2147483000; display: inline-flex; align-items: center; gap: 8px; padding: 6px 10px; border: 1px solid var(--cw-border); border-radius: 6px; background: var(--cw-panel-bg); color: var(--cw-text-soft); font: 600 var(--cw-text-size-caption)/1 var(--cw-font-body); }\n");
    css.push_str(".cw-dsh-attribution::before { content: \"\"; width: 16px; height: 16px; background: var(--cw-mark) center/contain no-repeat; }\n");
    css
}

/// A self-contained preview page that renders the token sheet — used only
/// for visual inspection; it is not the DSH web UI.
pub(crate) fn skin_preview_html() -> String {
    let css = skin_css();
    let swatches = [
        ("surface-bg", "Surface"),
        ("panel-bg", "Panel"),
        ("composer-bg", "Composer"),
        ("elevated-bg", "Elevated"),
        ("selection-bg", "Selection"),
        ("accent-primary", "Action"),
        ("accent-secondary", "Live"),
        ("accent-action", "Human / waiting"),
        ("success", "Success / working"),
        ("warning", "Warning"),
        ("error-fg", "Danger"),
        ("mode-agent", "Mode: Act"),
        ("mode-plan", "Mode: Plan"),
        ("mode-operate", "Mode: Operate"),
        ("mode-yolo", "Mode: Yolo"),
        ("permission-ask", "Permission: Ask"),
        ("permission-auto-review", "Permission: Auto-Review"),
        ("permission-full-access", "Permission: Full Access"),
    ];
    let mut body = String::new();
    for (var, label) in swatches {
        body.push_str(&format!(
            "<div class=\"sw\"><span class=\"chip\" style=\"background:var(--cw-{var})\"></span><span>{label}</span><code>--cw-{var}</code></div>\n"
        ));
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Codewhale skin preview (not DSH)</title><style>{css}\n\
*,*::before,*::after{{box-sizing:border-box}}html,body{{margin:0;overflow-x:hidden}}main{{max-width:960px;margin:0 auto;padding:24px;position:relative;z-index:1}}@media (max-width:480px){{main{{padding:12px}}.grid{{grid-template-columns:1fr}}.cw-dsh-attribution{{right:8px;bottom:8px;max-width:calc(100vw - 16px);white-space:normal}}}}h1{{font-family:var(--cw-font-display);font-weight:600;letter-spacing:-0.02em}}.grid{{display:grid;grid-template-columns:repeat(auto-fill,minmax(220px,1fr));gap:12px}}.sw{{display:flex;flex-direction:column;gap:6px;padding:12px;border:1px solid var(--cw-border);border-radius:8px;background:var(--cw-panel-bg)}}.chip{{display:block;height:36px;border-radius:6px;border:1px solid var(--cw-border)}}code{{color:var(--cw-text-muted);font-size:var(--cw-text-size-caption)}}.banner{{padding:10px 12px;border:1px solid var(--cw-warning);border-radius:6px;color:var(--cw-text-body);background:var(--cw-composer-bg);margin-bottom:16px}}.states{{display:flex;flex-wrap:wrap;gap:8px;margin:16px 0}}.state{{padding:6px 10px;border-radius:6px;border:1px solid var(--cw-border);background:var(--cw-panel-bg)}}.state.focus{{outline:2px solid var(--cw-focus-ring);outline-offset:2px}}.state.sel{{background:var(--cw-selection-bg);font-weight:600}}.state.err{{border-color:var(--cw-error-border);background:var(--cw-error-surface);color:var(--cw-error-text)}}.state.wait{{border-color:var(--cw-waiting);color:var(--cw-waiting)}}.state.work{{border-color:var(--cw-working);color:var(--cw-working)}}\
</style></head><body data-ds-dark-theme><main><h1>Codewhale skin — token preview</h1><div class=\"banner\">PREVIEW ONLY. This page renders the exported token sheet; it is not the DeepSeek Harness UI. Applying the sheet to DSH is an unsupported overlay.</div>\
<div class=\"states\"><span class=\"state focus\">Focus</span><span class=\"state sel\">Selected</span><span class=\"state err\">Error</span><span class=\"state wait\">Waiting for you</span><span class=\"state work\">Working</span></div>\
<div class=\"grid\">{body}</div></main><div class=\"cw-dsh-attribution\">DeepSeek Harness connected through Codewhale</div></body></html>"
    )
}
