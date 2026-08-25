//! The platform-neutral driver contract every backend implements.
//!
//! All coordinates crossing this boundary are **device pixels** of the
//! captured display (the pixel grid of the screenshot the driver returned).
//! Backends that post input in other units (macOS points, Windows normalized
//! absolute coordinates) convert internally using the scale they reported in
//! [`TargetInfo`].

use std::fmt;

use crate::elements::ElementDriver;
use crate::keys::KeyCombo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "left" | "" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "middle" => Ok(Self::Middle),
            other => Err(format!(
                "unknown button `{other}`; expected left, right, or middle"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    Up,
    Down,
    Left,
    Right,
}

impl ScrollDir {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            other => Err(format!(
                "unknown scroll direction `{other}`; expected up, down, left, or right"
            )),
        }
    }
}

/// A point in device pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Desktop,
    Android,
    Harmony,
}

impl TargetKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Android => "android",
            Self::Harmony => "harmony",
        }
    }
}

/// What the model is told about the target in `computer_info`.
#[derive(Debug, Clone)]
pub struct TargetInfo {
    pub kind: TargetKind,
    /// Human-readable backend name, e.g. `macos-coregraphics`, `adb`.
    pub driver: String,
    /// Captured display size in device pixels.
    pub device_w: u32,
    pub device_h: u32,
    /// Free-form diagnostics: permissions, helper binaries found, device id.
    pub notes: Vec<String>,
    pub supports_ui_tree: bool,
    pub supports_apps: bool,
}

/// An encoded screenshot straight from the backend (PNG or JPEG bytes).
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub bytes: Vec<u8>,
}

/// One interactive element from an accessibility/layout dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiNode {
    pub class: String,
    /// Visible text or content description.
    pub label: String,
    /// Resource id / component id when present.
    pub id: String,
    /// `(left, top, right, bottom)` in device pixels.
    pub bounds: (u32, u32, u32, u32),
    pub clickable: bool,
    pub scrollable: bool,
    pub editable: bool,
    pub focused: bool,
    pub depth: u8,
}

impl UiNode {
    pub fn center(&self) -> (u32, u32) {
        let (l, t, r, b) = self.bounds;
        ((l + r) / 2, (t + b) / 2)
    }

    pub fn is_interesting(&self) -> bool {
        self.clickable
            || self.scrollable
            || self.editable
            || self.focused
            || !self.label.trim().is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction<'a> {
    Launch(&'a str),
    List,
    Current,
}

#[derive(Debug)]
pub enum DriverError {
    /// The backend cannot do this on this target (e.g. UI tree on desktop).
    Unsupported(String),
    /// A helper binary or device is missing; message names the fix.
    Unavailable(String),
    /// The OS refused (permissions).
    Permission(String),
    /// Anything else, already phrased for the operator.
    Failed(String),
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(m) => write!(f, "unsupported on this target: {m}"),
            Self::Unavailable(m) => write!(f, "unavailable: {m}"),
            Self::Permission(m) => write!(f, "permission denied: {m}"),
            Self::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for DriverError {}

impl DriverError {
    pub fn failed(msg: impl Into<String>) -> Self {
        Self::Failed(msg.into())
    }
}

pub trait Driver {
    fn info(&mut self) -> Result<TargetInfo, DriverError>;
    fn screenshot(&mut self) -> Result<RawFrame, DriverError>;
    fn move_to(&mut self, p: Point) -> Result<(), DriverError>;
    /// `clicks` is 1..=3; `hold_ms > 0` means press-and-hold (long press).
    fn click(
        &mut self,
        p: Point,
        button: Button,
        clicks: u32,
        hold_ms: u64,
    ) -> Result<(), DriverError>;
    fn drag(&mut self, from: Point, to: Point, duration_ms: u64) -> Result<(), DriverError>;
    /// `amount` is in notches (desktop) or roughly half-screens/5 (devices).
    fn scroll(&mut self, p: Point, dir: ScrollDir, amount: u32) -> Result<(), DriverError>;
    fn type_text(&mut self, text: &str) -> Result<(), DriverError>;
    fn key(&mut self, combo: &KeyCombo) -> Result<(), DriverError>;
    fn ui_tree(&mut self) -> Result<Vec<UiNode>, DriverError>;
    fn app(&mut self, action: AppAction<'_>) -> Result<String, DriverError>;
    fn devices(&mut self) -> Result<String, DriverError>;

    /// App/element-level control (phase 2). Desktop drivers that can address
    /// one app at a time return `Some(self)`; everyone else inherits `None`.
    fn element(&mut self) -> Option<&mut dyn ElementDriver> {
        None
    }
}
