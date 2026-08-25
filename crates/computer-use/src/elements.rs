//! Element-level perception and action: the [`ElementDriver`] contract.
//!
//! Phase 1 speaks pixels to a whole screen; phase 2 speaks **apps and
//! elements**. An [`ElementDriver`] lists running apps, captures one
//! app/window at a time (image + indexed element tree), and acts on that
//! app without touching the user's cursor or foreground (where the platform
//! supports it — macOS). Platform support is reported per driver in
//! [`ElementCaps`]; the session translates gaps into actionable errors.

use crate::consent::AppIdentity;
use crate::driver::{Button, DriverError, ScrollDir};
use crate::keys::KeyCombo;

/// One window of an app as the directory reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    /// Platform window id (CGWindowID on macOS, HWND-derived on Windows).
    pub id: u32,
    pub title: String,
    /// Window frame in the platform's screen space (points on macOS,
    /// physical pixels elsewhere; drivers convert internally).
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// A running app and its windows.
#[derive(Debug, Clone, PartialEq)]
pub struct AppInfo {
    pub identity: AppIdentity,
    pub windows: Vec<WindowInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StateMode {
    /// Window image + indexed element tree.
    #[default]
    Full,
    /// Window image only.
    Image,
    /// Element tree only.
    Ax,
}

impl StateMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "full" | "both" => Ok(Self::Full),
            "image" | "screenshot" | "pixels" => Ok(Self::Image),
            "ax" | "tree" | "elements" | "text" => Ok(Self::Ax),
            other => Err(format!(
                "unknown mode `{other}`; expected full, image, or ax"
            )),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Image => "image",
            Self::Ax => "ax",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StateOpts {
    pub mode: StateMode,
    /// Case-insensitive substring filter over node text and roles.
    pub filter: Option<String>,
    /// Restrict capture to one window of the app.
    pub window_id: Option<u32>,
    /// Cap on reported interactive nodes.
    pub max_nodes: usize,
    /// Longest edge of the returned window image (0 = driver default).
    pub max_edge: u32,
}

/// One element in a captured window, in snapshot order.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementNode {
    pub index: usize,
    /// Accessibility role or widget class ("AXButton", "Button").
    pub role: String,
    /// Title / value / description, whichever the element exposes.
    pub title: String,
    /// Frame in **window-image pixels** of the snapshot that listed it.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Supported element actions ("press", "menu", "set_value", …).
    pub actions: Vec<String>,
    pub focused: bool,
    pub enabled: bool,
    /// Secure text field: never receives set_value/type.
    pub secure: bool,
    pub depth: u8,
}

impl ElementNode {
    pub fn is_interactive(&self) -> bool {
        !self.actions.is_empty()
            || self.focused
            || self.secure
            || matches!(
                self.role.as_str(),
                "AXTextField" | "AXTextArea" | "AXComboBox" | "TextField" | "EditText"
            )
    }

    pub fn center(&self) -> (f64, f64) {
        (
            f64::from(self.x) + f64::from(self.w) / 2.0,
            f64::from(self.y) + f64::from(self.h) / 2.0,
        )
    }
}

/// The result of `app_state`: one window as image + indexed tree.
#[derive(Debug, Clone)]
pub struct AppState {
    pub identity: AppIdentity,
    pub window: WindowInfo,
    /// PNG of the window, scaled to the model budget. Window-local pixels
    /// of this image are the coordinate space for background clicks.
    pub image_png: Option<Vec<u8>>,
    pub image_w: u32,
    pub image_h: u32,
    pub nodes: Vec<ElementNode>,
    /// Nodes hidden by the cap/filter (for the "N more omitted" line).
    pub omitted: usize,
    /// The window is fully covered by other windows (capture still works).
    pub occluded: bool,
}

/// What a driver can do with elements on this platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ElementCaps {
    /// Element tree capture is available.
    pub tree: bool,
    /// Window-scoped image capture is available.
    pub window_image: bool,
    /// Actions reach the app without foreground or cursor movement.
    pub background_actions: bool,
    /// Short note for `computer_info`.
    pub note: &'static str,
}

/// One action on one app. Coordinates are window-local in the platform's
/// native units (points on macOS), mapped by the session from the last
/// snapshot's image pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum ElementAction {
    /// AXPress on the indexed element; falls back to a background click at
    /// its center.
    Press {
        index: usize,
    },
    Click {
        x: f64,
        y: f64,
        button: Button,
        clicks: u32,
        hold_ms: u64,
    },
    /// Write `value` to the indexed element with read-back verification;
    /// text-field fallback is focus + select-all + type.
    SetValue {
        index: usize,
        value: String,
    },
    Type {
        text: String,
    },
    Key {
        combo: KeyCombo,
    },
    /// Open the indexed element's menu (AXShowMenu / secondary action).
    Menu {
        index: usize,
    },
    /// Scroll by whole pages at the indexed element or a point; the receipt
    /// reports whether the window actually moved.
    Scroll {
        index: Option<usize>,
        point: Option<(f64, f64)>,
        dir: ScrollDir,
        pages: u32,
    },
    /// Select a character range in the indexed text element.
    SelectText {
        index: usize,
        start: usize,
        end: usize,
    },
    Drag {
        from: (f64, f64),
        to: (f64, f64),
        duration_ms: u64,
    },
}

/// What the driver observed after acting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActionReceipt {
    /// Operator phrasing: "pressed button \"OK\"".
    pub text: String,
    /// Read-back result for writes (Some(true) = value confirmed).
    pub verified: Option<bool>,
    /// Scroll movement detection (Some(false) = end of content).
    pub moved: Option<bool>,
}

/// The app-targeted half of a driver. Phase-1 [`crate::driver::Driver`]
/// stays for whole-screen/foreground work; desktop drivers that can address
/// apps implement both (exposed through
/// `Driver::element`), device drivers map element actions onto their UI
/// trees.
pub trait ElementDriver {
    /// Running apps with windows, in no particular order.
    fn apps(&mut self) -> Result<Vec<AppInfo>, DriverError>;

    /// Capture one window of `app` (image and/or indexed tree) and cache
    /// the snapshot so index-based actions can refer to it.
    fn app_state(&mut self, app: &AppIdentity, opts: &StateOpts) -> Result<AppState, DriverError>;

    /// Perform one action against `app`, using the last snapshot where an
    /// index is involved.
    fn act(
        &mut self,
        app: &AppIdentity,
        action: ElementAction,
    ) -> Result<ActionReceipt, DriverError>;

    /// Explicit, visible fallback: bring the app's window forward.
    fn raise(&mut self, app: &AppIdentity) -> Result<(), DriverError>;

    fn caps(&self) -> ElementCaps;
}

/// Render an element tree the way the model sees it: one line per
/// interactive node with a stable `#index`, role, title, and flags.
pub fn render_nodes(nodes: &[ElementNode], filter: Option<&str>, max: usize) -> (String, usize) {
    let mut lines = Vec::new();
    let mut shown = 0usize;
    let mut total = 0usize;
    for node in nodes {
        if !node.is_interactive() {
            continue;
        }
        if let Some(needle) = filter
            && !needle.is_empty()
        {
            let needle = needle.to_lowercase();
            let haystack = format!("{} {}", node.role, node.title).to_lowercase();
            if !haystack.contains(&needle) {
                continue;
            }
        }
        total += 1;
        if shown >= max {
            continue;
        }
        let mut flags: Vec<&str> = Vec::new();
        if node.focused {
            flags.push("focused");
        }
        if !node.enabled {
            flags.push("disabled");
        }
        if node.secure {
            flags.push("secure");
        }
        let title = if node.title.is_empty() {
            String::new()
        } else {
            let trimmed: String = node.title.chars().take(80).collect();
            format!(" \"{}\"", trimmed.replace('\n', "\\n"))
        };
        let acts = if node.actions.is_empty() {
            String::new()
        } else {
            format!(" {}", node.actions.join(","))
        };
        let flag_text = if flags.is_empty() {
            String::new()
        } else {
            format!(" {}", flags.join(","))
        };
        lines.push(format!(
            "#{index} [{role}]{title} at ({x},{y} {w}x{h}){flag_text}{acts}",
            index = node.index,
            role = node.role,
            x = node.x,
            y = node.y,
            w = node.w,
            h = node.h,
        ));
        shown += 1;
    }
    (lines.join("\n"), total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(index: usize, role: &str, title: &str, actions: &[&str]) -> ElementNode {
        ElementNode {
            index,
            role: role.into(),
            title: title.into(),
            x: index as u32 * 10,
            y: 5,
            w: 80,
            h: 20,
            actions: actions.iter().map(|s| s.to_string()).collect(),
            focused: false,
            enabled: true,
            secure: false,
            depth: 2,
        }
    }

    #[test]
    fn mode_parsing() {
        assert_eq!(StateMode::parse("full").unwrap(), StateMode::Full);
        assert_eq!(StateMode::parse("IMAGE").unwrap(), StateMode::Image);
        assert_eq!(StateMode::parse("tree").unwrap(), StateMode::Ax);
        assert_eq!(StateMode::parse("").unwrap(), StateMode::Full);
        assert!(StateMode::parse("soup").is_err());
    }

    #[test]
    fn render_lists_interactive_nodes_only_with_cap() {
        let nodes = vec![
            node(0, "AXWindow", "Main", &[]),
            node(1, "AXButton", "OK", &["press"]),
            node(2, "AXTextField", "search", &[]),
            node(3, "AXButton", "Cancel", &["press"]),
        ];
        let (text, total) = render_nodes(&nodes, None, 2);
        assert_eq!(total, 3);
        assert!(text.contains("#1 [AXButton] \"OK\""));
        assert!(text.contains("#2 [AXTextField] \"search\""));
        assert!(!text.contains("Cancel"), "cap of 2 nodes");
        assert!(!text.contains("AXWindow"), "non-interactive hidden");
    }

    #[test]
    fn render_filters_case_insensitively() {
        let nodes = vec![
            node(0, "AXButton", "Save Draft", &["press"]),
            node(1, "AXButton", "Delete", &["press"]),
        ];
        let (text, total) = render_nodes(&nodes, Some("save"), 10);
        assert_eq!(total, 1);
        assert!(text.contains("Save Draft"));
        assert!(!text.contains("Delete"));
    }

    #[test]
    fn secure_fields_are_interactive_but_flagged() {
        let mut node = node(0, "AXSecureTextField", "", &[]);
        node.secure = true;
        assert!(node.is_interactive());
        let (text, _) = render_nodes(&[node], None, 10);
        assert!(text.contains("secure"));
    }

    #[test]
    fn center_math() {
        let node = node(0, "AXButton", "", &[]);
        let (cx, cy) = node.center();
        assert_eq!((cx, cy), (40.0, 15.0));
    }
}
