//! macOS element backend: the accessibility tree, window-scoped capture, and
//! background input.
//!
//! Phase 1 (`macos.rs`) speaks pixels to the whole screen through the HID
//! event tap. This module implements [`ElementDriver`] on top of the same
//! [`MacDriver`], and speaks to **one app at a time** without moving the
//! user's cursor or raising a window:
//!
//! * perception — `CGWindowListCopyWindowInfo` for the app/window directory,
//!   `CGWindowListCreateImage` for a window image that works while the window
//!   is covered, and `AXUIElementCopyAttributeValue` to walk the element tree;
//! * action — `AXUIElementPerformAction` / `AXUIElementSetAttributeValue`
//!   first (nothing on screen moves, and the read-back is free verification),
//!   `CGEventPostToPid` as the fallback for apps that ignore synthetic AX
//!   presses (Electron/Chromium).
//!
//! Coordinates. Three spaces meet here:
//!
//! * the **window image** the model sees (pixels of the scaled PNG);
//! * **window-local points**, which the session produces by mapping image
//!   pixels through the window size — this is what `ElementAction` carries;
//! * **global display points**, which every posted `CGEvent` carries, even
//!   one delivered to a single process: `CGEventPostToPid` does not
//!   reinterpret the location per window, so the driver adds the window
//!   origin before posting. Verified live — posting a window-local location
//!   silently misses, landing wherever that point falls on the screen.
//!
//! The AX tree reports global points, so node frames are translated by the
//! window origin on the way in and back out on the way to an event.
//!
//! Permissions: Accessibility (AX reads/writes, event posting) and Screen
//! Recording (window images and window titles) must both be granted to the
//! process that runs Codewhale.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::Duration;

use image::{DynamicImage, RgbaImage, imageops::FilterType};

use crate::consent::AppIdentity;
use crate::driver::{Button, DriverError, ScrollDir};
use crate::drivers::macos::{MacDriver, cf_boolean_true, keycode, modifier_flags};
use crate::elements::{
    ActionReceipt, AppInfo, AppState, ElementAction, ElementCaps, ElementDriver, ElementNode,
    StateMode, StateOpts, WindowInfo,
};
use crate::keys::KeyCombo;
use crate::process;

// ---------------------------------------------------------------------------
// Foreign types and symbols
// ---------------------------------------------------------------------------

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFDataRef = *const c_void;
type AXUIElementRef = *mut c_void;
type CGImageRef = *mut c_void;
type CGEventRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CFRange {
    location: isize,
    length: isize,
}

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NUMBER_SINT64: i32 = 4;
const K_CF_NUMBER_FLOAT64: i32 = 6;

/// `kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements`:
/// real windows, front to back. Covered windows are still "on screen"; only
/// minimized and hidden ones drop out.
const K_CG_WINDOW_LIST_ON_SCREEN: u32 = (1 << 0) | (1 << 4);
const K_CG_WINDOW_LIST_INCLUDING_WINDOW: u32 = 1 << 3;
const K_CG_WINDOW_IMAGE_BOUNDS_IGNORE_FRAMING: u32 = 1 << 0;
const K_CG_NULL_WINDOW_ID: u32 = 0;

const K_AX_ERROR_SUCCESS: i32 = 0;
const K_AX_VALUE_CG_POINT: u32 = 1;
const K_AX_VALUE_CG_SIZE: u32 = 2;
const K_AX_VALUE_CF_RANGE: u32 = 4;

const K_CG_EVENT_MOUSE_MOVED: u32 = 5;
const K_CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const K_CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
const K_CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const K_CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
const K_CG_EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
const K_CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const K_CG_EVENT_OTHER_MOUSE_UP: u32 = 26;
const K_CG_MOUSE_BUTTON_LEFT: u32 = 0;
const K_CG_MOUSE_BUTTON_RIGHT: u32 = 1;
const K_CG_MOUSE_BUTTON_CENTER: u32 = 2;
const K_CG_MOUSE_EVENT_CLICK_STATE: u32 = 1;
/// `kCGMouseEventWindowUnderMousePointer` / `…ThatCanHandleThisEvent`.
/// `CGEventPostToPid` bypasses the window server's hit testing, so these
/// arrive as 0 and AppKit has no window to route the event to. Filling them
/// with the target's `CGWindowID` is what makes a posted click land.
const K_CG_MOUSE_EVENT_WINDOW_UNDER_POINTER: u32 = 91;
const K_CG_MOUSE_EVENT_WINDOW_UNDER_POINTER_HANDLER: u32 = 92;
const K_CG_SCROLL_EVENT_UNIT_LINE: u32 = 1;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        num_bytes: isize,
        encoding: u32,
        external_repr: bool,
    ) -> CFStringRef;
    fn CFStringGetLength(s: CFStringRef) -> isize;
    fn CFStringGetCString(s: CFStringRef, buffer: *mut u8, size: isize, encoding: u32) -> bool;
    fn CFArrayGetCount(array: CFArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, index: isize) -> CFTypeRef;
    fn CFDictionaryGetValue(dict: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;
    fn CFNumberGetValue(number: CFTypeRef, the_type: i32, value: *mut c_void) -> bool;
    fn CFBooleanGetValue(boolean: CFTypeRef) -> bool;
    fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
    fn CFDataGetLength(data: CFDataRef) -> isize;
    fn CFURLCreateFromFileSystemRepresentation(
        alloc: CFTypeRef,
        buffer: *const u8,
        len: isize,
        is_directory: bool,
    ) -> CFTypeRef;
    fn CFBundleCreate(alloc: CFTypeRef, url: CFTypeRef) -> CFTypeRef;
    fn CFBundleGetIdentifier(bundle: CFTypeRef) -> CFStringRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static CGRectNull: CGRect;
    static kCGWindowNumber: CFStringRef;
    static kCGWindowOwnerPID: CFStringRef;
    static kCGWindowOwnerName: CFStringRef;
    static kCGWindowName: CFStringRef;
    static kCGWindowBounds: CFStringRef;
    static kCGWindowLayer: CFStringRef;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
    fn CGWindowListCreateImage(
        screen_bounds: CGRect,
        list_option: u32,
        window_id: u32,
        image_option: u32,
    ) -> CGImageRef;
    fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect) -> bool;
    fn CGImageGetWidth(image: CGImageRef) -> usize;
    fn CGImageGetHeight(image: CGImageRef) -> usize;
    fn CGImageGetBytesPerRow(image: CGImageRef) -> usize;
    fn CGImageGetBitsPerPixel(image: CGImageRef) -> usize;
    fn CGImageGetDataProvider(image: CGImageRef) -> *mut c_void;
    fn CGImageRelease(image: CGImageRef);
    fn CGDataProviderCopyData(provider: *mut c_void) -> CFDataRef;
    fn CGEventCreateMouseEvent(
        source: *mut c_void,
        mouse_type: u32,
        point: CGPoint,
        button: u32,
    ) -> CGEventRef;
    fn CGEventCreateKeyboardEvent(source: *mut c_void, keycode: u16, keydown: bool) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent2(
        source: *mut c_void,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventSetLocation(event: CGEventRef, location: CGPoint);
    fn CGEventKeyboardSetUnicodeString(event: CGEventRef, length: usize, string: *const u16);
    fn CGEventPostToPid(pid: i32, event: CGEventRef);
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyElementAtPosition(
        application: AXUIElementRef,
        x: f32,
        y: f32,
        element: *mut AXUIElementRef,
    ) -> i32;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> i32;
    fn AXUIElementCopyActionNames(element: AXUIElementRef, names: *mut CFArrayRef) -> i32;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> i32;
    fn AXValueCreate(the_type: u32, value_ptr: *const c_void) -> CFTypeRef;
    fn AXValueGetValue(value: CFTypeRef, the_type: u32, value_ptr: *mut c_void) -> bool;
}

unsafe extern "C" {
    /// `libproc`, part of libSystem: the executable path of a live pid.
    fn proc_pidpath(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
}

// ---------------------------------------------------------------------------
// CoreFoundation helpers
// ---------------------------------------------------------------------------

/// An owned `CFStringRef`, released on drop.
struct CFStr(CFStringRef);

impl CFStr {
    fn new(text: &str) -> Self {
        // SAFETY: `text` is a valid UTF-8 slice; CF copies the bytes.
        let raw = unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                text.as_ptr(),
                text.len() as isize,
                K_CF_STRING_ENCODING_UTF8,
                false,
            )
        };
        Self(raw)
    }

    fn get(&self) -> CFStringRef {
        self.0
    }
}

impl Drop for CFStr {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own the one reference created in `new`.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// Read a `CFStringRef` into a Rust `String`.
fn cf_string(value: CFTypeRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: `value` is a live CF object; the type check guards the cast.
    unsafe {
        if CFGetTypeID(value) != CFStringGetTypeID() {
            return None;
        }
        let len = CFStringGetLength(value);
        // Worst case UTF-8 expansion for UTF-16 code units, plus NUL.
        let cap = (len as usize).saturating_mul(4).saturating_add(1);
        let mut buf = vec![0u8; cap.max(1)];
        if !CFStringGetCString(
            value,
            buf.as_mut_ptr(),
            buf.len() as isize,
            K_CF_STRING_ENCODING_UTF8,
        ) {
            return None;
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        buf.truncate(end);
        String::from_utf8(buf).ok()
    }
}

fn cf_i64(value: CFTypeRef) -> Option<i64> {
    if value.is_null() {
        return None;
    }
    // SAFETY: type-checked before reading as a number.
    unsafe {
        if CFGetTypeID(value) != CFNumberGetTypeID() {
            return None;
        }
        let mut out: i64 = 0;
        let ok = CFNumberGetValue(value, K_CF_NUMBER_SINT64, (&raw mut out).cast::<c_void>());
        ok.then_some(out)
    }
}

fn cf_f64(value: CFTypeRef) -> Option<f64> {
    if value.is_null() {
        return None;
    }
    // SAFETY: type-checked before reading as a number.
    unsafe {
        if CFGetTypeID(value) != CFNumberGetTypeID() {
            return None;
        }
        let mut out: f64 = 0.0;
        let ok = CFNumberGetValue(value, K_CF_NUMBER_FLOAT64, (&raw mut out).cast::<c_void>());
        ok.then_some(out)
    }
}

fn cf_bool(value: CFTypeRef) -> Option<bool> {
    if value.is_null() {
        return None;
    }
    // SAFETY: type-checked before reading as a boolean.
    unsafe {
        if CFGetTypeID(value) != CFBooleanGetTypeID() {
            return None;
        }
        Some(CFBooleanGetValue(value))
    }
}

fn cf_release(value: CFTypeRef) {
    if !value.is_null() {
        // SAFETY: caller owns the reference being dropped here.
        unsafe { CFRelease(value) };
    }
}

// ---------------------------------------------------------------------------
// Accessibility helpers
// ---------------------------------------------------------------------------

/// Copy one attribute. The returned reference is owned by the caller.
fn ax_copy(element: AXUIElementRef, attribute: &str) -> Option<CFTypeRef> {
    if element.is_null() {
        return None;
    }
    let key = CFStr::new(attribute);
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: `element` is live, `key` outlives the call, and `value` is a
    // valid out-pointer that AX only writes on success.
    let err = unsafe { AXUIElementCopyAttributeValue(element, key.get(), &raw mut value) };
    if err != K_AX_ERROR_SUCCESS || value.is_null() {
        None
    } else {
        Some(value)
    }
}

fn ax_string(element: AXUIElementRef, attribute: &str) -> Option<String> {
    let value = ax_copy(element, attribute)?;
    let out = cf_string(value);
    cf_release(value);
    out
}

fn ax_bool(element: AXUIElementRef, attribute: &str) -> Option<bool> {
    let value = ax_copy(element, attribute)?;
    let out = cf_bool(value);
    cf_release(value);
    out
}

fn ax_number(element: AXUIElementRef, attribute: &str) -> Option<f64> {
    let value = ax_copy(element, attribute)?;
    let out = cf_f64(value);
    cf_release(value);
    out
}

/// Read an `AXValue`-wrapped `CGPoint` or `CGSize` as `(a, b)`.
fn ax_pair(element: AXUIElementRef, attribute: &str, kind: u32) -> Option<(f64, f64)> {
    let value = ax_copy(element, attribute)?;
    let mut point = CGPoint::default();
    let mut size = CGSize::default();
    // SAFETY: the out-pointer matches `kind`; AXValueGetValue writes only on
    // a type match and reports it in the return value.
    let ok = unsafe {
        match kind {
            K_AX_VALUE_CG_POINT => AXValueGetValue(value, kind, (&raw mut point).cast::<c_void>()),
            _ => AXValueGetValue(value, kind, (&raw mut size).cast::<c_void>()),
        }
    };
    cf_release(value);
    if !ok {
        return None;
    }
    Some(match kind {
        K_AX_VALUE_CG_POINT => (point.x, point.y),
        _ => (size.width, size.height),
    })
}

/// The element's frame in global points, when it exposes one.
fn ax_frame(element: AXUIElementRef) -> Option<(f64, f64, f64, f64)> {
    let (x, y) = ax_pair(element, "AXPosition", K_AX_VALUE_CG_POINT)?;
    let (w, h) = ax_pair(element, "AXSize", K_AX_VALUE_CG_SIZE)?;
    Some((x, y, w, h))
}

/// Children as owned references (caller releases each).
fn ax_children(element: AXUIElementRef) -> Vec<AXUIElementRef> {
    let Some(array) = ax_copy(element, "AXChildren") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // SAFETY: `array` is a live CFArray of AXUIElementRefs; each value is
    // retained before it outlives the array.
    unsafe {
        let count = CFArrayGetCount(array);
        for index in 0..count {
            let child = CFArrayGetValueAtIndex(array, index);
            if !child.is_null() {
                out.push(CFRetain(child).cast_mut());
            }
        }
    }
    cf_release(array);
    out
}

fn ax_action_names(element: AXUIElementRef) -> Vec<String> {
    let mut array: CFArrayRef = std::ptr::null();
    // SAFETY: valid element and out-pointer; AX writes only on success.
    let err = unsafe { AXUIElementCopyActionNames(element, &raw mut array) };
    if err != K_AX_ERROR_SUCCESS || array.is_null() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: `array` is a live CFArray of CFStrings we now own.
    unsafe {
        let count = CFArrayGetCount(array);
        for index in 0..count {
            if let Some(name) = cf_string(CFArrayGetValueAtIndex(array, index)) {
                out.push(name);
            }
        }
    }
    cf_release(array);
    out
}

/// The deepest element at a global screen point, as the app itself reports
/// it. This is how a "click at these pixels" becomes a real, background,
/// verifiable action on a native control: `CGEventPostToPid` mouse events do
/// not drive AppKit's control machinery (measured — a posted click on a
/// Calculator key does nothing, while the same point through the HID tap
/// works), but the app will happily press the element we hit-test to.
fn ax_element_at(app: AXUIElementRef, point: (f64, f64)) -> Option<AXUIElementRef> {
    let mut element: AXUIElementRef = std::ptr::null_mut();
    // SAFETY: live application element; AX writes `element` only on success.
    let err = unsafe {
        AXUIElementCopyElementAtPosition(app, point.0 as f32, point.1 as f32, &raw mut element)
    };
    if err != K_AX_ERROR_SUCCESS || element.is_null() {
        None
    } else {
        Some(element)
    }
}

fn ax_perform(element: AXUIElementRef, action: &str) -> bool {
    let name = CFStr::new(action);
    // SAFETY: live element, live action name.
    unsafe { AXUIElementPerformAction(element, name.get()) == K_AX_ERROR_SUCCESS }
}

fn ax_set_string(element: AXUIElementRef, attribute: &str, value: &str) -> bool {
    let key = CFStr::new(attribute);
    let text = CFStr::new(value);
    // SAFETY: live element; AX copies the value it is handed.
    unsafe { AXUIElementSetAttributeValue(element, key.get(), text.get()) == K_AX_ERROR_SUCCESS }
}

fn ax_set_true(element: AXUIElementRef, attribute: &str) -> bool {
    let key = CFStr::new(attribute);
    // SAFETY: live element; `kCFBooleanTrue` is an immortal CF constant.
    unsafe {
        AXUIElementSetAttributeValue(element, key.get(), cf_boolean_true().cast_const())
            == K_AX_ERROR_SUCCESS
    }
}

/// Map an AX action name onto the vocabulary the model sees.
fn action_label(name: &str) -> Option<&'static str> {
    match name {
        "AXPress" => Some("press"),
        "AXShowMenu" => Some("menu"),
        "AXIncrement" => Some("increment"),
        "AXDecrement" => Some("decrement"),
        "AXConfirm" => Some("confirm"),
        "AXCancel" => Some("cancel"),
        "AXPick" => Some("pick"),
        "AXRaise" => Some("raise"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Window directory
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct WindowRecord {
    id: u32,
    pid: u32,
    owner: String,
    title: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl WindowRecord {
    fn covers(&self, other: &WindowRecord) -> bool {
        self.x <= other.x + 0.5
            && self.y <= other.y + 0.5
            && self.x + self.w >= other.x + other.w - 0.5
            && self.y + self.h >= other.y + other.h - 0.5
    }

    fn info(&self) -> WindowInfo {
        WindowInfo {
            id: self.id,
            title: self.title.clone(),
            x: self.x as i32,
            y: self.y as i32,
            w: self.w.max(0.0) as u32,
            h: self.h.max(0.0) as u32,
        }
    }
}

/// Every on-screen layer-0 window, front to back.
fn window_records() -> Result<Vec<WindowRecord>, DriverError> {
    // SAFETY: a plain CoreGraphics query; the returned array is released below.
    let array =
        unsafe { CGWindowListCopyWindowInfo(K_CG_WINDOW_LIST_ON_SCREEN, K_CG_NULL_WINDOW_ID) };
    if array.is_null() {
        return Err(DriverError::Permission(
            "CGWindowListCopyWindowInfo returned nothing: grant Screen Recording to the app running Codewhale (System Settings → Privacy & Security → Screen Recording) and restart it".to_string(),
        ));
    }
    let mut out = Vec::new();
    // SAFETY: `array` is a live CFArray of CFDictionaries owned by us.
    unsafe {
        let count = CFArrayGetCount(array);
        for index in 0..count {
            let dict = CFArrayGetValueAtIndex(array, index);
            if dict.is_null() {
                continue;
            }
            let layer = cf_i64(CFDictionaryGetValue(dict, kCGWindowLayer)).unwrap_or(-1);
            if layer != 0 {
                continue;
            }
            let (Some(id), Some(pid)) = (
                cf_i64(CFDictionaryGetValue(dict, kCGWindowNumber)),
                cf_i64(CFDictionaryGetValue(dict, kCGWindowOwnerPID)),
            ) else {
                continue;
            };
            let bounds = CFDictionaryGetValue(dict, kCGWindowBounds);
            let mut rect = CGRect::default();
            if bounds.is_null() || !CGRectMakeWithDictionaryRepresentation(bounds, &raw mut rect) {
                continue;
            }
            if rect.size.width < 1.0 || rect.size.height < 1.0 {
                continue;
            }
            out.push(WindowRecord {
                id: id as u32,
                pid: pid as u32,
                owner: cf_string(CFDictionaryGetValue(dict, kCGWindowOwnerName))
                    .unwrap_or_default(),
                title: cf_string(CFDictionaryGetValue(dict, kCGWindowName)).unwrap_or_default(),
                x: rect.origin.x,
                y: rect.origin.y,
                w: rect.size.width,
                h: rect.size.height,
            });
        }
    }
    cf_release(array);
    Ok(out)
}

/// Executable path of a live pid.
fn executable_path(pid: u32) -> Option<PathBuf> {
    let mut buf = vec![0u8; 4096];
    // SAFETY: `buf` is a writable buffer of the length we pass along.
    let len = unsafe { proc_pidpath(pid as i32, buf.as_mut_ptr().cast::<c_void>(), 4096) };
    if len <= 0 {
        return None;
    }
    buf.truncate(len as usize);
    String::from_utf8(buf).ok().map(PathBuf::from)
}

/// `/Applications/Notes.app/Contents/MacOS/Notes` → `/Applications/Notes.app`.
fn app_bundle_root(executable: &Path) -> Option<PathBuf> {
    let mut current = executable;
    while let Some(parent) = current.parent() {
        if parent
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
        {
            return Some(parent.to_path_buf());
        }
        current = parent;
    }
    None
}

/// Xcode leaves the build setting in the plist of unconfigured bundles; that
/// is not an identity.
fn sanitize_bundle_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains("$(") || trimmed.contains("${") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// `CFBundleIdentifier` out of an XML `Info.plist`. The binary plists that
/// ship with most system apps go through `CFBundleGetIdentifier` instead;
/// this is the readable-plist fallback.
fn parse_bundle_identifier(plist: &str) -> Option<String> {
    let key = plist.find("<key>CFBundleIdentifier</key>")?;
    let rest = &plist[key..];
    let open = rest.find("<string>")? + "<string>".len();
    let close = rest[open..].find("</string>")? + open;
    sanitize_bundle_id(&rest[open..close])
}

/// Ask CoreFoundation for a bundle's identifier.
fn cf_bundle_identifier(root: &Path) -> Option<String> {
    let bytes = root.as_os_str().as_encoded_bytes();
    // SAFETY: `bytes` is a valid filesystem representation of `root`; both CF
    // objects created here are released before returning.
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            true,
        );
        if url.is_null() {
            return None;
        }
        let bundle = CFBundleCreate(std::ptr::null(), url);
        cf_release(url);
        if bundle.is_null() {
            return None;
        }
        // `CFBundleGetIdentifier` returns a borrowed reference.
        let out = cf_string(CFBundleGetIdentifier(bundle)).and_then(|id| sanitize_bundle_id(&id));
        cf_release(bundle);
        out
    }
}

/// Bundle id for an executable inside a `.app`, or `None` for plain binaries
/// and unconfigured bundles.
fn bundle_id_from_executable_path(executable: &Path) -> Option<String> {
    let root = app_bundle_root(executable)?;
    if let Some(id) = cf_bundle_identifier(&root) {
        return Some(id);
    }
    let plist = std::fs::read_to_string(root.join("Contents").join("Info.plist")).ok()?;
    parse_bundle_identifier(&plist)
}

thread_local! {
    /// Bundle ids are stable per executable path and cost a bundle load to
    /// resolve; `apps()` runs on every app-targeted call.
    static BUNDLE_IDS: RefCell<HashMap<PathBuf, Option<String>>> =
        RefCell::new(HashMap::new());
}

fn cached_bundle_id(executable: &Path) -> Option<String> {
    BUNDLE_IDS.with(|cache| {
        cache
            .borrow_mut()
            .entry(executable.to_path_buf())
            .or_insert_with(|| bundle_id_from_executable_path(executable))
            .clone()
    })
}

fn identity_for(pid: u32, owner: &str) -> AppIdentity {
    let executable = executable_path(pid);
    let process_name = executable
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    AppIdentity {
        pid,
        name: owner.trim().to_string(),
        bundle_id: executable
            .as_deref()
            .and_then(cached_bundle_id)
            .unwrap_or_default(),
        process_name,
    }
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// One captured element, holding the live AX reference the action path needs.
struct SnapNode {
    element: AXUIElementRef,
    /// Frame in window-local points.
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    role: String,
    title: String,
    secure: bool,
    /// Distance from the window element, for the reported tree shape.
    depth: u8,
    /// Inside an `AXWebArea`: Chromium/Electron render content, where an
    /// accessibility write and a keystroke are not equivalent.
    in_web: bool,
}

/// The last `computer_app_state` capture of one window, with the AX
/// references retained so `#index` keeps meaning something afterwards.
pub struct MacElementSnapshot {
    pub pid: u32,
    pub window_id: u32,
    /// Window frame in screen points.
    pub window_x: f64,
    pub window_y: f64,
    pub window_w: f64,
    pub window_h: f64,
    app: AXUIElementRef,
    window: AXUIElementRef,
    nodes: Vec<SnapNode>,
}

impl MacElementSnapshot {
    fn node(&self, index: usize) -> Result<&SnapNode, DriverError> {
        self.nodes.get(index).ok_or_else(|| {
            DriverError::Failed(format!(
                "index {index} is out of range; the last app-state listed {} elements",
                self.nodes.len()
            ))
        })
    }

    /// Centre of the indexed element in window-local points.
    fn center(&self, index: usize) -> Result<(f64, f64), DriverError> {
        let node = self.node(index)?;
        Ok((node.x + node.w / 2.0, node.y + node.h / 2.0))
    }

    /// Window-local points → the global display space `CGEvent` uses.
    ///
    /// Posted events carry a **global** location even when they are delivered
    /// to one process: AppKit converts to window coordinates itself, using the
    /// window origin. Verified live — a window-local location silently misses
    /// (the click lands wherever that point falls on the screen).
    fn to_global(&self, local: (f64, f64)) -> (f64, f64) {
        (self.window_x + local.0, self.window_y + local.1)
    }

    /// Scroll-bar positions, used to tell "scrolled" from "end of content".
    ///
    /// Resolved fresh from the live window rather than from the snapshot:
    /// AppKit recreates its scrollers as content changes, so the references
    /// captured at snapshot time go stale exactly when a scroll succeeds —
    /// which would silently turn the movement signal off.
    fn scroll_positions(&self) -> Vec<f64> {
        let mut out = Vec::new();
        collect_scroll_values(self.window, 0, &mut out);
        out
    }
}

impl Drop for MacElementSnapshot {
    fn drop(&mut self) {
        for node in &self.nodes {
            cf_release(node.element.cast_const());
        }
        cf_release(self.window.cast_const());
        cf_release(self.app.cast_const());
    }
}

/// Depth-bounded search for scroll bars under `element`, reading each one's
/// position. Borrowed traversal: every child reference is released here.
fn collect_scroll_values(element: AXUIElementRef, depth: u8, out: &mut Vec<f64>) {
    const MAX_SCROLL_DEPTH: u8 = 10;
    const MAX_SCROLL_BARS: usize = 8;
    if element.is_null() || depth > MAX_SCROLL_DEPTH || out.len() >= MAX_SCROLL_BARS {
        return;
    }
    if ax_string(element, "AXRole").as_deref() == Some("AXScrollBar")
        && let Some(value) = ax_number(element, "AXValue")
    {
        out.push(value);
        return;
    }
    for child in ax_children(element) {
        collect_scroll_values(child, depth + 1, out);
        cf_release(child.cast_const());
    }
}

const MAX_DEPTH: u8 = 24;
const MAX_WALK_NODES: usize = 1500;

/// Depth-first walk of the AX tree. Takes ownership of `element`: it either
/// becomes a node or is released here.
fn walk(
    element: AXUIElementRef,
    depth: u8,
    origin: (f64, f64),
    in_web: bool,
    out: &mut Vec<SnapNode>,
    dropped: &mut usize,
) {
    if out.len() >= MAX_WALK_NODES {
        *dropped += 1;
        cf_release(element.cast_const());
        return;
    }
    let role = ax_string(element, "AXRole").unwrap_or_default();
    let frame = ax_frame(element).unwrap_or((origin.0, origin.1, 0.0, 0.0));
    let title = ax_string(element, "AXTitle")
        .filter(|t| !t.trim().is_empty())
        .or_else(|| ax_string(element, "AXValue").filter(|t| !t.trim().is_empty()))
        .or_else(|| ax_string(element, "AXDescription").filter(|t| !t.trim().is_empty()))
        .or_else(|| ax_string(element, "AXPlaceholderValue").filter(|t| !t.trim().is_empty()))
        .or_else(|| ax_string(element, "AXHelp").filter(|t| !t.trim().is_empty()))
        .unwrap_or_default();
    let in_web = in_web || role == "AXWebArea";
    out.push(SnapNode {
        element,
        x: frame.0 - origin.0,
        y: frame.1 - origin.1,
        w: frame.2,
        h: frame.3,
        secure: role == "AXSecureTextField",
        role,
        title,
        depth,
        in_web,
    });
    if depth >= MAX_DEPTH {
        return;
    }
    for child in ax_children(element) {
        walk(child, depth + 1, origin, in_web, out, dropped);
    }
}

/// Turn a snapshot node into the model-facing element, in image pixels.
fn to_element_node(
    index: usize,
    node: &SnapNode,
    scale_x: f64,
    scale_y: f64,
    image_w: u32,
    image_h: u32,
) -> ElementNode {
    let clamp = |value: f64, max: u32| -> u32 {
        if !value.is_finite() || value < 0.0 {
            0
        } else {
            (value.round() as u32).min(max)
        }
    };
    let x = clamp(node.x * scale_x, image_w);
    let y = clamp(node.y * scale_y, image_h);
    let mut actions: Vec<String> = ax_action_names(node.element)
        .iter()
        .filter_map(|name| action_label(name))
        .map(str::to_string)
        .collect();
    let editable = matches!(
        node.role.as_str(),
        "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSecureTextField"
    );
    if editable && !node.secure && !actions.iter().any(|a| a == "set_value") {
        actions.push("set_value".to_string());
    }
    ElementNode {
        index,
        role: node.role.clone(),
        title: node.title.clone(),
        x,
        y,
        w: clamp(node.w * scale_x, image_w.saturating_sub(x)),
        h: clamp(node.h * scale_y, image_h.saturating_sub(y)),
        actions,
        focused: ax_bool(node.element, "AXFocused").unwrap_or(false),
        enabled: ax_bool(node.element, "AXEnabled").unwrap_or(true),
        secure: node.secure,
        depth: node.depth,
    }
}

// ---------------------------------------------------------------------------
// Window image
// ---------------------------------------------------------------------------

/// BGRA (as CoreGraphics hands it over, with row padding) → RGBA.
fn bgra_to_rgba_image(data: &[u8], width: u32, height: u32, stride: usize) -> Option<RgbaImage> {
    if width == 0 || height == 0 {
        return None;
    }
    let row_bytes = (width as usize).checked_mul(4)?;
    if stride < row_bytes {
        return None;
    }
    let needed = stride
        .checked_mul(height as usize - 1)?
        .checked_add(row_bytes)?;
    if data.len() < needed {
        return None;
    }
    let mut out = Vec::with_capacity(row_bytes.checked_mul(height as usize)?);
    for row in 0..height as usize {
        let start = row * stride;
        for pixel in data[start..start + row_bytes].chunks_exact(4) {
            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    RgbaImage::from_raw(width, height, out)
}

/// Capture one window even when it is covered, and scale it to `max_edge`.
/// Returns `(png, width, height)` in the scaled image's pixels.
fn capture_window(window_id: u32, max_edge: u32) -> Result<(Vec<u8>, u32, u32), DriverError> {
    // SAFETY: `CGRectNull` asks for the window's own bounds; the image and
    // its pixel data are released below.
    let image = unsafe {
        CGWindowListCreateImage(
            CGRectNull,
            K_CG_WINDOW_LIST_INCLUDING_WINDOW,
            window_id,
            K_CG_WINDOW_IMAGE_BOUNDS_IGNORE_FRAMING,
        )
    };
    if image.is_null() {
        return Err(DriverError::Permission(
            "window capture returned nothing: grant Screen Recording to the app running Codewhale and restart it".to_string(),
        ));
    }
    // SAFETY: `image` is a live CGImage; the data provider copy is owned here.
    let captured = unsafe {
        let width = CGImageGetWidth(image) as u32;
        let height = CGImageGetHeight(image) as u32;
        let stride = CGImageGetBytesPerRow(image);
        let bpp = CGImageGetBitsPerPixel(image);
        let provider = CGImageGetDataProvider(image);
        let result = if bpp != 32 || provider.is_null() {
            None
        } else {
            let data = CGDataProviderCopyData(provider);
            if data.is_null() {
                None
            } else {
                let bytes = CFDataGetBytePtr(data);
                let len = CFDataGetLength(data);
                let slice = if bytes.is_null() || len <= 0 {
                    None
                } else {
                    Some(std::slice::from_raw_parts(bytes, len as usize))
                };
                let image =
                    slice.and_then(|slice| bgra_to_rgba_image(slice, width, height, stride));
                cf_release(data);
                image
            }
        };
        CGImageRelease(image);
        result
    };
    let rgba = captured.ok_or_else(|| {
        DriverError::Failed("the window image came back in an unexpected pixel format".to_string())
    })?;
    let (width, height) = (rgba.width(), rgba.height());
    let (out_w, out_h) = crate::frame::fit(width, height, max_edge);
    let image = DynamicImage::ImageRgba8(rgba);
    let scaled = if (out_w, out_h) == (width, height) {
        image
    } else {
        image.resize_exact(out_w, out_h, FilterType::CatmullRom)
    };
    // The model budget wants PNG without an alpha channel; window corners are
    // the only transparent pixels and they carry no information.
    let png = crate::frame::encode_png(&scaled.to_rgb8()).map_err(DriverError::Failed)?;
    Ok((png, out_w, out_h))
}

// ---------------------------------------------------------------------------
// Background input (never touches the cursor or the foreground)
// ---------------------------------------------------------------------------

/// Post an event to one process. Coordinates carried by the event are
/// **window-local points** of the target window, not screen points.
fn post_to_pid(pid: u32, event: CGEventRef) -> Result<(), DriverError> {
    if event.is_null() {
        return Err(DriverError::Failed(
            "CoreGraphics refused to create the event".to_string(),
        ));
    }
    // SAFETY: `event` is a valid CGEvent we own; post, then release.
    unsafe {
        CGEventPostToPid(pid as i32, event);
        CFRelease(event);
    }
    Ok(())
}

fn mouse_buttons(button: Button) -> (u32, u32, u32) {
    match button {
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
    }
}

fn mouse_to_pid(
    pid: u32,
    window_id: u32,
    kind: u32,
    point: (f64, f64),
    button: u32,
    click_state: i64,
) -> Result<(), DriverError> {
    let cg = CGPoint {
        x: point.0,
        y: point.1,
    };
    // SAFETY: valid arguments; ownership of the event moves into post_to_pid.
    let event = unsafe { CGEventCreateMouseEvent(std::ptr::null_mut(), kind, cg, button) };
    if event.is_null() {
        return Err(DriverError::Failed(
            "CGEventCreateMouseEvent returned null".to_string(),
        ));
    }
    // SAFETY: `event` is live and owned here.
    unsafe {
        if click_state > 0 {
            CGEventSetIntegerValueField(event, K_CG_MOUSE_EVENT_CLICK_STATE, click_state);
        }
        CGEventSetIntegerValueField(
            event,
            K_CG_MOUSE_EVENT_WINDOW_UNDER_POINTER,
            i64::from(window_id),
        );
        CGEventSetIntegerValueField(
            event,
            K_CG_MOUSE_EVENT_WINDOW_UNDER_POINTER_HANDLER,
            i64::from(window_id),
        );
    }
    post_to_pid(pid, event)
}

fn click_to_pid(
    pid: u32,
    window_id: u32,
    point: (f64, f64),
    button: Button,
    clicks: u32,
    hold_ms: u64,
) -> Result<(), DriverError> {
    let (down, up, code) = mouse_buttons(button);
    // A move first: controls that track the pointer (hover states, menus)
    // ignore a down that arrives with no pointer over them.
    mouse_to_pid(pid, window_id, K_CG_EVENT_MOUSE_MOVED, point, code, 0)?;
    if hold_ms > 0 {
        mouse_to_pid(pid, window_id, down, point, code, 1)?;
        std::thread::sleep(Duration::from_millis(hold_ms));
        return mouse_to_pid(pid, window_id, up, point, code, 1);
    }
    for n in 1..=i64::from(clicks.max(1)) {
        mouse_to_pid(pid, window_id, down, point, code, n)?;
        mouse_to_pid(pid, window_id, up, point, code, n)?;
        if n < i64::from(clicks) {
            std::thread::sleep(Duration::from_millis(60));
        }
    }
    Ok(())
}

fn key_to_pid(pid: u32, code: u16, down: bool, flags: u64) -> Result<(), DriverError> {
    // SAFETY: valid arguments; ownership moves into post_to_pid.
    let event = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), code, down) };
    if event.is_null() {
        return Err(DriverError::Failed(
            "CGEventCreateKeyboardEvent returned null".to_string(),
        ));
    }
    if flags != 0 {
        // SAFETY: `event` is live and owned here.
        unsafe { CGEventSetFlags(event, flags) };
    }
    post_to_pid(pid, event)
}

fn combo_to_pid(pid: u32, combo: &KeyCombo) -> Result<(), DriverError> {
    let (code, needs_shift) = keycode(&combo.key)?;
    let flags = modifier_flags(combo, needs_shift);
    key_to_pid(pid, code, true, flags)?;
    std::thread::sleep(Duration::from_millis(20));
    key_to_pid(pid, code, false, flags)
}

fn type_to_pid(pid: u32, text: &str) -> Result<(), DriverError> {
    for (line, segment) in text.split('\n').enumerate() {
        if line > 0 {
            key_to_pid(pid, 36, true, 0)?;
            key_to_pid(pid, 36, false, 0)?;
            std::thread::sleep(Duration::from_millis(15));
        }
        // Chunk on char boundaries so a surrogate pair is never split across
        // two keyboard events.
        let chars: Vec<char> = segment.chars().collect();
        for chunk in chars.chunks(12) {
            let units: Vec<u16> = chunk.iter().collect::<String>().encode_utf16().collect();
            for down in [true, false] {
                // SAFETY: valid arguments; the UTF-16 buffer outlives the call
                // and ownership of the event moves into post_to_pid.
                let event = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), 0, down) };
                if event.is_null() {
                    return Err(DriverError::Failed(
                        "CGEventCreateKeyboardEvent returned null".to_string(),
                    ));
                }
                // SAFETY: `event` is live and owned; `units` outlives the call.
                unsafe { CGEventKeyboardSetUnicodeString(event, units.len(), units.as_ptr()) };
                post_to_pid(pid, event)?;
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    }
    Ok(())
}

/// One "page" of wheel scrolling, in line units.
const LINES_PER_PAGE: i32 = 12;

fn scroll_to_pid(
    pid: u32,
    point: (f64, f64),
    dir: ScrollDir,
    pages: u32,
) -> Result<(), DriverError> {
    let lines = i32::try_from(pages.max(1))
        .unwrap_or(1)
        .saturating_mul(LINES_PER_PAGE);
    let (dy, dx) = match dir {
        ScrollDir::Up => (lines, 0),
        ScrollDir::Down => (-lines, 0),
        ScrollDir::Left => (0, lines),
        ScrollDir::Right => (0, -lines),
    };
    // SAFETY: valid arguments; ownership moves into post_to_pid.
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
    if event.is_null() {
        return Err(DriverError::Failed(
            "CGEventCreateScrollWheelEvent2 returned null".to_string(),
        ));
    }
    // The app routes the wheel by the event's location, in window-local points.
    // SAFETY: `event` is live and owned here.
    unsafe {
        CGEventSetLocation(
            event,
            CGPoint {
                x: point.0,
                y: point.1,
            },
        )
    };
    post_to_pid(pid, event)
}

fn drag_to_pid(
    pid: u32,
    window_id: u32,
    from: (f64, f64),
    to: (f64, f64),
    duration_ms: u64,
) -> Result<(), DriverError> {
    mouse_to_pid(
        pid,
        window_id,
        K_CG_EVENT_MOUSE_MOVED,
        from,
        K_CG_MOUSE_BUTTON_LEFT,
        0,
    )?;
    mouse_to_pid(
        pid,
        window_id,
        K_CG_EVENT_LEFT_MOUSE_DOWN,
        from,
        K_CG_MOUSE_BUTTON_LEFT,
        1,
    )?;
    let steps = 20u64;
    let pause = duration_ms / steps;
    for step in 1..=steps {
        let f = step as f64 / steps as f64;
        let point = (from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f);
        mouse_to_pid(
            pid,
            window_id,
            K_CG_EVENT_LEFT_MOUSE_DRAGGED,
            point,
            K_CG_MOUSE_BUTTON_LEFT,
            1,
        )?;
        std::thread::sleep(Duration::from_millis(pause.max(5)));
    }
    mouse_to_pid(
        pid,
        window_id,
        K_CG_EVENT_LEFT_MOUSE_UP,
        to,
        K_CG_MOUSE_BUTTON_LEFT,
        1,
    )
}

// ---------------------------------------------------------------------------
// Host terminal detection
// ---------------------------------------------------------------------------

/// Process names that are never the terminal app itself.
const TRANSPARENT_PARENTS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "tcsh",
    "csh",
    "login",
    "tmux",
    "tmux: server",
    "screen",
    "node",
    "deno",
    "bun",
    "python",
    "python3",
    "cargo",
    "rustc",
    "env",
    "script",
    "codewhale",
    "codew",
    "codewhale-computer-use",
    "claude",
    "codex",
];

fn parent_of(pid: u32) -> Option<(u32, String)> {
    let ps = PathBuf::from("/bin/ps");
    let pid_text = pid.to_string();
    let out = process::run(
        &ps,
        &["-o", "ppid=,comm=", "-p", &pid_text],
        Duration::from_secs(5),
    )
    .ok()?;
    if !out.success() {
        return None;
    }
    let line = out.stdout_text();
    let line = line.trim();
    let (ppid, comm) = line.split_once(char::is_whitespace)?;
    let ppid = ppid.trim().parse::<u32>().ok()?;
    Some((ppid, comm.trim().to_string()))
}

fn base_name(command: &str) -> String {
    Path::new(command)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| command.to_string())
}

/// The terminal app hosting Codewhale, as `(pid, name)`. Walks the parent
/// chain past shells and the agent's own processes; the consent model
/// hard-excludes whatever this finds so the agent can never drive the window
/// it is being watched in.
pub fn detect_host_terminal() -> Option<(u32, String)> {
    let mut pid = std::process::id();
    for _ in 0..8 {
        let (ppid, _own) = parent_of(pid)?;
        if ppid <= 1 {
            return None;
        }
        let (_, parent_comm) = parent_of(ppid)?;
        let name = base_name(&parent_comm);
        if !TRANSPARENT_PARENTS
            .iter()
            .any(|known| name.eq_ignore_ascii_case(known))
        {
            return Some((ppid, name));
        }
        pid = ppid;
    }
    None
}

// ---------------------------------------------------------------------------
// ElementDriver
// ---------------------------------------------------------------------------

impl MacDriver {
    /// Match the AX window for a `CGWindowList` record: by title first, then
    /// by position (titles are empty without Screen Recording, and duplicate
    /// across document windows).
    fn match_ax_window(app: AXUIElementRef, target: &WindowRecord) -> AXUIElementRef {
        let windows = ax_children_named(app, "AXWindows");
        let mut chosen: AXUIElementRef = std::ptr::null_mut();
        let mut by_position: AXUIElementRef = std::ptr::null_mut();
        for window in windows {
            let title_match = !target.title.trim().is_empty()
                && ax_string(window, "AXTitle").as_deref() == Some(target.title.as_str());
            let position_match = ax_pair(window, "AXPosition", K_AX_VALUE_CG_POINT)
                .is_some_and(|(x, y)| (x - target.x).abs() < 2.0 && (y - target.y).abs() < 2.0);
            if title_match && chosen.is_null() {
                chosen = window;
                continue;
            }
            if position_match && by_position.is_null() {
                by_position = window;
                continue;
            }
            cf_release(window.cast_const());
        }
        if chosen.is_null() {
            chosen = by_position;
        } else if !by_position.is_null() {
            cf_release(by_position.cast_const());
        }
        chosen
    }

    /// The snapshot backing index-based actions on `app`.
    fn snapshot_for(&self, app: &AppIdentity) -> Result<&MacElementSnapshot, DriverError> {
        match &self.element_snapshot {
            Some(snapshot) if snapshot.pid == app.pid => Ok(snapshot),
            _ => Err(DriverError::Failed(format!(
                "no window snapshot of `{}`; call computer_app_state on it first",
                app.label()
            ))),
        }
    }
}

/// Children of an array-valued attribute (`AXWindows`), owned by the caller.
fn ax_children_named(element: AXUIElementRef, attribute: &str) -> Vec<AXUIElementRef> {
    let Some(array) = ax_copy(element, attribute) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // SAFETY: `array` is a live CFArray of AXUIElementRefs owned by us; each
    // value is retained before it outlives the array.
    unsafe {
        let count = CFArrayGetCount(array);
        for index in 0..count {
            let child = CFArrayGetValueAtIndex(array, index);
            if !child.is_null() {
                out.push(CFRetain(child).cast_mut());
            }
        }
    }
    cf_release(array);
    out
}

impl ElementDriver for MacDriver {
    fn apps(&mut self) -> Result<Vec<AppInfo>, DriverError> {
        let records = window_records()?;
        let own_pid = std::process::id();
        let mut grouped: Vec<AppInfo> = Vec::new();
        for record in records {
            if record.pid == own_pid {
                continue;
            }
            match grouped
                .iter_mut()
                .find(|app| app.identity.pid == record.pid)
            {
                Some(app) => app.windows.push(record.info()),
                None => grouped.push(AppInfo {
                    identity: identity_for(record.pid, &record.owner),
                    windows: vec![record.info()],
                }),
            }
        }
        grouped.sort_by(|a, b| {
            a.identity
                .label()
                .to_lowercase()
                .cmp(&b.identity.label().to_lowercase())
        });
        Ok(grouped)
    }

    fn app_state(&mut self, app: &AppIdentity, opts: &StateOpts) -> Result<AppState, DriverError> {
        let records = window_records()?;
        let mut owned = records.iter().filter(|r| r.pid == app.pid);
        let target = match opts.window_id {
            Some(id) => owned.find(|r| r.id == id).cloned().ok_or_else(|| {
                DriverError::Failed(format!(
                    "`{}` has no window {id}; computer_apps lists its window ids",
                    app.label()
                ))
            })?,
            // The list is front to back, so the first match is the frontmost.
            None => owned.next().cloned().ok_or_else(|| {
                DriverError::Failed(format!(
                    "`{}` has no on-screen window (it may be minimized or hidden; computer_raise brings it back)",
                    app.label()
                ))
            })?,
        };
        // Fully covered by a single window in front of it?
        let occluded = records
            .iter()
            .take_while(|r| r.id != target.id)
            .any(|front| front.pid != target.pid && front.covers(&target));

        let want_image = opts.mode != StateMode::Ax;
        let (image_png, image_w, image_h) = if want_image {
            let max_edge = if opts.max_edge == 0 {
                1024
            } else {
                opts.max_edge
            };
            let (png, w, h) = capture_window(target.id, max_edge)?;
            (Some(png), w, h)
        } else {
            // No image: report node frames in the window's own points so the
            // session's image→window mapping stays the identity.
            (None, target.w.max(1.0) as u32, target.h.max(1.0) as u32)
        };

        let mut nodes = Vec::new();
        let mut dropped = 0usize;
        let mut ax_app: AXUIElementRef = std::ptr::null_mut();
        let mut ax_window: AXUIElementRef = std::ptr::null_mut();
        let want_tree = opts.mode != StateMode::Image;
        if want_tree {
            MacDriver::need_accessibility()?;
        }
        // Bind the AX window even for an image-only capture: the snapshot is
        // what later actions hit-test and measure scrolling against, so an
        // `image` state must not leave the app unaddressable. Without
        // Accessibility this stays null and only the image comes back.
        if MacDriver::accessibility_granted() {
            // SAFETY: creating an application element for a live pid.
            ax_app = unsafe { AXUIElementCreateApplication(app.pid as i32) };
        }
        if ax_app.is_null() {
            if want_tree {
                return Err(DriverError::Failed(format!(
                    "could not open an accessibility connection to `{}`",
                    app.label()
                )));
            }
        } else {
            // SAFETY: `ax_app` is live; the timeout keeps a wedged app from
            // hanging the tool call.
            unsafe { AXUIElementSetMessagingTimeout(ax_app, 2.0) };
            // Chromium and Electron publish only the window chrome until a
            // client asks for the web tree; without this an Electron app looks
            // like three traffic-light buttons. Apps that do not know the
            // attribute simply refuse the write. The tree is then built
            // asynchronously, so the first request for an app has to wait for
            // it — later ones find the attribute already set and do not.
            if ax_bool(ax_app, "AXManualAccessibility") != Some(true)
                && ax_set_true(ax_app, "AXManualAccessibility")
                && want_tree
            {
                std::thread::sleep(Duration::from_millis(400));
            }
            ax_window = MacDriver::match_ax_window(ax_app, &target);
            if ax_window.is_null() && want_tree {
                cf_release(ax_app.cast_const());
                return Err(DriverError::Failed(format!(
                    "`{}` did not expose window \"{}\" over accessibility; some apps (games, remote desktops) publish no tree — use computer_screenshot and pixel coordinates there",
                    app.label(),
                    target.title
                )));
            }
        }
        if want_tree && !ax_window.is_null() {
            // SAFETY: the snapshot keeps its own reference to the window
            // element beyond the walk that consumes the walked copy.
            let walked = unsafe { CFRetain(ax_window.cast_const()).cast_mut() };
            walk(
                walked,
                0,
                (target.x, target.y),
                false,
                &mut nodes,
                &mut dropped,
            );
        }

        let (scale_x, scale_y) = if target.w > 0.0 && target.h > 0.0 {
            (f64::from(image_w) / target.w, f64::from(image_h) / target.h)
        } else {
            (self.backing_scale(), self.backing_scale())
        };
        let reported: Vec<ElementNode> = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| to_element_node(index, node, scale_x, scale_y, image_w, image_h))
            .collect();

        self.element_snapshot = Some(MacElementSnapshot {
            pid: app.pid,
            window_id: target.id,
            window_x: target.x,
            window_y: target.y,
            window_w: target.w,
            window_h: target.h,
            app: ax_app,
            window: ax_window,
            nodes,
        });

        Ok(AppState {
            identity: app.clone(),
            window: target.info(),
            image_png,
            image_w,
            image_h,
            nodes: reported,
            omitted: dropped,
            occluded,
        })
    }

    fn act(
        &mut self,
        app: &AppIdentity,
        action: ElementAction,
    ) -> Result<ActionReceipt, DriverError> {
        MacDriver::need_accessibility()?;
        let snapshot = self.snapshot_for(app)?;
        let pid = snapshot.pid;
        let window_id = snapshot.window_id;
        match action {
            ElementAction::Press { index } => {
                let node = snapshot.node(index)?;
                let label = describe(node, index);
                if ax_perform(node.element, "AXPress") {
                    return Ok(ActionReceipt {
                        text: format!("pressed {label}"),
                        ..Default::default()
                    });
                }
                let point = snapshot.to_global(snapshot.center(index)?);
                click_to_pid(pid, window_id, point, Button::Left, 1, 0)?;
                Ok(ActionReceipt {
                    text: format!(
                        "AXPress was refused for {label}; clicked its centre in the background instead"
                    ),
                    ..Default::default()
                })
            }
            ElementAction::Click {
                x,
                y,
                button,
                clicks,
                hold_ms,
            } => {
                let global = snapshot.to_global((x, y));
                // A plain single left click is expressible as an element
                // press, which is the only thing native AppKit controls
                // reliably accept from the background.
                let simple = button == Button::Left && clicks <= 1 && hold_ms == 0;
                if simple && let Some(hit) = ax_element_at(snapshot.app, global) {
                    let role = ax_string(hit, "AXRole").unwrap_or_default();
                    let title = ax_string(hit, "AXTitle").unwrap_or_default();
                    let pressed = ax_perform(hit, "AXPress");
                    cf_release(hit.cast_const());
                    if pressed {
                        let named = if title.trim().is_empty() {
                            format!("[{role}]")
                        } else {
                            format!("[{role}] \"{}\"", truncate(&title))
                        };
                        return Ok(ActionReceipt {
                            text: format!(
                                "pressed the element at ({x:.0}, {y:.0}) in `{}`: {named}",
                                app.label()
                            ),
                            ..Default::default()
                        });
                    }
                }
                click_to_pid(pid, window_id, global, button, clicks, hold_ms)?;
                Ok(ActionReceipt {
                    text: format!(
                        "posted a click at ({x:.0}, {y:.0}) to `{}` in the background (no pressable element there; Chromium/Electron content accepts this, native AppKit controls often ignore it — computer_element on an indexed node is the reliable path)",
                        app.label()
                    ),
                    ..Default::default()
                })
            }
            ElementAction::SetValue { index, value } => {
                let node = snapshot.node(index)?;
                if node.secure {
                    return Err(DriverError::Failed(format!(
                        "{} is a secure text field; Codewhale never writes into password fields — ask the user to type it",
                        describe(node, index)
                    )));
                }
                let label = describe(node, index);
                let element = node.element;
                let in_web = node.in_web;
                if ax_set_string(element, "AXValue", &value) {
                    let mut text = format!("set {label} to \"{}\"", truncate(&value));
                    if in_web {
                        // Measured on Chromium: writing `AXValue` into a
                        // contenteditable changes the content but fires no DOM
                        // `input` event, so a framework-backed composer (Slack,
                        // Discord, Notion — anything React-shaped) keeps its old
                        // state while the text sits there looking correct. The
                        // read-back below is honest about the *content* and says
                        // nothing about the app's own state, so say so.
                        text.push_str(
                            "\nnote: this is web content. The text is in place, but a web app's own code may not have seen an input event — check that the app reacted (a send button enabling, a counter moving). If it did not, computer_raise the app and type instead.",
                        );
                    }
                    return Ok(ActionReceipt {
                        text,
                        verified: verify_value(snapshot, index, &value),
                        moved: None,
                    });
                }
                // Electron/Chromium ignore AXValue writes: focus the field,
                // select everything, and type over it.
                ax_set_true(element, "AXFocused");
                let point = snapshot.to_global(snapshot.center(index)?);
                click_to_pid(pid, window_id, point, Button::Left, 1, 0)?;
                std::thread::sleep(Duration::from_millis(40));
                combo_to_pid(
                    pid,
                    &crate::keys::parse_combo("cmd+a").map_err(DriverError::Failed)?,
                )?;
                type_to_pid(pid, &value)?;
                std::thread::sleep(Duration::from_millis(60));
                let verified = verify_value(snapshot, index, &value);
                Ok(ActionReceipt {
                    text: format!(
                        "AXValue was refused for {label}; focused it and typed \"{}\" instead",
                        truncate(&value)
                    ),
                    verified,
                    moved: None,
                })
            }
            ElementAction::Type { text } => {
                type_to_pid(pid, &text)?;
                Ok(ActionReceipt {
                    text: format!(
                        "typed {} characters into `{}` in the background",
                        text.chars().count(),
                        app.label()
                    ),
                    ..Default::default()
                })
            }
            ElementAction::Key { combo } => {
                combo_to_pid(pid, &combo)?;
                Ok(ActionReceipt {
                    text: format!("sent {combo} to `{}` in the background", app.label()),
                    ..Default::default()
                })
            }
            ElementAction::Menu { index } => {
                let node = snapshot.node(index)?;
                let label = describe(node, index);
                if ax_perform(node.element, "AXShowMenu") {
                    return Ok(ActionReceipt {
                        text: format!(
                            "performed AXShowMenu on {label}. A menu is its own window, so it will not appear in this app's window capture — read it with computer_screenshot, and note that a background app may decline to present one at all"
                        ),
                        ..Default::default()
                    });
                }
                let point = snapshot.to_global(snapshot.center(index)?);
                click_to_pid(pid, window_id, point, Button::Right, 1, 0)?;
                Ok(ActionReceipt {
                    text: format!(
                        "AXShowMenu was refused for {label}; right-clicked its centre instead"
                    ),
                    ..Default::default()
                })
            }
            ElementAction::Scroll {
                index,
                point,
                dir,
                pages,
            } => {
                let at = match (index, point) {
                    (Some(index), _) => snapshot.center(index)?,
                    (None, Some(point)) => point,
                    (None, None) => (snapshot.window_w / 2.0, snapshot.window_h / 2.0),
                };
                let before = snapshot.scroll_positions();
                // Wheel events reach Chromium/Electron content; AppKit scroll
                // views generally ignore a posted wheel, so when the scroll
                // bars say nothing moved we escalate to the page keys, which
                // do arrive (measured: posted key events work on a background
                // app, posted mouse events do not).
                scroll_to_pid(pid, snapshot.to_global(at), dir, pages)?;
                std::thread::sleep(Duration::from_millis(120));
                let mut how = "wheel";
                let mut after = snapshot.scroll_positions();
                if moved_between(&before, &after) == Some(false)
                    && let Some(code) = page_key(dir)
                {
                    for _ in 0..pages.clamp(1, MAX_PAGE_KEYS) {
                        key_to_pid(pid, code, true, 0)?;
                        key_to_pid(pid, code, false, 0)?;
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    std::thread::sleep(Duration::from_millis(120));
                    after = snapshot.scroll_positions();
                    how = "page keys";
                }
                Ok(ActionReceipt {
                    text: format!(
                        "scrolled {pages} page(s) {} in `{}` in the background (via {how})",
                        scroll_label(dir),
                        app.label()
                    ),
                    verified: None,
                    moved: moved_between(&before, &after),
                })
            }
            ElementAction::SelectText { index, start, end } => {
                let node = snapshot.node(index)?;
                let label = describe(node, index);
                let (start, end) = (start.min(end), start.max(end));
                let range = CFRange {
                    location: start as isize,
                    length: (end - start) as isize,
                };
                // SAFETY: the pointer matches kAXValueCFRangeType; the AXValue
                // is released after the write.
                let value = unsafe {
                    AXValueCreate(K_AX_VALUE_CF_RANGE, (&raw const range).cast::<c_void>())
                };
                if value.is_null() {
                    return Err(DriverError::Failed(
                        "could not build a text range for the selection".to_string(),
                    ));
                }
                let key = CFStr::new("AXSelectedTextRange");
                // SAFETY: live element, live attribute name and value.
                let err = unsafe { AXUIElementSetAttributeValue(node.element, key.get(), value) };
                cf_release(value);
                if err != K_AX_ERROR_SUCCESS {
                    return Err(DriverError::Failed(format!(
                        "{label} refused a text selection (it may not be a text element)"
                    )));
                }
                Ok(ActionReceipt {
                    text: format!("selected characters {start}..{end} in {label}"),
                    verified: Some(true),
                    moved: None,
                })
            }
            ElementAction::Drag {
                from,
                to,
                duration_ms,
            } => {
                drag_to_pid(
                    pid,
                    window_id,
                    snapshot.to_global(from),
                    snapshot.to_global(to),
                    duration_ms,
                )?;
                Ok(ActionReceipt {
                    text: format!(
                        "posted a drag ({:.0}, {:.0}) → ({:.0}, {:.0}) to `{}` in the background. Measured: native AppKit views ignore posted mouse events, so this usually does nothing outside Chromium/Electron content — to select text use action=select_text, and for a real drag use computer_raise then the foreground computer_drag",
                        from.0,
                        from.1,
                        to.0,
                        to.1,
                        app.label()
                    ),
                    ..Default::default()
                })
            }
        }
    }

    fn raise(&mut self, app: &AppIdentity) -> Result<(), DriverError> {
        let osascript = PathBuf::from("/usr/bin/osascript");
        let script = format!(
            "tell application \"System Events\" to set frontmost of (first process whose unix id is {}) to true",
            app.pid
        );
        process::run_ok(&osascript, &["-e", &script], Duration::from_secs(20))?;
        Ok(())
    }

    fn caps(&self) -> ElementCaps {
        ElementCaps {
            tree: true,
            window_image: true,
            background_actions: MacDriver::accessibility_granted(),
            note: "macOS: accessibility tree, window capture, and background actions — element presses, value writes, text selection, typing, keys and scrolling reach a background app; drags do not",
        }
    }
}

/// Page Up / Page Down virtual key codes; horizontal scrolling has no
/// keyboard equivalent, so those stay on the wheel.
fn page_key(dir: ScrollDir) -> Option<u16> {
    match dir {
        ScrollDir::Up => Some(116),
        ScrollDir::Down => Some(121),
        ScrollDir::Left | ScrollDir::Right => None,
    }
}

/// Never hold the keyboard for longer than a user would wait.
const MAX_PAGE_KEYS: u32 = 20;

fn scroll_label(dir: ScrollDir) -> &'static str {
    match dir {
        ScrollDir::Up => "up",
        ScrollDir::Down => "down",
        ScrollDir::Left => "left",
        ScrollDir::Right => "right",
    }
}

/// Compare two scroll-bar readings. `None` when the window exposes no scroll
/// bar to compare, so the caller reports "unknown" rather than guessing.
fn moved_between(before: &[f64], after: &[f64]) -> Option<bool> {
    if before.is_empty() || before.len() != after.len() {
        return None;
    }
    Some(
        before
            .iter()
            .zip(after)
            .any(|(a, b)| (a - b).abs() > 0.000_5),
    )
}

/// Read an element's value back after writing it.
///
/// Two things make the naive read wrong, and both were measured rather than
/// guessed. Chromium applies the write asynchronously, so an immediate read
/// still returns the old text; and it rebuilds the accessibility node when
/// the DOM value changes, so the reference captured at snapshot time goes
/// stale exactly on a successful write. Reading once, through the old
/// reference, reports `verified: false` for a write that landed — worse than
/// silence, because the model then retries or gives up.
///
/// So: re-resolve the element by hit-testing its centre, and give the app a
/// short window to settle. `None` means "could not read", never "failed".
fn verify_value(snapshot: &MacElementSnapshot, index: usize, expected: &str) -> Option<bool> {
    const ATTEMPTS: u32 = 6;
    const SETTLE: Duration = Duration::from_millis(50);
    let node = snapshot.nodes.get(index)?;
    let centre = snapshot.to_global((node.x + node.w / 2.0, node.y + node.h / 2.0));
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(SETTLE);
        }
        // The snapshot's own reference first: it is still valid for every app
        // that does not rebuild its tree, and costs one round trip.
        if let Some(read) = ax_string(node.element, "AXValue") {
            if read == expected {
                return Some(true);
            }
            last = Some(false);
        }
        if let Some(fresh) = ax_element_at(snapshot.app, centre) {
            let read = ax_string(fresh, "AXValue");
            cf_release(fresh.cast_const());
            if let Some(read) = read {
                if read == expected {
                    return Some(true);
                }
                last = Some(false);
            }
        }
    }
    last
}

fn describe(node: &SnapNode, index: usize) -> String {
    if node.title.trim().is_empty() {
        format!("#{index} [{}]", node.role)
    } else {
        format!("#{index} [{}] \"{}\"", node.role, truncate(&node.title))
    }
}

fn truncate(text: &str) -> String {
    let flat = text.replace('\n', "\\n");
    if flat.chars().count() <= 60 {
        flat
    } else {
        let head: String = flat.chars().take(57).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_local_points_become_global_display_points() {
        // `CGEventPostToPid` carries a global location even when it is aimed
        // at one process, so the window origin has to go back on.
        let snapshot = MacElementSnapshot {
            pid: 1,
            window_id: 2,
            window_x: 400.0,
            window_y: 300.0,
            window_w: 230.0,
            window_h: 408.0,
            app: std::ptr::null_mut(),
            window: std::ptr::null_mut(),
            nodes: Vec::new(),
        };
        assert_eq!(snapshot.to_global((88.0, 211.0)), (488.0, 511.0));
        assert_eq!(snapshot.to_global((0.0, 0.0)), (400.0, 300.0));
    }

    #[test]
    fn scroll_movement_is_unknown_rather_than_guessed() {
        // A real move.
        assert_eq!(moved_between(&[0.10], &[0.42]), Some(true));
        // End of content: the bar did not budge.
        assert_eq!(moved_between(&[0.99], &[0.99]), Some(false));
        // Sub-threshold jitter is not movement.
        assert_eq!(moved_between(&[0.5], &[0.5000001]), Some(false));
        // Nothing to compare: say so instead of claiming "did not move".
        assert_eq!(moved_between(&[], &[]), None);
        assert_eq!(moved_between(&[0.1], &[]), None);
        assert_eq!(moved_between(&[0.1], &[0.1, 0.2]), None);
    }

    #[test]
    fn only_vertical_scrolling_has_a_keyboard_fallback() {
        assert_eq!(page_key(ScrollDir::Up), Some(116));
        assert_eq!(page_key(ScrollDir::Down), Some(121));
        assert_eq!(page_key(ScrollDir::Left), None);
        assert_eq!(page_key(ScrollDir::Right), None);
        assert_eq!(scroll_label(ScrollDir::Down), "down");
    }

    #[test]
    fn parse_bundle_identifier_reads_the_xml_plist() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Notes</string>
    <key>CFBundleIdentifier</key>
    <string>com.apple.Notes</string>
</dict>
</plist>"#;
        assert_eq!(
            parse_bundle_identifier(plist).as_deref(),
            Some("com.apple.Notes")
        );
        // An unconfigured Xcode bundle has no identity yet.
        let placeholder =
            "<key>CFBundleIdentifier</key>\n<string>$(PRODUCT_BUNDLE_IDENTIFIER)</string>";
        assert_eq!(parse_bundle_identifier(placeholder), None);
        assert_eq!(parse_bundle_identifier("<dict></dict>"), None);
    }

    #[test]
    fn bgra_to_rgba_image_swizzles_and_honours_stride() {
        // 2x2 pixels, 12-byte stride (4 bytes of row padding).
        let mut data = Vec::new();
        for row in 0..2u8 {
            for col in 0..2u8 {
                // B, G, R, A
                data.extend_from_slice(&[10 + row, 20 + col, 30, 255]);
            }
            data.extend_from_slice(&[0xAA; 4]);
        }
        let image = bgra_to_rgba_image(&data, 2, 2, 12).expect("image");
        assert_eq!(image.dimensions(), (2, 2));
        assert_eq!(image.get_pixel(0, 0).0, [30, 20, 10, 255]);
        assert_eq!(image.get_pixel(1, 0).0, [30, 21, 10, 255]);
        assert_eq!(image.get_pixel(0, 1).0, [30, 20, 11, 255]);
        // Padding must not leak into the image.
        assert!(image.pixels().all(|p| p.0[0] == 30));
        // Short buffers and impossible strides are rejected, not read past.
        assert!(bgra_to_rgba_image(&data[..8], 2, 2, 12).is_none());
        assert!(bgra_to_rgba_image(&data, 2, 2, 4).is_none());
        assert!(bgra_to_rgba_image(&data, 0, 2, 12).is_none());
    }

    #[test]
    fn bundle_id_from_executable_path_reads_the_enclosing_app() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app = dir.path().join("Widget.app");
        let contents = app.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).expect("dirs");
        std::fs::write(
            contents.join("Info.plist"),
            "<plist><dict><key>CFBundleIdentifier</key><string>com.example.widget</string></dict></plist>",
        )
        .expect("plist");
        let exe = contents.join("MacOS").join("Widget");
        std::fs::write(&exe, b"#!/bin/sh\n").expect("exe");

        assert_eq!(app_bundle_root(&exe).as_deref(), Some(app.as_path()));
        assert_eq!(
            bundle_id_from_executable_path(&exe).as_deref(),
            Some("com.example.widget")
        );
        // A plain binary outside any bundle has no identity.
        let loose = dir.path().join("tool");
        std::fs::write(&loose, b"").expect("tool");
        assert_eq!(app_bundle_root(&loose), None);
        assert_eq!(bundle_id_from_executable_path(&loose), None);
    }

    #[test]
    fn window_records_detect_full_coverage() {
        let base = WindowRecord {
            id: 1,
            pid: 10,
            owner: "Notes".into(),
            title: "Notes".into(),
            x: 100.0,
            y: 100.0,
            w: 400.0,
            h: 300.0,
        };
        let covering = WindowRecord {
            x: 0.0,
            y: 0.0,
            w: 1440.0,
            h: 900.0,
            id: 2,
            ..base.clone()
        };
        let partial = WindowRecord {
            x: 200.0,
            y: 100.0,
            w: 400.0,
            h: 300.0,
            id: 3,
            ..base.clone()
        };
        assert!(covering.covers(&base));
        assert!(!partial.covers(&base));
        assert!(base.covers(&base), "identical frames count as covered");
    }

    #[test]
    fn action_labels_map_to_the_model_vocabulary() {
        assert_eq!(action_label("AXPress"), Some("press"));
        assert_eq!(action_label("AXShowMenu"), Some("menu"));
        assert_eq!(action_label("AXScrollToVisible"), None);
    }

    #[test]
    fn truncate_keeps_receipts_short_and_single_line() {
        assert_eq!(truncate("hello\nworld"), "hello\\nworld");
        let long = "x".repeat(200);
        let out = truncate(&long);
        assert_eq!(out.chars().count(), 58);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn base_name_strips_the_directory() {
        assert_eq!(
            base_name("/Applications/WezTerm.app/Contents/MacOS/wezterm-gui"),
            "wezterm-gui"
        );
        assert_eq!(base_name("Terminal"), "Terminal");
    }
}
