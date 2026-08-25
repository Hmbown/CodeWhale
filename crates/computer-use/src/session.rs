//! Tool semantics on top of a [`Driver`]: coordinate frames, bounds,
//! observe-mode gating, the post-action screenshot, and the phase-2
//! app/element surface with its consent gate.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::{Config, MAX_MAX_EDGE, MIN_MAX_EDGE, Mode};
use crate::consent::{self, AppIdentity, Verdict};
use crate::driver::{
    AppAction, Button, Driver, DriverError, Point, ScrollDir, TargetInfo, TargetKind, UiNode,
};
use crate::elements::{self, ElementAction, StateMode};
use crate::frame::{self, Frame, Zoom};
use crate::keys;

pub const MAX_TYPE_CHARS: usize = 4000;
pub const MAX_WAIT_MS: u64 = 15_000;
pub const MAX_DRAG_MS: u64 = 5_000;
pub const MAX_HOLD_MS: u64 = 5_000;
pub const MAX_SCROLL_AMOUNT: u32 = 50;
pub const MAX_UI_NODES: usize = 300;
pub const DEFAULT_UI_NODES: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    pub text: String,
    pub image_png: Option<Vec<u8>>,
    pub is_error: bool,
}

impl ToolOutcome {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            image_png: None,
            is_error: false,
        }
    }

    fn err(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            image_png: None,
            is_error: true,
        }
    }
}

pub struct Session {
    driver: Box<dyn Driver>,
    cfg: Config,
    info: Option<TargetInfo>,
    /// Raw bytes of the last capture (for zoom).
    capture: Option<Vec<u8>>,
    frame: Option<Frame>,
    zoom: Option<Zoom>,
    /// Last `computer_app_state` snapshot per app: identity, window frame,
    /// and the scale from window-image pixels to the platform's native
    /// coordinate space.
    app_snapshots: HashMap<String, AppSnapshot>,
}

#[derive(Debug, Clone)]
pub struct AppSnapshot {
    pub identity: AppIdentity,
    pub window_w: u32,
    pub window_h: u32,
    pub image_w: u32,
    pub image_h: u32,
    pub node_count: usize,
}

impl AppSnapshot {
    /// Map window-image pixels to the window's own pixel space.
    pub fn to_window(&self, x: f64, y: f64) -> Result<(f64, f64), String> {
        let (w, h) = (
            f64::from(self.image_w.max(1)),
            f64::from(self.image_h.max(1)),
        );
        if !x.is_finite() || !y.is_finite() || x < -0.5 || y < -0.5 || x > w + 0.5 || y > h + 0.5 {
            return Err(format!(
                "point ({x}, {y}) is outside the last app-state image {}x{}; call computer_app_state again",
                self.image_w, self.image_h
            ));
        }
        Ok((
            (x * f64::from(self.window_w) / w)
                .clamp(0.0, f64::from(self.window_w.saturating_sub(1))),
            (y * f64::from(self.window_h) / h)
                .clamp(0.0, f64::from(self.window_h.saturating_sub(1))),
        ))
    }
}

/// Tool names, in catalog order.
pub const TOOL_NAMES: &[&str] = &[
    "computer_info",
    "computer_screenshot",
    "computer_zoom",
    "computer_click",
    "computer_move",
    "computer_drag",
    "computer_scroll",
    "computer_type",
    "computer_key",
    "computer_wait",
    "computer_ui_tree",
    "computer_app",
    "computer_apps",
    "computer_app_state",
    "computer_element",
    "computer_raise",
    "computer_devices",
];

const ACTION_TOOLS: &[&str] = &[
    "computer_click",
    "computer_move",
    "computer_drag",
    "computer_scroll",
    "computer_type",
    "computer_key",
    "computer_app",
    "computer_element",
    "computer_raise",
];

fn frame_param() -> Value {
    json!({
        "type": "string",
        "enum": ["screen", "zoom"],
        "description": "Coordinate space: \"screen\" (default) = pixels of the last computer_screenshot; \"zoom\" = pixels of the last computer_zoom image."
    })
}

fn app_param() -> Value {
    json!({
        "type": "string",
        "description": "App to act on in the background (bundle id, name, or pid). With this set, the action targets that app's window instead of the foreground screen; call computer_app_state on it first. Without it, the action goes to the screen as usual."
    })
}

fn coord(desc: &str) -> Value {
    json!({ "type": "number", "description": desc })
}

/// MCP tool descriptors (`tools/list`).
pub fn tool_catalog() -> Vec<Value> {
    vec![
        json!({
            "name": "computer_info",
            "description": "Describe the controlled target: platform, display size, current frame, mode, and permission diagnostics. Call this first if something fails.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "computer_screenshot",
            "description": "Capture the screen. Returns an image plus its size as `frame: WxH`. All x/y arguments to other tools are pixels of this image. Set grid=true to overlay labeled coordinate lines when you need help estimating positions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "grid": { "type": "boolean", "description": "Overlay a labeled coordinate grid (default false)." },
                    "max_edge": { "type": "integer", "minimum": MIN_MAX_EDGE, "maximum": MAX_MAX_EDGE, "description": "Longest edge of the returned image in pixels (default from config, 1024)." }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_zoom",
            "description": "Re-capture a rectangular region of the screen at higher detail. x/y/width/height are in screen-frame pixels. Use it to read small text or locate small controls precisely; then act with frame=\"zoom\" or convert coordinates as instructed in the result.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "minimum": 0 },
                    "y": { "type": "integer", "minimum": 0 },
                    "width": { "type": "integer", "minimum": 8 },
                    "height": { "type": "integer", "minimum": 8 }
                },
                "required": ["x", "y", "width", "height"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_click",
            "description": "Click (or tap) at a point. Returns a fresh screenshot after the click. clicks=2 double-clicks; hold_ms>0 long-presses; button=right opens context menus (long-press on phones). With app set, clicks that app's window in the background at pixels of the last computer_app_state image (macOS).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "x": coord("X in frame pixels (or app-state image pixels when app is set)"),
                    "y": coord("Y in frame pixels (or app-state image pixels when app is set)"),
                    "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "Mouse button (default left)." },
                    "clicks": { "type": "integer", "minimum": 1, "maximum": 3, "description": "1 = single (default), 2 = double, 3 = triple." },
                    "hold_ms": { "type": "integer", "minimum": 0, "maximum": MAX_HOLD_MS, "description": "Press duration for long-press (default 0)." },
                    "frame": frame_param(),
                    "app": app_param()
                },
                "required": ["x", "y"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_move",
            "description": "Move the mouse pointer to a point without clicking (hover). Returns a fresh screenshot.",
            "inputSchema": {
                "type": "object",
                "properties": { "x": coord("X in frame pixels"), "y": coord("Y in frame pixels"), "frame": frame_param() },
                "required": ["x", "y"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_drag",
            "description": "Press at one point, move to another, release (drag-and-drop, swipe on phones). Returns a fresh screenshot. With app set, drags inside that app's window in the background (macOS; unreliable in Chromium windows).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from_x": coord("Start X"), "from_y": coord("Start Y"),
                    "to_x": coord("End X"), "to_y": coord("End Y"),
                    "duration_ms": { "type": "integer", "minimum": 0, "maximum": MAX_DRAG_MS, "description": "Movement duration (default 500)." },
                    "frame": frame_param(),
                    "app": app_param()
                },
                "required": ["from_x", "from_y", "to_x", "to_y"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_scroll",
            "description": "Scroll at a point. amount is in wheel notches on desktop and in swipe steps on phones (default 3). Returns a fresh screenshot. With app set, scrolls that app's window in the background and reports whether it moved.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "x": coord("X in frame pixels"), "y": coord("Y in frame pixels"),
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "amount": { "type": "integer", "minimum": 1, "maximum": MAX_SCROLL_AMOUNT },
                    "frame": frame_param(),
                    "app": app_param()
                },
                "required": ["x", "y", "direction"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_type",
            "description": "Type text into the focused control (click it first). Newlines are typed as Enter. Returns a fresh screenshot. With app set, types into the focused control of that app in the background (macOS).",
            "inputSchema": {
                "type": "object",
                "properties": { "text": { "type": "string", "maxLength": MAX_TYPE_CHARS }, "app": app_param() },
                "required": ["text"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_key",
            "description": "Press a key or combination, e.g. \"enter\", \"ctrl+s\", \"cmd+shift+t\", \"alt+f4\", \"back\" (phone back), \"apphome\" (phone home). Modifiers: ctrl, alt, shift, meta/cmd/win. Returns a fresh screenshot. With app set, the keystroke goes to that app in the background (macOS).",
            "inputSchema": {
                "type": "object",
                "properties": { "keys": { "type": "string" }, "app": app_param() },
                "required": ["keys"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_wait",
            "description": "Wait for the UI to settle (e.g. a page load), then return a fresh screenshot.",
            "inputSchema": {
                "type": "object",
                "properties": { "ms": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS, "description": "Milliseconds to wait (default 1000)." } },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_ui_tree",
            "description": "List interactive on-screen elements with their labels and frame-pixel centers (Android and HarmonyOS only). Prefer this over guessing coordinates on phones.",
            "inputSchema": {
                "type": "object",
                "properties": { "max_nodes": { "type": "integer", "minimum": 1, "maximum": MAX_UI_NODES } },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_app",
            "description": "Launch an application by name or package/bundle id, list installed apps, or report the current app.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["launch", "list", "current"] },
                    "name": { "type": "string", "description": "App name, Android package, or HarmonyOS bundle name (for launch)." }
                },
                "required": ["action"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_apps",
            "description": "List running apps with their windows and the computer-use consent state of each (allowed / needs approval / denied / excluded). Run this before the first computer_app_state.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "computer_app_state",
            "description": "Capture one app's window: the window image plus an indexed element tree, even when the window is covered or in the background. Indexes from the tree feed computer_element; x/y for app-targeted clicks are pixels of the returned image. mode: full (default) | image | ax; filter narrows the tree; window_id picks one window.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "App to capture (bundle id, name, or pid)." },
                    "mode": { "type": "string", "enum": ["full", "image", "ax"], "description": "What to capture (default full)." },
                    "filter": { "type": "string", "description": "Case-insensitive substring filter over the element tree." },
                    "window_id": { "type": "integer", "minimum": 0, "description": "Capture this window of the app (default: first)." },
                    "max_nodes": { "type": "integer", "minimum": 1, "maximum": MAX_UI_NODES, "description": "Max interactive elements listed (default 120)." }
                },
                "required": ["app"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_element",
            "description": "Act on an app through its elements (from the last computer_app_state). action: press (AXPress or background click on the indexed element), set_value (write + read-back), menu (open a menu), scroll (pages, reports moved), select_text (character range), click/type/key/coords/drag alternatives. Never types into secure fields.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "App to act on (bundle id, name, or pid)." },
                    "action": { "type": "string", "enum": ["press", "set_value", "menu", "scroll", "select_text", "click", "type", "key", "drag"], "description": "The element action." },
                    "index": { "type": "integer", "minimum": 0, "description": "Element index from computer_app_state (press/set_value/menu/select_text; optional for scroll)." },
                    "value": { "type": "string", "description": "New value for set_value." },
                    "text": { "type": "string", "description": "Text for the type action." },
                    "keys": { "type": "string", "description": "Key combo for the key action." },
                    "x": coord("X in app-state image pixels (click/drag start)."),
                    "y": coord("Y in app-state image pixels (click/drag start)."),
                    "to_x": coord("Drag end X"),
                    "to_y": coord("Drag end Y"),
                    "button": { "type": "string", "enum": ["left", "right", "middle"] },
                    "clicks": { "type": "integer", "minimum": 1, "maximum": 3 },
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction." },
                    "pages": { "type": "integer", "minimum": 1, "maximum": MAX_SCROLL_AMOUNT, "description": "Pages to scroll (default 1)." },
                    "start": { "type": "integer", "minimum": 0, "description": "select_text start offset." },
                    "end": { "type": "integer", "minimum": 0, "description": "select_text end offset." }
                },
                "required": ["app", "action"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_raise",
            "description": "Bring an app's window to the foreground. Visible fallback for when the app ignores background input; prefer the app argument / computer_element first. Returns a fresh screenshot.",
            "inputSchema": {
                "type": "object",
                "properties": { "app": { "type": "string", "description": "App to raise (bundle id, name, or pid)." } },
                "required": ["app"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "computer_devices",
            "description": "List attached Android (adb) and HarmonyOS (hdc) devices and which one is selected.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
    ]
}

fn get_f64(args: &Value, key: &str) -> Result<f64, String> {
    match args.get(key) {
        Some(v) => v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
            .ok_or_else(|| format!("`{key}` must be a number")),
        None => Err(format!("missing required argument `{key}`")),
    }
}

fn get_u64_opt(args: &Value, key: &str, default: u64, max: u64) -> Result<u64, String> {
    let Some(v) = args.get(key) else {
        return Ok(default);
    };
    if v.is_null() {
        return Ok(default);
    }
    let n = v
        .as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        .ok_or_else(|| format!("`{key}` must be an integer"))?;
    if n < 0.0 || n.fract() != 0.0 {
        return Err(format!("`{key}` must be a non-negative integer"));
    }
    let n = n as u64;
    if n > max {
        return Err(format!("`{key}` must be at most {max}"));
    }
    Ok(n)
}

fn get_str_opt<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn get_bool_opt(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn describe_nodes(nodes: &[UiNode], frame: &Frame, max: usize) -> String {
    let scale_x = f64::from(frame.shot_w) / f64::from(frame.dev_w.max(1));
    let scale_y = f64::from(frame.shot_h) / f64::from(frame.dev_h.max(1));
    let mut lines = Vec::new();
    let mut shown = 0usize;
    for node in nodes.iter().filter(|n| n.is_interesting()) {
        if shown >= max {
            break;
        }
        let (cx, cy) = node.center();
        let (l, t, r, b) = node.bounds;
        let fx = (f64::from(cx) * scale_x).round() as u32;
        let fy = (f64::from(cy) * scale_y).round() as u32;
        let fw = (f64::from(r.saturating_sub(l)) * scale_x).round() as u32;
        let fh = (f64::from(b.saturating_sub(t)) * scale_y).round() as u32;
        let mut flags = Vec::new();
        if node.clickable {
            flags.push("clickable");
        }
        if node.editable {
            flags.push("editable");
        }
        if node.scrollable {
            flags.push("scrollable");
        }
        if node.focused {
            flags.push("focused");
        }
        let label = if node.label.is_empty() {
            String::new()
        } else {
            format!(" \"{}\"", node.label)
        };
        let id = if node.id.is_empty() {
            String::new()
        } else {
            format!(" id={}", node.id)
        };
        lines.push(format!(
            "#{shown} [{}]{label}{id} center=({fx},{fy}) size={fw}x{fh}{}",
            node.class,
            if flags.is_empty() {
                String::new()
            } else {
                format!(" {}", flags.join(","))
            }
        ));
        shown += 1;
    }
    let total = nodes.iter().filter(|n| n.is_interesting()).count();
    let mut out = format!(
        "{} ({shown} of {total} interactive elements; centers are frame pixels)\n",
        frame.describe()
    );
    out.push_str(&lines.join("\n"));
    if total > shown {
        out.push_str(&format!(
            "\n… {} more omitted; raise max_nodes or zoom in",
            total - shown
        ));
    }
    out
}

impl Session {
    pub fn new(driver: Box<dyn Driver>, cfg: Config) -> Self {
        Self {
            driver,
            cfg,
            info: None,
            capture: None,
            frame: None,
            zoom: None,
            app_snapshots: HashMap::new(),
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn frame(&self) -> Option<Frame> {
        self.frame
    }

    fn info(&mut self) -> Result<TargetInfo, DriverError> {
        if let Some(info) = &self.info {
            return Ok(info.clone());
        }
        let info = self.driver.info()?;
        self.info = Some(info.clone());
        Ok(info)
    }

    fn settle_ms(&mut self) -> u64 {
        match self.info().map(|i| i.kind) {
            Ok(TargetKind::Desktop) | Err(_) => self.cfg.settle_ms_desktop,
            Ok(_) => self.cfg.settle_ms_device,
        }
    }

    /// Capture, downscale, and store a new frame. Invalidates any zoom.
    fn capture_frame(&mut self, grid: bool, max_edge: u32) -> Result<(Vec<u8>, Frame), String> {
        let raw = self.driver.screenshot().map_err(|e| e.to_string())?;
        let (png, frame) = frame::prepare(&raw.bytes, max_edge, grid)?;
        self.capture = Some(raw.bytes);
        self.frame = Some(frame);
        self.zoom = None;
        Ok((png, frame))
    }

    fn point_from(&self, args: &Value, xk: &str, yk: &str) -> Result<Point, String> {
        let x = get_f64(args, xk)?;
        let y = get_f64(args, yk)?;
        let frame = self
            .frame
            .ok_or_else(|| "no frame yet: call computer_screenshot before acting".to_string())?;
        let use_zoom = match get_str_opt(args, "frame")
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("") | Some("screen") => false,
            Some("zoom") => true,
            Some(other) => {
                return Err(format!(
                    "frame must be \"screen\" or \"zoom\", got `{other}`"
                ));
            }
        };
        let (fx, fy) = if use_zoom {
            let zoom = self
                .zoom
                .ok_or_else(|| "frame=\"zoom\" requested but there is no current zoom image; call computer_zoom first".to_string())?;
            zoom.to_frame(x, y)?
        } else {
            (x, y)
        };
        let (dx, dy) = frame.to_device(fx, fy)?;
        Ok(Point { x: dx, y: dy })
    }

    /// Finish an action: settle, re-capture, and attach the new frame.
    fn after_action(&mut self, mut text: String) -> ToolOutcome {
        if !self.cfg.screenshot_after_action {
            if let Some(frame) = self.frame {
                text = format!("{}\n{text}", frame.describe());
            }
            return ToolOutcome::ok(text);
        }
        let settle = self.settle_ms();
        if settle > 0 {
            std::thread::sleep(Duration::from_millis(settle));
        }
        let grid = self.cfg.grid_default;
        let max_edge = self.cfg.max_edge;
        match self.capture_frame(grid, max_edge) {
            Ok((png, frame)) => ToolOutcome {
                text: format!(
                    "{}\n{text}\n(screenshot taken after the action; any earlier zoom is stale)",
                    frame.describe()
                ),
                image_png: Some(png),
                is_error: false,
            },
            Err(e) => ToolOutcome::ok(format!("{text}\n(post-action screenshot failed: {e})")),
        }
    }

    pub fn call(&mut self, name: &str, args: &Value) -> ToolOutcome {
        if !TOOL_NAMES.contains(&name) {
            return ToolOutcome::err(format!("unknown tool `{name}`"));
        }
        if self.cfg.mode == Mode::Observe && ACTION_TOOLS.contains(&name) {
            return ToolOutcome::err(format!(
                "`{name}` is disabled: the computer-use server is running in observe mode (set mode = \"act\" in ~/.codewhale/computer-use.toml to allow input)"
            ));
        }
        let args = if args.is_null() {
            &Value::Object(Default::default())
        } else {
            args
        };
        let result = match name {
            "computer_info" => self.tool_info(),
            "computer_screenshot" => self.tool_screenshot(args),
            "computer_zoom" => self.tool_zoom(args),
            "computer_click" => self.tool_click(args),
            "computer_move" => self.tool_move(args),
            "computer_drag" => self.tool_drag(args),
            "computer_scroll" => self.tool_scroll(args),
            "computer_type" => self.tool_type(args),
            "computer_key" => self.tool_key(args),
            "computer_wait" => self.tool_wait(args),
            "computer_ui_tree" => self.tool_ui_tree(args),
            "computer_app" => self.tool_app(args),
            "computer_apps" => self.tool_apps(),
            "computer_app_state" => self.tool_app_state(args),
            "computer_element" => self.tool_element(args),
            "computer_raise" => self.tool_raise(args),
            "computer_devices" => self.tool_devices(),
            _ => unreachable!("tool names are validated above"),
        };
        match result {
            Ok(outcome) => outcome,
            Err(message) => ToolOutcome::err(message),
        }
    }

    fn tool_info(&mut self) -> Result<ToolOutcome, String> {
        let mut lines = Vec::new();
        match self.driver.info() {
            Ok(info) => {
                self.info = Some(info.clone());
                lines.push(format!("target: {} ({})", info.kind.label(), info.driver));
                lines.push(format!(
                    "display: {}x{} device pixels",
                    info.device_w, info.device_h
                ));
                lines.push(format!(
                    "capabilities: screenshot, zoom, click, move, drag, scroll, type, key, wait{}{}",
                    if info.supports_ui_tree { ", ui_tree" } else { "" },
                    if info.supports_apps { ", app" } else { "" }
                ));
                for note in &info.notes {
                    lines.push(format!("note: {note}"));
                }
            }
            Err(e) => lines.push(format!("target: unavailable ({e})")),
        }
        lines.push(format!("mode: {}", self.cfg.mode.label()));
        lines.push(format!(
            "screenshot budget: longest edge {} px; post-action screenshots {}",
            self.cfg.max_edge,
            if self.cfg.screenshot_after_action {
                "on"
            } else {
                "off"
            }
        ));
        match self.frame {
            Some(frame) => lines.push(frame.describe()),
            None => lines.push("frame: none yet (call computer_screenshot)".to_string()),
        }
        if let Some(zoom) = self.zoom {
            lines.push(zoom.describe());
        }
        lines.push(
            "coordinates: give x/y in pixels of the most recent screenshot image (top-left origin)"
                .to_string(),
        );
        Ok(ToolOutcome::ok(lines.join("\n")))
    }

    fn tool_screenshot(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        let grid = get_bool_opt(args, "grid", self.cfg.grid_default);
        let max_edge = get_u64_opt(
            args,
            "max_edge",
            u64::from(self.cfg.max_edge),
            u64::from(MAX_MAX_EDGE),
        )? as u32;
        if max_edge < MIN_MAX_EDGE {
            return Err(format!("max_edge must be at least {MIN_MAX_EDGE}"));
        }
        let (png, frame) = self.capture_frame(grid, max_edge)?;
        let mut text = frame.describe();
        if grid {
            text.push_str("\ngrid: magenta lines with yellow pixel labels are an overlay, not part of the screen");
        }
        Ok(ToolOutcome {
            text,
            image_png: Some(png),
            is_error: false,
        })
    }

    fn tool_zoom(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        let frame = self
            .frame
            .ok_or_else(|| "no frame yet: call computer_screenshot before zooming".to_string())?;
        let x = get_u64_opt(args, "x", u64::MAX, u64::from(u32::MAX))?;
        let y = get_u64_opt(args, "y", u64::MAX, u64::from(u32::MAX))?;
        let w = get_u64_opt(args, "width", u64::MAX, u64::from(u32::MAX))?;
        let h = get_u64_opt(args, "height", u64::MAX, u64::from(u32::MAX))?;
        if [x, y, w, h].contains(&u64::MAX) {
            return Err("x, y, width, and height are required".to_string());
        }
        // Fresh capture so the zoom reflects the current screen; the frame
        // geometry must still match or the coordinates would lie.
        let raw = self.driver.screenshot().map_err(|e| e.to_string())?;
        let dims = frame::decode(&raw.bytes).map(|img| {
            use image::GenericImageView;
            img.dimensions()
        })?;
        if dims != (frame.dev_w, frame.dev_h) {
            return Err(format!(
                "screen size changed ({}x{} -> {}x{}); call computer_screenshot again",
                frame.dev_w, frame.dev_h, dims.0, dims.1
            ));
        }
        self.capture = Some(raw.bytes);
        let capture = self.capture.as_ref().expect("capture just stored");
        let (png, zoom) = frame::zoom(
            capture,
            &frame,
            (x as u32, y as u32, w as u32, h as u32),
            self.cfg.max_edge,
        )?;
        self.zoom = Some(zoom);
        Ok(ToolOutcome {
            text: format!("{}\n{}", frame.describe(), zoom.describe()),
            image_png: Some(png),
            is_error: false,
        })
    }

    fn tool_click(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        if let Some(out) = self.routed_app_action(args, "computer_click")? {
            return Ok(out);
        }
        let p = self.point_from(args, "x", "y")?;
        let button = Button::parse(get_str_opt(args, "button").unwrap_or("left"))?;
        let clicks = get_u64_opt(args, "clicks", 1, 3)?.max(1) as u32;
        let hold_ms = get_u64_opt(args, "hold_ms", 0, MAX_HOLD_MS)?;
        self.driver
            .click(p, button, clicks, hold_ms)
            .map_err(|e| e.to_string())?;
        let what = match (button, clicks, hold_ms) {
            (_, _, h) if h > 0 => format!("long-pressed ({h} ms)"),
            (Button::Left, 1, _) => "clicked".to_string(),
            (Button::Left, 2, _) => "double-clicked".to_string(),
            (Button::Left, 3, _) => "triple-clicked".to_string(),
            (Button::Right, _, _) => "right-clicked".to_string(),
            (Button::Middle, _, _) => "middle-clicked".to_string(),
            (Button::Left, n, _) => format!("clicked {n}x"),
        };
        Ok(self.after_action(format!("{what} at device ({:.0}, {:.0})", p.x, p.y)))
    }

    fn tool_move(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        let p = self.point_from(args, "x", "y")?;
        self.driver.move_to(p).map_err(|e| e.to_string())?;
        Ok(self.after_action(format!("moved pointer to device ({:.0}, {:.0})", p.x, p.y)))
    }

    fn tool_drag(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        if let Some(out) = self.routed_app_action(args, "computer_drag")? {
            return Ok(out);
        }
        let from = self.point_from(args, "from_x", "from_y")?;
        let to = self.point_from(args, "to_x", "to_y")?;
        let duration = get_u64_opt(args, "duration_ms", 500, MAX_DRAG_MS)?;
        self.driver
            .drag(from, to, duration)
            .map_err(|e| e.to_string())?;
        Ok(self.after_action(format!(
            "dragged from device ({:.0}, {:.0}) to ({:.0}, {:.0}) over {duration} ms",
            from.x, from.y, to.x, to.y
        )))
    }

    fn tool_scroll(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        if let Some(out) = self.routed_app_action(args, "computer_scroll")? {
            return Ok(out);
        }
        let p = self.point_from(args, "x", "y")?;
        let dir = ScrollDir::parse(get_str_opt(args, "direction").unwrap_or(""))?;
        let amount = get_u64_opt(args, "amount", 3, u64::from(MAX_SCROLL_AMOUNT))?.max(1) as u32;
        self.driver
            .scroll(p, dir, amount)
            .map_err(|e| e.to_string())?;
        Ok(self.after_action(
            format!(
                "scrolled {dir:?} by {amount} at device ({:.0}, {:.0})",
                p.x, p.y
            )
            .to_ascii_lowercase(),
        ))
    }

    fn tool_type(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        if let Some(out) = self.routed_app_action(args, "computer_type")? {
            return Ok(out);
        }
        let text = get_str_opt(args, "text")
            .ok_or_else(|| "missing required argument `text`".to_string())?;
        let count = text.chars().count();
        if count == 0 {
            return Err("text must not be empty".to_string());
        }
        if count > MAX_TYPE_CHARS {
            return Err(format!(
                "text is {count} characters; the limit is {MAX_TYPE_CHARS}"
            ));
        }
        self.driver.type_text(text).map_err(|e| e.to_string())?;
        Ok(self.after_action(format!("typed {count} characters")))
    }

    fn tool_key(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        if let Some(out) = self.routed_app_action(args, "computer_key")? {
            return Ok(out);
        }
        let keys = get_str_opt(args, "keys")
            .ok_or_else(|| "missing required argument `keys`".to_string())?;
        let combo = keys::parse_combo(keys)?;
        self.driver.key(&combo).map_err(|e| e.to_string())?;
        Ok(self.after_action(format!("pressed {combo}")))
    }

    fn tool_wait(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        let ms = get_u64_opt(args, "ms", 1000, MAX_WAIT_MS)?;
        std::thread::sleep(Duration::from_millis(ms));
        let grid = self.cfg.grid_default;
        let max_edge = self.cfg.max_edge;
        let (png, frame) = self.capture_frame(grid, max_edge)?;
        Ok(ToolOutcome {
            text: format!("{}\nwaited {ms} ms", frame.describe()),
            image_png: Some(png),
            is_error: false,
        })
    }

    fn tool_ui_tree(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        let max = get_u64_opt(
            args,
            "max_nodes",
            DEFAULT_UI_NODES as u64,
            MAX_UI_NODES as u64,
        )?
        .max(1) as usize;
        let nodes = self.driver.ui_tree().map_err(|e| e.to_string())?;
        if self.frame.is_none() {
            let grid = self.cfg.grid_default;
            let max_edge = self.cfg.max_edge;
            self.capture_frame(grid, max_edge)?;
        }
        let frame = self.frame.expect("frame ensured above");
        Ok(ToolOutcome::ok(describe_nodes(&nodes, &frame, max)))
    }

    fn tool_app(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        let action = get_str_opt(args, "action")
            .unwrap_or("")
            .to_ascii_lowercase();
        let name = get_str_opt(args, "name").unwrap_or("").trim().to_string();
        let action = match action.as_str() {
            "launch" | "open" | "start" => {
                if name.is_empty() {
                    return Err("`name` is required for action=launch".to_string());
                }
                AppAction::Launch(&name)
            }
            "list" => AppAction::List,
            "current" | "focused" => AppAction::Current,
            other => {
                return Err(format!(
                    "unknown app action `{other}`; expected launch, list, or current"
                ));
            }
        };
        let text = self.driver.app(action).map_err(|e| e.to_string())?;
        if matches!(action, AppAction::Launch(_)) {
            Ok(self.after_action(text))
        } else {
            Ok(ToolOutcome::ok(text))
        }
    }

    fn tool_devices(&mut self) -> Result<ToolOutcome, String> {
        self.driver
            .devices()
            .map(ToolOutcome::ok)
            .map_err(|e| e.to_string())
    }

    // ------------------------------------------------------------------
    // Phase 2: apps, elements, consent.
    // ------------------------------------------------------------------

    fn element_driver(&mut self) -> Result<&mut dyn elements::ElementDriver, String> {
        self.driver.element().ok_or_else(|| {
            "app/element tools are not available on this target: they run on macOS (full), Windows (reads), Android and HarmonyOS (element actions); use computer_screenshot + coordinates here".to_string()
        })
    }

    /// Find the running app matching `spec` (bundle id, name, or pid).
    fn resolve_app(&mut self, spec: &str) -> Result<AppIdentity, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("missing required argument `app`".to_string());
        }
        let apps = self.element_driver()?.apps().map_err(|e| e.to_string())?;
        if let Some(pid) = spec
            .parse::<u32>()
            .ok()
            .and_then(|pid| apps.iter().find(|a| a.identity.pid == pid))
        {
            return Ok(pid.identity.clone());
        }
        let needle = spec.to_lowercase();
        let matches: Vec<&elements::AppInfo> = apps
            .iter()
            .filter(|a| {
                a.identity.aliases().iter().any(|alias| {
                    let alias = alias.to_lowercase();
                    alias == needle || alias.contains(&needle) || needle.contains(&alias)
                })
            })
            .collect();
        // Exact alias beats substring.
        if let Some(exact) = matches.iter().find(|a| {
            a.identity
                .aliases()
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(spec))
        }) {
            return Ok(exact.identity.clone());
        }
        match matches.as_slice() {
            [one] => Ok(one.identity.clone()),
            [] => Err(format!(
                "no running app matches `{spec}`; computer_apps lists what is running"
            )),
            many => {
                let names: Vec<String> = many.iter().map(|a| a.identity.label()).collect();
                Err(format!(
                    "`{spec}` matches several apps; name one exactly: {}",
                    names.join(", ")
                ))
            }
        }
    }

    /// Consent gate for every app-targeted call.
    fn gate_app(&self, app: &AppIdentity) -> Result<(), String> {
        match self.cfg.apps.verdict(app) {
            Verdict::Allowed => Ok(()),
            Verdict::NeedsApproval => {
                Err(consent::needs_approval_error(app, &self.cfg.path_hint()))
            }
            Verdict::Denied => Err(consent::denied_error(app)),
            Verdict::Excluded => Err(consent::excluded_error(app)),
        }
    }

    fn tool_apps(&mut self) -> Result<ToolOutcome, String> {
        let apps = self.element_driver()?.apps().map_err(|e| e.to_string())?;
        if apps.is_empty() {
            return Ok(ToolOutcome::ok("no apps with windows found".to_string()));
        }
        let mut lines = vec![format!("{} apps with windows:", apps.len())];
        for app in &apps {
            let verdict = match self.cfg.apps.verdict(&app.identity) {
                Verdict::Allowed => "allowed",
                Verdict::NeedsApproval => "needs approval",
                Verdict::Denied => "denied",
                Verdict::Excluded => "excluded",
            };
            let id = if app.identity.bundle_id.is_empty() {
                String::new()
            } else {
                format!(" ({})", app.identity.bundle_id)
            };
            let windows: Vec<String> = app
                .windows
                .iter()
                .map(|w| {
                    let title = if w.title.trim().is_empty() {
                        "untitled".to_string()
                    } else {
                        w.title.clone()
                    };
                    format!("{}#{}", title, w.id)
                })
                .collect();
            lines.push(format!(
                "- {}{} pid={} [{}] windows: {}",
                app.identity.label(),
                id,
                app.identity.pid,
                verdict,
                windows.join(", ")
            ));
        }
        lines.push(format!(
            "consent: allow = {} deny = {}",
            self.cfg.apps.allow_line(),
            self.cfg.apps.deny_line()
        ));
        Ok(ToolOutcome::ok(lines.join("\n")))
    }

    fn tool_app_state(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        let spec = get_str_opt(args, "app")
            .ok_or_else(|| "missing required argument `app`".to_string())?;
        let identity = self.resolve_app(spec)?;
        self.gate_app(&identity)?;
        let mode = StateMode::parse(get_str_opt(args, "mode").unwrap_or(""))?;
        let max = get_u64_opt(
            args,
            "max_nodes",
            DEFAULT_UI_NODES as u64,
            MAX_UI_NODES as u64,
        )?
        .max(1) as usize;
        let opts = elements::StateOpts {
            mode,
            filter: get_str_opt(args, "filter").map(str::to_string),
            window_id: get_str_opt(args, "window_id")
                .or_else(|| args.get("window_id").and_then(|v| v.as_str()).map(|_| ""))
                .and_then(|raw| {
                    let raw = if raw.is_empty() {
                        args.get("window_id").and_then(Value::as_str)?.to_string()
                    } else {
                        raw.to_string()
                    };
                    raw.trim().parse::<u32>().ok()
                })
                .or_else(|| {
                    args.get("window_id")
                        .and_then(Value::as_u64)
                        .map(|n| n as u32)
                }),
            max_nodes: max,
            max_edge: self.cfg.max_edge,
        };
        let state = self
            .element_driver()?
            .app_state(&identity, &opts)
            .map_err(|e| e.to_string())?;
        let snapshot = AppSnapshot {
            identity: identity.clone(),
            window_w: state.window.w,
            window_h: state.window.h,
            image_w: state.image_w,
            image_h: state.image_h,
            node_count: state.nodes.len(),
        };
        self.app_snapshots.insert(identity.label(), snapshot);
        let (tree, total) = elements::render_nodes(&state.nodes, opts.filter.as_deref(), max);
        let mut lines = vec![format!(
            "app: {}{} window \"{}\" ({}x{})",
            identity.label(),
            if identity.bundle_id.is_empty() {
                String::new()
            } else {
                format!(" ({})", identity.bundle_id)
            },
            state.window.title,
            state.image_w,
            state.image_h
        )];
        if state.occluded {
            lines.push("occluded: true (window is covered; capture still reflects it)".to_string());
        }
        if mode != StateMode::Image {
            lines.push(format!(
                "elements ({total} interactive of {} nodes):",
                state.nodes.len()
            ));
            if tree.is_empty() {
                lines.push("(none matched)".to_string());
            } else {
                lines.push(tree);
            }
            if total > max {
                lines.push(format!(
                    "… {} more omitted; raise max_nodes or use filter",
                    total - max
                ));
            }
            if state.omitted > 0 {
                // The driver stopped walking, so those elements have no index
                // at all — filter cannot reach them either.
                lines.push(format!(
                    "note: the window is larger than the capture limit; {} subtree(s) were not walked and are not addressable",
                    state.omitted
                ));
            }
            lines.push("act by index with computer_element; x/y for app-targeted clicks are pixels of this image".to_string());
        }
        Ok(ToolOutcome {
            text: lines.join("\n"),
            image_png: state.image_png,
            is_error: false,
        })
    }

    fn tool_element(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        let spec = get_str_opt(args, "app")
            .ok_or_else(|| "missing required argument `app`".to_string())?;
        let action_name = get_str_opt(args, "action")
            .unwrap_or("")
            .to_ascii_lowercase();
        let identity = self.resolve_app(spec)?;
        self.gate_app(&identity)?;
        let snapshot = self
            .app_snapshots
            .get(&identity.label())
            .cloned()
            .ok_or_else(|| {
                format!(
                    "no app-state snapshot of `{}`; call computer_app_state on it first",
                    identity.label()
                )
            })?;
        let index = |args: &Value| -> Result<usize, String> {
            get_u64_opt(args, "index", u64::MAX, u64::from(usize::MAX as u32))
                .map(|n| n as usize)
                .and_then(|n| {
                    if n == usize::MAX {
                        Err(format!(
                            "`index` is required for action={action_name}; take it from the last computer_app_state"
                        ))
                    } else if n >= snapshot.node_count {
                        Err(format!(
                            "index {n} is out of range; the last app-state of `{}` listed {} elements",
                            identity.label(),
                            snapshot.node_count
                        ))
                    } else {
                        Ok(n)
                    }
                })
        };
        let window_point = |args: &Value, xk: &str, yk: &str| -> Result<(f64, f64), String> {
            let x = get_f64(args, xk)?;
            let y = get_f64(args, yk)?;
            snapshot.to_window(x, y)
        };
        let action = match action_name.as_str() {
            "press" => ElementAction::Press {
                index: index(args)?,
            },
            "set_value" => {
                let value = get_str_opt(args, "value")
                    .ok_or_else(|| "`value` is required for action=set_value".to_string())?
                    .to_string();
                ElementAction::SetValue {
                    index: index(args)?,
                    value,
                }
            }
            "menu" => ElementAction::Menu {
                index: index(args)?,
            },
            "scroll" => {
                let dir = ScrollDir::parse(get_str_opt(args, "direction").unwrap_or("down"))?;
                let pages =
                    get_u64_opt(args, "pages", 1, u64::from(MAX_SCROLL_AMOUNT))?.max(1) as u32;
                let index = match get_str_opt(args, "index")
                    .or(args.get("index").and_then(Value::as_u64).map(|_| ""))
                {
                    Some(_) => Some(get_u64_opt(args, "index", u64::MAX, u64::MAX)? as usize),
                    None => None,
                };
                ElementAction::Scroll {
                    index,
                    point: None,
                    dir,
                    pages,
                }
            }
            "select_text" => ElementAction::SelectText {
                index: index(args)?,
                start: get_u64_opt(args, "start", 0, u64::MAX)? as usize,
                end: get_u64_opt(args, "end", u64::MAX, u64::MAX)? as usize,
            },
            "click" => {
                let (x, y) = window_point(args, "x", "y")?;
                ElementAction::Click {
                    x,
                    y,
                    button: Button::parse(get_str_opt(args, "button").unwrap_or("left"))?,
                    clicks: get_u64_opt(args, "clicks", 1, 3)?.max(1) as u32,
                    hold_ms: get_u64_opt(args, "hold_ms", 0, MAX_HOLD_MS)?,
                }
            }
            "type" => {
                let text = get_str_opt(args, "text")
                    .ok_or_else(|| "`text` is required for action=type".to_string())?;
                if text.is_empty() {
                    return Err("text must not be empty".to_string());
                }
                ElementAction::Type {
                    text: text.to_string(),
                }
            }
            "key" => {
                let keys = get_str_opt(args, "keys")
                    .ok_or_else(|| "`keys` is required for action=key".to_string())?;
                ElementAction::Key {
                    combo: keys::parse_combo(keys)?,
                }
            }
            "drag" => {
                let from = window_point(args, "x", "y")?;
                let to = window_point(args, "to_x", "to_y")?;
                ElementAction::Drag {
                    from,
                    to,
                    duration_ms: get_u64_opt(args, "duration_ms", 500, MAX_DRAG_MS)?,
                }
            }
            other => {
                return Err(format!(
                    "unknown element action `{other}`; expected press, set_value, menu, scroll, select_text, click, type, key, or drag"
                ));
            }
        };
        let receipt = self
            .element_driver()?
            .act(&identity, action)
            .map_err(|e| e.to_string())?;
        let mut text = receipt.text;
        if let Some(verified) = receipt.verified {
            text.push_str(&format!("\nverified: {verified}"));
        }
        if let Some(moved) = receipt.moved {
            text.push_str(&format!(
                "\nmoved: {moved}{}",
                if moved {
                    ""
                } else {
                    " (end of content or no scrollable area)"
                }
            ));
        }
        Ok(ToolOutcome::ok(text))
    }

    fn tool_raise(&mut self, args: &Value) -> Result<ToolOutcome, String> {
        let spec = get_str_opt(args, "app")
            .ok_or_else(|| "missing required argument `app`".to_string())?;
        let identity = self.resolve_app(spec)?;
        self.gate_app(&identity)?;
        self.element_driver()?
            .raise(&identity)
            .map_err(|e| e.to_string())?;
        Ok(self.after_action(format!(
            "raised {} to the foreground (visible fallback; background actions stay preferred)",
            identity.label()
        )))
    }

    /// Route an app-targeted variant of a foreground action tool. Returns
    /// `Some(outcome)` when `app` is set.
    fn routed_app_action(
        &mut self,
        args: &Value,
        tool: &str,
    ) -> Result<Option<ToolOutcome>, String> {
        let Some(spec) = get_str_opt(args, "app") else {
            return Ok(None);
        };
        let identity = self.resolve_app(spec)?;
        self.gate_app(&identity)?;
        let snapshot = self.app_snapshots.get(&identity.label()).cloned().ok_or_else(|| {
            format!(
                "no app-state snapshot of `{}`; call computer_app_state on it before acting on it",
                identity.label()
            )
        })?;
        let point = |xk: &str, yk: &str| -> Result<(f64, f64), String> {
            let x = get_f64(args, xk)?;
            let y = get_f64(args, yk)?;
            snapshot.to_window(x, y)
        };
        let action = match tool {
            "computer_click" => {
                let (x, y) = point("x", "y")?;
                ElementAction::Click {
                    x,
                    y,
                    button: Button::parse(get_str_opt(args, "button").unwrap_or("left"))?,
                    clicks: get_u64_opt(args, "clicks", 1, 3)?.max(1) as u32,
                    hold_ms: get_u64_opt(args, "hold_ms", 0, MAX_HOLD_MS)?,
                }
            }
            "computer_scroll" => {
                let (x, y) = point("x", "y")?;
                ElementAction::Scroll {
                    index: None,
                    point: Some((x, y)),
                    dir: ScrollDir::parse(get_str_opt(args, "direction").unwrap_or(""))?,
                    pages: get_u64_opt(args, "amount", 3, u64::from(MAX_SCROLL_AMOUNT))?.max(1)
                        as u32,
                }
            }
            "computer_type" => {
                let text = get_str_opt(args, "text")
                    .ok_or_else(|| "missing required argument `text`".to_string())?;
                let count = text.chars().count();
                if count == 0 {
                    return Err("text must not be empty".to_string());
                }
                if count > MAX_TYPE_CHARS {
                    return Err(format!(
                        "text is {count} characters; the limit is {MAX_TYPE_CHARS}"
                    ));
                }
                ElementAction::Type {
                    text: text.to_string(),
                }
            }
            "computer_key" => {
                let keys = get_str_opt(args, "keys")
                    .ok_or_else(|| "missing required argument `keys`".to_string())?;
                ElementAction::Key {
                    combo: keys::parse_combo(keys)?,
                }
            }
            "computer_drag" => {
                let from = point("from_x", "from_y")?;
                let to = point("to_x", "to_y")?;
                ElementAction::Drag {
                    from,
                    to,
                    duration_ms: get_u64_opt(args, "duration_ms", 500, MAX_DRAG_MS)?,
                }
            }
            _ => return Err(format!("internal: {tool} cannot be app-routed")),
        };
        let receipt = self
            .element_driver()?
            .act(&identity, action)
            .map_err(|e| e.to_string())?;
        let mut text = format!("(background, app = {}) {}", identity.label(), receipt.text);
        if let Some(verified) = receipt.verified {
            text.push_str(&format!("\nverified: {verified}"));
        }
        if let Some(moved) = receipt.moved {
            text.push_str(&format!("\nmoved: {moved}"));
        }
        Ok(Some(ToolOutcome::ok(text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::mock::{Call, MockDriver};

    fn session(width: u32, height: u32) -> (Session, std::rc::Rc<std::cell::RefCell<Vec<Call>>>) {
        let (driver, calls) = MockDriver::new(width, height);
        let cfg = Config {
            settle_ms_desktop: 0,
            settle_ms_device: 0,
            ..Config::default()
        };
        (Session::new(Box::new(driver), cfg), calls)
    }

    #[test]
    fn screenshot_sets_frame_and_click_maps_to_device_pixels() {
        let (mut s, calls) = session(2560, 1600);
        let shot = s.call("computer_screenshot", &json!({}));
        assert!(!shot.is_error);
        assert!(shot.image_png.is_some());
        assert!(
            shot.text
                .starts_with("frame: 1024x640 (device 2560x1600, scale 2.500)"),
            "{}",
            shot.text
        );

        let click = s.call("computer_click", &json!({"x": 512, "y": 320}));
        assert!(!click.is_error, "{}", click.text);
        assert!(click.image_png.is_some(), "post-action screenshot attached");
        let recorded = calls.borrow();
        assert!(
            matches!(recorded[1], Call::Click(Point { x, y }, Button::Left, 1, 0) if x == 1280.0 && y == 800.0),
            "{recorded:?}"
        );
        assert_eq!(
            recorded.len(),
            3,
            "screenshot, click, post-action screenshot"
        );
    }

    #[test]
    fn actions_require_a_frame_and_stay_in_bounds() {
        let (mut s, _) = session(800, 600);
        let out = s.call("computer_click", &json!({"x": 10, "y": 10}));
        assert!(out.is_error);
        assert!(out.text.contains("computer_screenshot"), "{}", out.text);
        s.call("computer_screenshot", &json!({}));
        let out = s.call("computer_click", &json!({"x": 900, "y": 10}));
        assert!(out.is_error);
        assert!(
            out.text.contains("outside the current frame"),
            "{}",
            out.text
        );
    }

    #[test]
    fn zoom_frame_coordinates_are_mapped() {
        let (mut s, calls) = session(2000, 1000);
        s.cfg.max_edge = 1000; // frame 1000x500 keeps the arithmetic obvious
        s.call("computer_screenshot", &json!({}));
        let zoom = s.call(
            "computer_zoom",
            &json!({"x": 100, "y": 50, "width": 200, "height": 100}),
        );
        assert!(!zoom.is_error, "{}", zoom.text);
        assert!(zoom.image_png.is_some());
        assert!(zoom.text.contains("x=100..300"), "{}", zoom.text);
        // Zoom image is 1000x500; its center is frame (200, 100) → device (400, 200).
        let click = s.call(
            "computer_click",
            &json!({"x": 500, "y": 250, "frame": "zoom"}),
        );
        assert!(!click.is_error, "{}", click.text);
        let recorded = calls.borrow();
        let click_call = recorded
            .iter()
            .find(|c| matches!(c, Call::Click(..)))
            .unwrap();
        match click_call {
            Call::Click(p, ..) => {
                assert!(
                    (p.x - 400.0).abs() < 1.0 && (p.y - 200.0).abs() < 1.0,
                    "{p:?}"
                );
            }
            _ => unreachable!(),
        }
        drop(recorded);
        // Zoom is stale after the action.
        let out = s.call("computer_click", &json!({"x": 1, "y": 1, "frame": "zoom"}));
        assert!(out.is_error);
        assert!(out.text.contains("no current zoom"), "{}", out.text);
    }

    #[test]
    fn observe_mode_blocks_input_but_allows_screenshots() {
        let (driver, calls) = MockDriver::new(800, 600);
        let cfg = Config {
            mode: Mode::Observe,
            ..Config::default()
        };
        let mut s = Session::new(Box::new(driver), cfg);
        assert!(!s.call("computer_screenshot", &json!({})).is_error);
        let out = s.call("computer_type", &json!({"text": "hi"}));
        assert!(out.is_error);
        assert!(out.text.contains("observe mode"), "{}", out.text);
        assert!(!calls.borrow().iter().any(|c| matches!(c, Call::Type(_))));
    }

    #[test]
    fn key_type_scroll_drag_and_limits() {
        let (mut s, calls) = session(800, 600);
        s.call("computer_screenshot", &json!({}));
        assert!(!s.call("computer_key", &json!({"keys": "ctrl+s"})).is_error);
        assert!(!s.call("computer_type", &json!({"text": "hello"})).is_error);
        assert!(
            !s.call(
                "computer_scroll",
                &json!({"x": 10, "y": 10, "direction": "down"})
            )
            .is_error
        );
        assert!(
            !s.call(
                "computer_drag",
                &json!({"from_x": 1, "from_y": 1, "to_x": 50, "to_y": 50})
            )
            .is_error
        );
        let recorded = calls.borrow();
        assert!(recorded.contains(&Call::Key("ctrl+s".into())));
        assert!(recorded.contains(&Call::Type("hello".into())));
        assert!(
            recorded
                .iter()
                .any(|c| matches!(c, Call::Scroll(_, ScrollDir::Down, 3)))
        );
        assert!(recorded.iter().any(|c| matches!(c, Call::Drag(_, _, 500))));
        drop(recorded);
        let too_long = "x".repeat(MAX_TYPE_CHARS + 1);
        assert!(s.call("computer_type", &json!({"text": too_long})).is_error);
        assert!(
            s.call("computer_wait", &json!({"ms": MAX_WAIT_MS + 1}))
                .is_error
        );
        assert!(
            s.call("computer_key", &json!({"keys": "ctrl+bogus"}))
                .is_error
        );
        assert!(
            s.call(
                "computer_scroll",
                &json!({"x": 1, "y": 1, "direction": "sideways"})
            )
            .is_error
        );
        assert!(s.call("computer_nope", &json!({})).is_error);
    }

    #[test]
    fn ui_tree_lists_interactive_nodes_in_frame_space() {
        let (mut driver, _) = MockDriver::new(2000, 1000);
        driver.kind = TargetKind::Android;
        driver.nodes = vec![
            UiNode {
                class: "Button".into(),
                label: "OK".into(),
                id: "com.app:id/ok".into(),
                bounds: (1000, 500, 1200, 600),
                clickable: true,
                scrollable: false,
                editable: false,
                focused: false,
                depth: 3,
            },
            UiNode {
                class: "View".into(),
                label: String::new(),
                id: String::new(),
                bounds: (0, 0, 10, 10),
                clickable: false,
                scrollable: false,
                editable: false,
                focused: false,
                depth: 1,
            },
        ];
        let cfg = Config {
            settle_ms_device: 0,
            ..Config::default()
        };
        let mut s = Session::new(Box::new(driver), cfg);
        let out = s.call("computer_ui_tree", &json!({}));
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains(
                "#0 [Button] \"OK\" id=com.app:id/ok center=(563,282) size=102x51 clickable"
            ),
            "{}",
            out.text
        );
        assert!(out.text.contains("1 of 1"), "{}", out.text);
    }

    #[test]
    fn catalog_matches_tool_names() {
        let catalog = tool_catalog();
        let names: Vec<&str> = catalog
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, TOOL_NAMES);
        for tool in &catalog {
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["description"].as_str().unwrap().len() > 20);
        }
    }
}
