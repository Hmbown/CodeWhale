//! macOS driver: `screencapture` for pixels, CoreGraphics events for input.
//!
//! No crates: the handful of CoreGraphics/ApplicationServices symbols are
//! declared here. Mouse and keyboard events are posted at the HID tap in
//! display points; the driver converts from device pixels using the
//! backing scale of the main display.
//!
//! Permissions: Screen Recording (for `screencapture`) and Accessibility
//! (for posting events) must be granted to the terminal app that runs
//! Codewhale.

use std::ffi::c_void;
use std::time::Duration;

use crate::driver::{
    AppAction, Button, Driver, DriverError, Point, RawFrame, ScrollDir, TargetInfo, TargetKind,
    UiNode,
};
use crate::keys::{Key, KeyCombo, NamedKey};
use crate::process;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

type CGEventRef = *mut c_void;
type CGEventSourceRef = *mut c_void;

const K_CG_HID_EVENT_TAP: u32 = 0;
const K_CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const K_CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
const K_CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const K_CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
const K_CG_EVENT_MOUSE_MOVED: u32 = 5;
const K_CG_EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
const K_CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const K_CG_EVENT_OTHER_MOUSE_UP: u32 = 26;
const K_CG_MOUSE_BUTTON_LEFT: u32 = 0;
const K_CG_MOUSE_BUTTON_RIGHT: u32 = 1;
const K_CG_MOUSE_BUTTON_CENTER: u32 = 2;
const K_CG_MOUSE_EVENT_CLICK_STATE: u32 = 1;
const K_CG_SCROLL_EVENT_UNIT_LINE: u32 = 1;
const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_CONTROL: u64 = 1 << 18;
const FLAG_ALTERNATE: u64 = 1 << 19;
const FLAG_COMMAND: u64 = 1 << 20;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGDisplayCopyDisplayMode(display: u32) -> *mut c_void;
    fn CGDisplayModeGetPixelWidth(mode: *const c_void) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: *const c_void) -> usize;
    fn CGDisplayModeRelease(mode: *mut c_void);
    fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        mouse_type: u32,
        point: CGPoint,
        button: u32,
    ) -> CGEventRef;
    fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        keycode: u16,
        keydown: bool,
    ) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent2(
        source: CGEventSourceRef,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventKeyboardSetUnicodeString(event: CGEventRef, length: usize, string: *const u16);
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    static kCFBooleanTrue: *const c_void;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *const c_void;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    static kAXTrustedCheckOptionPrompt: *const c_void;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
}

/// Current permission state: `(accessibility, screen_recording)`.
pub fn permission_status() -> (bool, bool) {
    // SAFETY: no preconditions.
    unsafe { (AXIsProcessTrusted(), CGPreflightScreenCaptureAccess()) }
}

/// Ask macOS to show the Accessibility and Screen Recording prompts for the
/// current process (the terminal app) and return the resulting state. macOS
/// only adds the app to the list; the user still flips the toggles.
pub fn request_permissions() -> (bool, bool) {
    // SAFETY: builds a one-entry CFDictionary {kAXTrustedCheckOptionPrompt: true}
    // from valid CoreFoundation constants and releases it afterwards.
    let accessibility = unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const u8 as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const u8 as *const c_void,
        );
        let trusted = AXIsProcessTrustedWithOptions(options);
        if !options.is_null() {
            CFRelease(options);
        }
        trusted
    };
    // SAFETY: no preconditions.
    let screen = unsafe { CGRequestScreenCaptureAccess() };
    (accessibility, screen)
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// The shared `kCFBooleanTrue` constant, usable as an AX attribute value.
pub(crate) fn cf_boolean_true() -> *mut c_void {
    // SAFETY: kCFBooleanTrue is a global CF constant; borrowing is fine.
    unsafe { kCFBooleanTrue as *mut c_void }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

fn post(event: CGEventRef) -> Result<(), DriverError> {
    if event.is_null() {
        return Err(DriverError::Failed(
            "CoreGraphics refused to create the event".to_string(),
        ));
    }
    // SAFETY: `event` is a valid CGEvent we own; post then release.
    unsafe {
        CGEventPost(K_CG_HID_EVENT_TAP, event);
        CFRelease(event);
    }
    Ok(())
}

fn post_with_flags(event: CGEventRef, flags: u64) -> Result<(), DriverError> {
    if event.is_null() {
        return Err(DriverError::Failed(
            "CoreGraphics refused to create the event".to_string(),
        ));
    }
    // SAFETY: event is valid and owned.
    unsafe {
        if flags != 0 {
            CGEventSetFlags(event, flags);
        }
    }
    post(event)
}

pub struct MacDriver {
    display: u32,
    logical_w: f64,
    logical_h: f64,
    pixel_w: u32,
    pixel_h: u32,
    pub(crate) element_snapshot: Option<crate::drivers::macos_ax::MacElementSnapshot>,
}

impl MacDriver {
    pub fn new() -> Result<Self, DriverError> {
        // SAFETY: plain CoreGraphics queries; the display mode is released.
        let (display, bounds, pw, ph) = unsafe {
            let display = CGMainDisplayID();
            let bounds = CGDisplayBounds(display);
            // In HiDPI ("looks like") modes CGDisplayPixelsWide reports
            // points; the display mode carries the real backing size that
            // `screencapture` produces.
            let mode = CGDisplayCopyDisplayMode(display);
            let (pw, ph) = if mode.is_null() {
                (bounds.size.width as usize, bounds.size.height as usize)
            } else {
                let dims = (
                    CGDisplayModeGetPixelWidth(mode),
                    CGDisplayModeGetPixelHeight(mode),
                );
                CGDisplayModeRelease(mode);
                dims
            };
            (display, bounds, pw, ph)
        };
        Ok(Self {
            display,
            logical_w: bounds.size.width,
            logical_h: bounds.size.height,
            pixel_w: pw as u32,
            pixel_h: ph as u32,
            element_snapshot: None,
        })
    }

    fn scale(&self) -> f64 {
        if self.logical_w > 0.0 {
            f64::from(self.pixel_w) / self.logical_w
        } else {
            1.0
        }
    }

    /// Backing scale exposed to the element backend.
    pub(crate) fn backing_scale(&self) -> f64 {
        self.scale()
    }

    fn to_points(&self, p: Point) -> CGPoint {
        let s = self.scale();
        CGPoint {
            x: (p.x / s).clamp(0.0, (self.logical_w - 1.0).max(0.0)),
            y: (p.y / s).clamp(0.0, (self.logical_h - 1.0).max(0.0)),
        }
    }

    fn accessibility_ok() -> bool {
        // SAFETY: no preconditions.
        unsafe { AXIsProcessTrusted() }
    }

    fn require_accessibility() -> Result<(), DriverError> {
        if Self::accessibility_ok() {
            Ok(())
        } else {
            Err(DriverError::Permission(
                "Accessibility is not granted: open System Settings → Privacy & Security → Accessibility and enable the terminal app running Codewhale, then restart it".to_string(),
            ))
        }
    }

    pub(crate) fn accessibility_granted() -> bool {
        Self::accessibility_ok()
    }

    pub(crate) fn need_accessibility() -> Result<(), DriverError> {
        Self::require_accessibility()
    }

    fn mouse(
        &self,
        kind: u32,
        p: CGPoint,
        button: u32,
        click_state: i64,
        flags: u64,
    ) -> Result<(), DriverError> {
        // SAFETY: creating an event with valid arguments; ownership handled in post.
        let event = unsafe { CGEventCreateMouseEvent(std::ptr::null_mut(), kind, p, button) };
        if event.is_null() {
            return Err(DriverError::Failed(
                "CGEventCreateMouseEvent returned null".to_string(),
            ));
        }
        if click_state > 0 {
            // SAFETY: event is valid.
            unsafe {
                CGEventSetIntegerValueField(event, K_CG_MOUSE_EVENT_CLICK_STATE, click_state)
            };
        }
        post_with_flags(event, flags)
    }

    fn key_event(keycode: u16, down: bool, flags: u64) -> Result<(), DriverError> {
        // SAFETY: valid arguments.
        let event = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), keycode, down) };
        post_with_flags(event, flags)
    }

    fn type_unicode(chunk: &[u16]) -> Result<(), DriverError> {
        for down in [true, false] {
            // SAFETY: valid arguments; the UTF-16 buffer outlives the call.
            let event = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), 0, down) };
            if event.is_null() {
                return Err(DriverError::Failed(
                    "CGEventCreateKeyboardEvent returned null".to_string(),
                ));
            }
            unsafe { CGEventKeyboardSetUnicodeString(event, chunk.len(), chunk.as_ptr()) };
            post(event)?;
        }
        Ok(())
    }
}

pub(crate) fn modifier_flags(combo: &KeyCombo, extra_shift: bool) -> u64 {
    let mut flags = 0;
    if combo.modifiers.ctrl {
        flags |= FLAG_CONTROL;
    }
    if combo.modifiers.alt {
        flags |= FLAG_ALTERNATE;
    }
    if combo.modifiers.shift || extra_shift {
        flags |= FLAG_SHIFT;
    }
    if combo.modifiers.meta {
        flags |= FLAG_COMMAND;
    }
    flags
}

/// US-layout virtual key codes. Returns `(keycode, needs_shift)`.
pub(crate) fn keycode(key: &Key) -> Result<(u16, bool), DriverError> {
    Ok(match key {
        Key::Named(named) => (
            match named {
                NamedKey::Enter => 36,
                NamedKey::Tab => 48,
                NamedKey::Escape => 53,
                NamedKey::Backspace => 51,
                NamedKey::Delete => 117,
                NamedKey::Space => 49,
                NamedKey::Up => 126,
                NamedKey::Down => 125,
                NamedKey::Left => 123,
                NamedKey::Right => 124,
                NamedKey::Home => 115,
                NamedKey::End => 119,
                NamedKey::PageUp => 116,
                NamedKey::PageDown => 121,
                NamedKey::Insert => 114,
                NamedKey::CapsLock => 57,
                NamedKey::F(n) => match n {
                    1 => 122,
                    2 => 120,
                    3 => 99,
                    4 => 118,
                    5 => 96,
                    6 => 97,
                    7 => 98,
                    8 => 100,
                    9 => 101,
                    10 => 109,
                    11 => 103,
                    12 => 111,
                    13 => 105,
                    14 => 107,
                    15 => 113,
                    16 => 106,
                    17 => 64,
                    18 => 79,
                    19 => 80,
                    20 => 90,
                    _ => return Err(DriverError::Failed(format!("F{n} has no macOS key code"))),
                },
                NamedKey::Back
                | NamedKey::AppHome
                | NamedKey::Recents
                | NamedKey::Power
                | NamedKey::Menu => {
                    return Err(DriverError::Unsupported(format!(
                        "{named:?} is a phone key; use cmd/ctrl shortcuts on macOS"
                    )));
                }
                NamedKey::VolumeUp => 72,
                NamedKey::VolumeDown => 73,
            },
            false,
        ),
        Key::Char(c) => {
            let lower = c.to_ascii_lowercase();
            let shifted_symbols: &[(char, char)] = &[
                ('~', '`'),
                ('!', '1'),
                ('@', '2'),
                ('#', '3'),
                ('$', '4'),
                ('%', '5'),
                ('^', '6'),
                ('&', '7'),
                ('*', '8'),
                ('(', '9'),
                (')', '0'),
                ('_', '-'),
                ('+', '='),
                ('{', '['),
                ('}', ']'),
                ('|', '\\'),
                (':', ';'),
                ('"', '\''),
                ('<', ','),
                ('>', '.'),
                ('?', '/'),
            ];
            let (base, shift) = if c.is_ascii_uppercase() {
                (lower, true)
            } else if let Some((_, b)) = shifted_symbols.iter().find(|(s, _)| *s == *c) {
                (*b, true)
            } else {
                (*c, false)
            };
            let code: u16 = match base {
                'a' => 0,
                's' => 1,
                'd' => 2,
                'f' => 3,
                'h' => 4,
                'g' => 5,
                'z' => 6,
                'x' => 7,
                'c' => 8,
                'v' => 9,
                'b' => 11,
                'q' => 12,
                'w' => 13,
                'e' => 14,
                'r' => 15,
                'y' => 16,
                't' => 17,
                '1' => 18,
                '2' => 19,
                '3' => 20,
                '4' => 21,
                '6' => 22,
                '5' => 23,
                '=' => 24,
                '9' => 25,
                '7' => 26,
                '-' => 27,
                '8' => 28,
                '0' => 29,
                ']' => 30,
                'o' => 31,
                'u' => 32,
                '[' => 33,
                'i' => 34,
                'p' => 35,
                'l' => 37,
                'j' => 38,
                '\'' => 39,
                'k' => 40,
                ';' => 41,
                '\\' => 42,
                ',' => 43,
                '/' => 44,
                'n' => 45,
                'm' => 46,
                '.' => 47,
                '`' => 50,
                ' ' => 49,
                '\n' => 36,
                '\t' => 48,
                other => {
                    return Err(DriverError::Failed(format!(
                        "no macOS key code for `{other}`; use computer_type"
                    )));
                }
            };
            (code, shift)
        }
    })
}

impl Driver for MacDriver {
    fn info(&mut self) -> Result<TargetInfo, DriverError> {
        // SAFETY: no preconditions.
        let screen_ok = unsafe { CGPreflightScreenCaptureAccess() };
        let mut notes = vec![
            format!(
                "display: id {} {}x{} points, backing scale {:.2}",
                self.display,
                self.logical_w,
                self.logical_h,
                self.scale()
            ),
            format!(
                "screen recording permission: {}",
                if screen_ok {
                    "granted"
                } else {
                    "NOT granted (System Settings → Privacy & Security → Screen Recording → enable your terminal app)"
                }
            ),
            format!(
                "accessibility permission: {}",
                if Self::accessibility_ok() {
                    "granted"
                } else {
                    "NOT granted (System Settings → Privacy & Security → Accessibility → enable your terminal app)"
                }
            ),
        ];
        if !screen_ok {
            notes.push(
                "screenshots may come back blank/desktop-only until Screen Recording is granted"
                    .to_string(),
            );
        }
        Ok(TargetInfo {
            kind: TargetKind::Desktop,
            driver: "macos-coregraphics".into(),
            device_w: self.pixel_w,
            device_h: self.pixel_h,
            notes,
            supports_ui_tree: false,
            supports_apps: true,
        })
    }

    fn screenshot(&mut self) -> Result<RawFrame, DriverError> {
        let screencapture = process::which("screencapture")
            .unwrap_or_else(|| std::path::PathBuf::from("/usr/sbin/screencapture"));
        let path = std::env::temp_dir().join(format!("codewhale-cu-{}.png", std::process::id()));
        let path_str = path.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&path);
        let out = process::run(
            &screencapture,
            // `-m` captures only the main display: the point/pixel geometry
            // above is the main display's, and multi-monitor captures would
            // otherwise write one file per screen.
            &["-x", "-C", "-m", "-t", "png", &path_str],
            Duration::from_secs(20),
        )?;
        if !out.success() {
            let _ = std::fs::remove_file(&path);
            return Err(DriverError::Failed(format!(
                "screencapture failed (status {:?}): {}",
                out.status,
                process::tail(&out.stderr, 300)
            )));
        }
        let bytes = std::fs::read(&path)
            .map_err(|e| DriverError::Failed(format!("screenshot file missing: {e}")))?;
        let _ = std::fs::remove_file(&path);
        let img = crate::frame::decode(&bytes).map_err(DriverError::Failed)?;
        use image::GenericImageView;
        let (w, h) = img.dimensions();
        // Multi-display setups can put a different display first; keep the
        // geometry consistent with what we actually captured.
        self.pixel_w = w;
        self.pixel_h = h;
        Ok(RawFrame { bytes })
    }

    fn move_to(&mut self, p: Point) -> Result<(), DriverError> {
        Self::require_accessibility()?;
        let pt = self.to_points(p);
        self.mouse(K_CG_EVENT_MOUSE_MOVED, pt, K_CG_MOUSE_BUTTON_LEFT, 0, 0)
    }

    fn click(
        &mut self,
        p: Point,
        button: Button,
        clicks: u32,
        hold_ms: u64,
    ) -> Result<(), DriverError> {
        Self::require_accessibility()?;
        let pt = self.to_points(p);
        self.mouse(K_CG_EVENT_MOUSE_MOVED, pt, K_CG_MOUSE_BUTTON_LEFT, 0, 0)?;
        std::thread::sleep(Duration::from_millis(30));
        let (down, up, btn) = match button {
            Button::Left => (
                K_CG_EVENT_LEFT_MOUSE_DOWN,
                K_CG_EVENT_LEFT_MOUSE_UP,
                K_CG_MOUSE_BUTTON_LEFT,
            ),
            Button::Right => (
                K_CG_EVENT_RIGHT_MOUSE_DOWN,
                K_CG_EVENT_RIGHT_MOUSE_UP,
                K_CG_MOUSE_BUTTON_RIGHT,
            ),
            Button::Middle => (
                K_CG_EVENT_OTHER_MOUSE_DOWN,
                K_CG_EVENT_OTHER_MOUSE_UP,
                K_CG_MOUSE_BUTTON_CENTER,
            ),
        };
        if hold_ms > 0 {
            self.mouse(down, pt, btn, 1, 0)?;
            std::thread::sleep(Duration::from_millis(hold_ms));
            return self.mouse(up, pt, btn, 1, 0);
        }
        for n in 1..=i64::from(clicks.max(1)) {
            self.mouse(down, pt, btn, n, 0)?;
            self.mouse(up, pt, btn, n, 0)?;
            if n < i64::from(clicks) {
                std::thread::sleep(Duration::from_millis(60));
            }
        }
        Ok(())
    }

    fn drag(&mut self, from: Point, to: Point, duration_ms: u64) -> Result<(), DriverError> {
        Self::require_accessibility()?;
        let start = self.to_points(from);
        let end = self.to_points(to);
        self.mouse(K_CG_EVENT_MOUSE_MOVED, start, K_CG_MOUSE_BUTTON_LEFT, 0, 0)?;
        std::thread::sleep(Duration::from_millis(30));
        self.mouse(
            K_CG_EVENT_LEFT_MOUSE_DOWN,
            start,
            K_CG_MOUSE_BUTTON_LEFT,
            1,
            0,
        )?;
        let steps = 20u64;
        let pause = duration_ms / steps;
        for i in 1..=steps {
            let f = i as f64 / steps as f64;
            let pt = CGPoint {
                x: start.x + (end.x - start.x) * f,
                y: start.y + (end.y - start.y) * f,
            };
            self.mouse(
                K_CG_EVENT_LEFT_MOUSE_DRAGGED,
                pt,
                K_CG_MOUSE_BUTTON_LEFT,
                1,
                0,
            )?;
            std::thread::sleep(Duration::from_millis(pause.max(5)));
        }
        self.mouse(K_CG_EVENT_LEFT_MOUSE_UP, end, K_CG_MOUSE_BUTTON_LEFT, 1, 0)
    }

    fn scroll(&mut self, p: Point, dir: ScrollDir, amount: u32) -> Result<(), DriverError> {
        Self::require_accessibility()?;
        let pt = self.to_points(p);
        self.mouse(K_CG_EVENT_MOUSE_MOVED, pt, K_CG_MOUSE_BUTTON_LEFT, 0, 0)?;
        let n = i32::try_from(amount.max(1)).unwrap_or(i32::MAX);
        let (dy, dx) = match dir {
            ScrollDir::Up => (n, 0),
            ScrollDir::Down => (-n, 0),
            ScrollDir::Left => (0, n),
            ScrollDir::Right => (0, -n),
        };
        // SAFETY: valid arguments.
        let event = unsafe {
            CGEventCreateScrollWheelEvent2(
                std::ptr::null_mut(),
                K_CG_SCROLL_EVENT_UNIT_LINE,
                2,
                dy,
                dx,
                0,
            )
        };
        post(event)
    }

    fn type_text(&mut self, text: &str) -> Result<(), DriverError> {
        Self::require_accessibility()?;
        for (i, segment) in text.split('\n').enumerate() {
            if i > 0 {
                Self::key_event(36, true, 0)?;
                Self::key_event(36, false, 0)?;
                std::thread::sleep(Duration::from_millis(15));
            }
            // Chunk on char boundaries so a surrogate pair is never split
            // across two keyboard events.
            let chars: Vec<char> = segment.chars().collect();
            for chunk in chars.chunks(12) {
                let units: Vec<u16> = chunk.iter().collect::<String>().encode_utf16().collect();
                Self::type_unicode(&units)?;
                std::thread::sleep(Duration::from_millis(8));
            }
        }
        Ok(())
    }

    fn key(&mut self, combo: &KeyCombo) -> Result<(), DriverError> {
        Self::require_accessibility()?;
        let (code, needs_shift) = keycode(&combo.key)?;
        let flags = modifier_flags(combo, needs_shift);
        Self::key_event(code, true, flags)?;
        std::thread::sleep(Duration::from_millis(20));
        Self::key_event(code, false, flags)
    }

    fn ui_tree(&mut self) -> Result<Vec<UiNode>, DriverError> {
        Err(DriverError::Unsupported(
            "UI tree dumps are only available on Android and HarmonyOS; use computer_zoom on macOS"
                .to_string(),
        ))
    }

    fn app(&mut self, action: AppAction<'_>) -> Result<String, DriverError> {
        let open = std::path::PathBuf::from("/usr/bin/open");
        let osascript = std::path::PathBuf::from("/usr/bin/osascript");
        match action {
            AppAction::Launch(name) => {
                let args: Vec<&str> = if name.contains('.') && !name.ends_with(".app") {
                    vec!["-b", name]
                } else {
                    vec!["-a", name]
                };
                process::run_ok(&open, &args, Duration::from_secs(20))?;
                Ok(format!("launched {name}"))
            }
            AppAction::List => {
                let out = process::run_ok(
                    &osascript,
                    &[
                        "-e",
                        "tell application \"System Events\" to get name of every process whose background only is false",
                    ],
                    Duration::from_secs(20),
                )?;
                let names: Vec<String> = out
                    .stdout_text()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                Ok(format!(
                    "{} running apps:\n{}",
                    names.len(),
                    names.join("\n")
                ))
            }
            AppAction::Current => {
                let out = process::run_ok(
                    &osascript,
                    &[
                        "-e",
                        "tell application \"System Events\" to get name of first process whose frontmost is true",
                    ],
                    Duration::from_secs(20),
                )?;
                Ok(format!("frontmost app: {}", out.stdout_text().trim()))
            }
        }
    }

    fn devices(&mut self) -> Result<String, DriverError> {
        Ok("desktop target (no adb/hdc device selected); set [android] or [harmony] in ~/.codewhale/computer-use.toml to drive a phone".to_string())
    }

    fn element(&mut self) -> Option<&mut dyn crate::elements::ElementDriver> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::parse_combo;

    #[test]
    fn keycodes_and_shift_state() {
        assert_eq!(keycode(&Key::Char('a')).unwrap(), (0, false));
        assert_eq!(keycode(&Key::Char('A')).unwrap(), (0, true));
        assert_eq!(keycode(&Key::Char('!')).unwrap(), (18, true));
        assert_eq!(keycode(&Key::Named(NamedKey::Enter)).unwrap(), (36, false));
        assert_eq!(keycode(&Key::Named(NamedKey::F(5))).unwrap(), (96, false));
        assert!(keycode(&Key::Named(NamedKey::Back)).is_err());
        let combo = parse_combo("cmd+shift+t").unwrap();
        assert_eq!(modifier_flags(&combo, false), FLAG_COMMAND | FLAG_SHIFT);
    }

    #[test]
    fn point_conversion_uses_backing_scale() {
        let d = MacDriver {
            display: 1,
            logical_w: 1440.0,
            logical_h: 900.0,
            pixel_w: 2880,
            pixel_h: 1800,
            element_snapshot: None,
        };
        let pt = d.to_points(Point {
            x: 1440.0,
            y: 900.0,
        });
        assert_eq!((pt.x, pt.y), (720.0, 450.0));
        let pt = d.to_points(Point {
            x: 99_999.0,
            y: -5.0,
        });
        assert_eq!((pt.x, pt.y), (1439.0, 0.0));
    }
}
