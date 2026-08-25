//! Windows driver: GDI capture of the primary monitor and `SendInput`.
//!
//! The process is switched to per-monitor DPI awareness so `GetSystemMetrics`
//! and the capture are in physical pixels, matching the coordinates the
//! model sees.

use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    SendInput, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1,
    VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT,
    VK_SHIFT, VK_SPACE, VK_TAB, VK_UP, VK_VOLUME_DOWN, VK_VOLUME_UP, VkKeyScanW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetSystemMetrics, GetWindow,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, PW_RENDERFULLCONTENT, SM_CXSCREEN, SM_CYSCREEN, SW_RESTORE,
    SetForegroundWindow, ShowWindow, WS_EX_TOOLWINDOW,
};

use crate::consent::AppIdentity;
use crate::driver::{
    AppAction, Button, Driver, DriverError, Point, RawFrame, ScrollDir, TargetInfo, TargetKind,
    UiNode,
};
use crate::elements::{
    ActionReceipt, AppInfo, AppState, ElementAction, ElementCaps, ElementDriver, StateMode,
    StateOpts, WindowInfo,
};
use crate::keys::{Key, KeyCombo, NamedKey};
use crate::process;

pub struct WindowsDriver {
    width: u32,
    height: u32,
}

fn send(inputs: &[INPUT]) -> Result<(), DriverError> {
    // SAFETY: `inputs` is a valid slice of fully initialised INPUT structs.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent as usize != inputs.len() {
        return Err(DriverError::Failed(format!(
            "SendInput delivered {sent} of {} events (is another window elevated?)",
            inputs.len()
        )));
    }
    Ok(())
}

fn mouse_input(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                // Wheel deltas are signed but the field is a DWORD.
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn key_input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

impl WindowsDriver {
    pub fn new() -> Result<Self, DriverError> {
        // SAFETY: documented process-wide setting; failure (already set) is harmless.
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let mut driver = Self {
            width: 0,
            height: 0,
        };
        driver.refresh_size();
        Ok(driver)
    }

    fn refresh_size(&mut self) {
        // SAFETY: no preconditions.
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        self.width = w.max(0) as u32;
        self.height = h.max(0) as u32;
    }

    fn absolute(&self, p: Point) -> (i32, i32) {
        let w = f64::from(self.width.max(1));
        let h = f64::from(self.height.max(1));
        let x = (p.x.clamp(0.0, w - 1.0) * 65535.0 / (w - 1.0).max(1.0)).round() as i32;
        let y = (p.y.clamp(0.0, h - 1.0) * 65535.0 / (h - 1.0).max(1.0)).round() as i32;
        (x, y)
    }

    fn move_abs(&self, p: Point) -> Result<(), DriverError> {
        let (x, y) = self.absolute(p);
        send(&[mouse_input(
            x,
            y,
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
        )])
    }

    fn button_flags(button: Button) -> (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS) {
        match button {
            Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            Button::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        }
    }

    fn press_vk(vk: u16, down: bool) -> INPUT {
        key_input(vk, 0, if down { 0 } else { KEYEVENTF_KEYUP })
    }
}

/// Returns `(virtual key, needs_shift)`.
fn vk_for(key: &Key) -> Result<(u16, bool), DriverError> {
    Ok(match key {
        Key::Named(named) => (
            match named {
                NamedKey::Enter => VK_RETURN,
                NamedKey::Tab => VK_TAB,
                NamedKey::Escape => VK_ESCAPE,
                NamedKey::Backspace => VK_BACK,
                NamedKey::Delete => VK_DELETE,
                NamedKey::Space => VK_SPACE,
                NamedKey::Up => VK_UP,
                NamedKey::Down => VK_DOWN,
                NamedKey::Left => VK_LEFT,
                NamedKey::Right => VK_RIGHT,
                NamedKey::Home => VK_HOME,
                NamedKey::End => VK_END,
                NamedKey::PageUp => VK_PRIOR,
                NamedKey::PageDown => VK_NEXT,
                NamedKey::Insert => VK_INSERT,
                NamedKey::CapsLock => VK_CAPITAL,
                NamedKey::F(n) => VK_F1 + u16::from(n.saturating_sub(1)),
                NamedKey::VolumeUp => VK_VOLUME_UP,
                NamedKey::VolumeDown => VK_VOLUME_DOWN,
                NamedKey::Back
                | NamedKey::AppHome
                | NamedKey::Recents
                | NamedKey::Power
                | NamedKey::Menu => {
                    return Err(DriverError::Unsupported(format!(
                        "{named:?} is a phone key; use win/ctrl/alt shortcuts on Windows"
                    )));
                }
            },
            false,
        ),
        Key::Char(c) => {
            let mut units = [0u16; 2];
            let encoded = c.encode_utf16(&mut units);
            if encoded.len() != 1 {
                return Err(DriverError::Failed(format!(
                    "`{c}` cannot be a shortcut key; use computer_type"
                )));
            }
            // SAFETY: plain layout lookup.
            let scan = unsafe { VkKeyScanW(encoded[0]) };
            if scan == -1 {
                return Err(DriverError::Failed(format!(
                    "`{c}` is not on the current keyboard layout; use computer_type"
                )));
            }
            let vk = (scan & 0xFF) as u16;
            let shift = (scan >> 8) & 1 == 1;
            (vk, shift)
        }
    })
}

impl Driver for WindowsDriver {
    fn info(&mut self) -> Result<TargetInfo, DriverError> {
        self.refresh_size();
        Ok(TargetInfo {
            kind: TargetKind::Desktop,
            driver: "windows-gdi-sendinput".into(),
            device_w: self.width,
            device_h: self.height,
            notes: vec![
                "primary monitor only; the process is per-monitor DPI aware so sizes are physical pixels".to_string(),
                "input to elevated (administrator) windows is blocked unless Codewhale itself is elevated".to_string(),
            ],
            supports_ui_tree: false,
            supports_apps: true,
        })
    }

    fn screenshot(&mut self) -> Result<RawFrame, DriverError> {
        self.refresh_size();
        let (w, h) = (self.width as i32, self.height as i32);
        if w <= 0 || h <= 0 {
            return Err(DriverError::Failed(
                "primary display reports zero size".to_string(),
            ));
        }
        // SAFETY: standard GDI capture sequence; every handle is released below.
        let pixels = unsafe {
            let screen = GetDC(std::ptr::null_mut());
            if screen.is_null() {
                return Err(DriverError::Failed("GetDC failed".to_string()));
            }
            let mem = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(mem, bitmap as _);
            let ok = BitBlt(mem, 0, 0, w, h, screen, 0, 0, SRCCOPY | CAPTUREBLT);
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [std::mem::zeroed()],
            };
            let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
            let lines = if ok != 0 {
                GetDIBits(
                    mem,
                    bitmap,
                    0,
                    h as u32,
                    buf.as_mut_ptr() as *mut _,
                    &mut info,
                    DIB_RGB_COLORS,
                )
            } else {
                0
            };
            SelectObject(mem, old);
            DeleteObject(bitmap as _);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            if ok == 0 || lines == 0 {
                return Err(DriverError::Failed(
                    "BitBlt/GetDIBits failed (secure desktop or locked screen?)".to_string(),
                ));
            }
            buf
        };
        let mut rgb = image::RgbImage::new(self.width, self.height);
        for (i, px) in rgb.pixels_mut().enumerate() {
            let b = pixels[i * 4];
            let g = pixels[i * 4 + 1];
            let r = pixels[i * 4 + 2];
            *px = image::Rgb([r, g, b]);
        }
        let png = crate::frame::encode_png(&rgb).map_err(DriverError::Failed)?;
        Ok(RawFrame { bytes: png })
    }

    fn move_to(&mut self, p: Point) -> Result<(), DriverError> {
        self.move_abs(p)
    }

    fn click(
        &mut self,
        p: Point,
        button: Button,
        clicks: u32,
        hold_ms: u64,
    ) -> Result<(), DriverError> {
        self.move_abs(p)?;
        std::thread::sleep(Duration::from_millis(20));
        let (down, up) = Self::button_flags(button);
        if hold_ms > 0 {
            send(&[mouse_input(0, 0, 0, down)])?;
            std::thread::sleep(Duration::from_millis(hold_ms));
            return send(&[mouse_input(0, 0, 0, up)]);
        }
        for i in 0..clicks.max(1) {
            send(&[mouse_input(0, 0, 0, down), mouse_input(0, 0, 0, up)])?;
            if i + 1 < clicks {
                std::thread::sleep(Duration::from_millis(60));
            }
        }
        Ok(())
    }

    fn drag(&mut self, from: Point, to: Point, duration_ms: u64) -> Result<(), DriverError> {
        self.move_abs(from)?;
        std::thread::sleep(Duration::from_millis(20));
        send(&[mouse_input(0, 0, 0, MOUSEEVENTF_LEFTDOWN)])?;
        let steps = 20u64;
        for i in 1..=steps {
            let f = i as f64 / steps as f64;
            self.move_abs(Point {
                x: from.x + (to.x - from.x) * f,
                y: from.y + (to.y - from.y) * f,
            })?;
            std::thread::sleep(Duration::from_millis((duration_ms / steps).max(5)));
        }
        send(&[mouse_input(0, 0, 0, MOUSEEVENTF_LEFTUP)])
    }

    fn scroll(&mut self, p: Point, dir: ScrollDir, amount: u32) -> Result<(), DriverError> {
        self.move_abs(p)?;
        let notches = i32::try_from(amount.max(1))
            .unwrap_or(i32::MAX)
            .saturating_mul(120);
        let (data, flags) = match dir {
            ScrollDir::Up => (notches, MOUSEEVENTF_WHEEL),
            ScrollDir::Down => (-notches, MOUSEEVENTF_WHEEL),
            ScrollDir::Right => (notches, MOUSEEVENTF_HWHEEL),
            ScrollDir::Left => (-notches, MOUSEEVENTF_HWHEEL),
        };
        send(&[mouse_input(0, 0, data, flags)])
    }

    fn type_text(&mut self, text: &str) -> Result<(), DriverError> {
        for ch in text.chars() {
            if ch == '\n' {
                send(&[
                    Self::press_vk(VK_RETURN, true),
                    Self::press_vk(VK_RETURN, false),
                ])?;
                continue;
            }
            let mut units = [0u16; 2];
            for unit in ch.encode_utf16(&mut units).iter() {
                send(&[
                    key_input(0, *unit, KEYEVENTF_UNICODE),
                    key_input(0, *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
                ])?;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        Ok(())
    }

    fn key(&mut self, combo: &KeyCombo) -> Result<(), DriverError> {
        let (vk, needs_shift) = vk_for(&combo.key)?;
        let mut mods: Vec<u16> = Vec::new();
        if combo.modifiers.ctrl {
            mods.push(VK_CONTROL);
        }
        if combo.modifiers.alt {
            mods.push(VK_MENU);
        }
        if combo.modifiers.shift || needs_shift {
            mods.push(VK_SHIFT);
        }
        if combo.modifiers.meta {
            mods.push(VK_LWIN);
        }
        let mut inputs: Vec<INPUT> = mods.iter().map(|m| Self::press_vk(*m, true)).collect();
        inputs.push(Self::press_vk(vk, true));
        inputs.push(Self::press_vk(vk, false));
        inputs.extend(mods.iter().rev().map(|m| Self::press_vk(*m, false)));
        send(&inputs)
    }

    fn ui_tree(&mut self) -> Result<Vec<UiNode>, DriverError> {
        Err(DriverError::Unsupported("UI tree dumps are only available on Android and HarmonyOS; use computer_zoom on Windows".to_string()))
    }

    fn app(&mut self, action: AppAction<'_>) -> Result<String, DriverError> {
        match action {
            AppAction::Launch(name) => {
                let cmd =
                    process::which("cmd").unwrap_or_else(|| std::path::PathBuf::from("cmd.exe"));
                process::run_ok(&cmd, &["/c", "start", "", name], Duration::from_secs(20))?;
                Ok(format!("started {name}"))
            }
            AppAction::List => {
                let ps = process::which("powershell")
                    .unwrap_or_else(|| std::path::PathBuf::from("powershell.exe"));
                let out = process::run_ok(
                    &ps,
                    &[
                        "-NoProfile",
                        "-Command",
                        "Get-Process | Where-Object { $_.MainWindowTitle } | ForEach-Object { $_.ProcessName + ' - ' + $_.MainWindowTitle }",
                    ],
                    Duration::from_secs(30),
                )?;
                Ok(format!("windowed processes:\n{}", out.stdout_text().trim()))
            }
            AppAction::Current => {
                let mut title = [0u16; 512];
                // SAFETY: buffer is valid for the stated length.
                let len = unsafe {
                    let hwnd = GetForegroundWindow();
                    if hwnd.is_null() {
                        0
                    } else {
                        GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32)
                    }
                };
                Ok(format!(
                    "foreground window: {}",
                    String::from_utf16_lossy(&title[..len.max(0) as usize])
                ))
            }
        }
    }

    fn devices(&mut self) -> Result<String, DriverError> {
        Ok("desktop target (no adb/hdc device selected); set [android] or [harmony] in %USERPROFILE%\\.codewhale\\computer-use.toml to drive a phone".to_string())
    }

    fn element(&mut self) -> Option<&mut dyn ElementDriver> {
        Some(self)
    }
}

// ---------------------------------------------------------------------------
// Element surface (phase 2): reads only.
//
// UI Automation would give Windows the same tree and background writes macOS
// has, but it is a COM surface and — as Codex documents for its own Windows
// build — cursor-free action is not generally reliable there. Until that
// lands, Windows contributes the app/window directory and window-scoped
// capture, and points the model at the phase-1 foreground tools for actions.
// ---------------------------------------------------------------------------

/// One top-level window as `EnumWindows` reports it.
struct WinRecord {
    hwnd: HWND,
    id: u32,
    pid: u32,
    title: String,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

/// HWNDs are documented as 32-bit-safe values even in 64-bit processes, so
/// the low half is a stable window id for the tool surface.
fn window_id(hwnd: HWND) -> u32 {
    hwnd as usize as u32
}

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: `buf` is valid for the length passed along.
    let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

/// Full path of the process owning a window, when it can be read.
fn process_path(pid: u32) -> Option<String> {
    // SAFETY: the handle is closed on every path out.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            buf.as_mut_ptr(),
            &mut len as *mut u32,
        );
        CloseHandle(handle);
        if ok == 0 || len == 0 {
            None
        } else {
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        }
    }
}

fn executable_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

/// `EnumWindows` callback: collects visible, titled, top-level windows.
unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> i32 {
    // SAFETY: `lparam` is the `&mut Vec<WinRecord>` handed to EnumWindows
    // below, which outlives the enumeration.
    let out = unsafe { &mut *(lparam as *mut Vec<WinRecord>) };
    // SAFETY: `hwnd` is a live window handle for the duration of the callback.
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return TRUE;
        }
        // Owned and tool windows are palettes and popups, not app windows.
        if !GetWindow(hwnd, GW_OWNER).is_null() {
            return TRUE;
        }
        if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
            return TRUE;
        }
        let title = window_title(hwnd);
        if title.trim().is_empty() {
            return TRUE;
        }
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(hwnd, &mut rect as *mut RECT) == 0 {
            return TRUE;
        }
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        if w <= 0 || h <= 0 {
            return TRUE;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid as *mut u32);
        if pid == 0 {
            return TRUE;
        }
        out.push(WinRecord {
            hwnd,
            id: window_id(hwnd),
            pid,
            title,
            x: rect.left,
            y: rect.top,
            w: w as u32,
            h: h as u32,
        });
        TRUE
    }
}

fn enumerate_windows() -> Vec<WinRecord> {
    let mut out: Vec<WinRecord> = Vec::new();
    // SAFETY: `collect_window` matches WNDENUMPROC and `out` outlives the
    // synchronous enumeration.
    unsafe {
        EnumWindows(Some(collect_window), (&raw mut out) as LPARAM);
    }
    out
}

impl WinRecord {
    fn info(&self) -> WindowInfo {
        WindowInfo {
            id: self.id,
            title: self.title.clone(),
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
        }
    }
}

/// Capture one window with `PrintWindow`, which asks the window to render
/// itself and so also works while it is partly covered.
fn capture_window(record: &WinRecord, max_edge: u32) -> Result<(Vec<u8>, u32, u32), DriverError> {
    let (w, h) = (record.w as i32, record.h as i32);
    if w <= 0 || h <= 0 {
        return Err(DriverError::Failed(
            "the window reports zero size".to_string(),
        ));
    }
    // SAFETY: standard GDI capture sequence; every handle is released below.
    let pixels = unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err(DriverError::Failed("GetDC failed".to_string()));
        }
        let mem = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bitmap as _);
        let printed = PrintWindow(record.hwnd, mem, PW_RENDERFULLCONTENT);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [std::mem::zeroed()],
        };
        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let lines = if printed != 0 {
            GetDIBits(
                mem,
                bitmap,
                0,
                h as u32,
                buf.as_mut_ptr() as *mut _,
                &mut info,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };
        SelectObject(mem, old);
        DeleteObject(bitmap as _);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        if printed == 0 || lines == 0 {
            return Err(DriverError::Failed(
                "PrintWindow/GetDIBits failed for this window (protected content or an elevated process?)".to_string(),
            ));
        }
        buf
    };
    let mut rgb = image::RgbImage::new(record.w, record.h);
    for (i, px) in rgb.pixels_mut().enumerate() {
        let b = pixels[i * 4];
        let g = pixels[i * 4 + 1];
        let r = pixels[i * 4 + 2];
        *px = image::Rgb([r, g, b]);
    }
    let (out_w, out_h) = crate::frame::fit(record.w, record.h, max_edge);
    let scaled = if (out_w, out_h) == (record.w, record.h) {
        rgb
    } else {
        image::imageops::resize(&rgb, out_w, out_h, image::imageops::FilterType::CatmullRom)
    };
    let png = crate::frame::encode_png(&scaled).map_err(DriverError::Failed)?;
    Ok((png, out_w, out_h))
}

const NO_ACTIONS: &str = "Windows element actions are not implemented: UI Automation gives the tree but not reliable cursor-free input. Use computer_raise to bring the window forward, then the foreground tools (computer_screenshot + computer_click/type/key) without the `app` argument.";

impl ElementDriver for WindowsDriver {
    fn apps(&mut self) -> Result<Vec<AppInfo>, DriverError> {
        let own_pid = std::process::id();
        let mut grouped: Vec<AppInfo> = Vec::new();
        for record in enumerate_windows() {
            if record.pid == own_pid {
                continue;
            }
            match grouped
                .iter_mut()
                .find(|app| app.identity.pid == record.pid)
            {
                Some(app) => app.windows.push(record.info()),
                None => {
                    let path = process_path(record.pid).unwrap_or_default();
                    let process_name = executable_name(&path);
                    let name = process_name
                        .strip_suffix(".exe")
                        .unwrap_or(&process_name)
                        .to_string();
                    grouped.push(AppInfo {
                        identity: AppIdentity {
                            pid: record.pid,
                            name,
                            // Windows has no reverse-DNS identity for a
                            // classic desktop app; the executable is it.
                            bundle_id: String::new(),
                            process_name,
                        },
                        windows: vec![record.info()],
                    });
                }
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
        if opts.mode == StateMode::Ax {
            return Err(DriverError::Unsupported(
                "there is no element tree on Windows yet; use mode=image (or computer_screenshot) here".to_string(),
            ));
        }
        let windows = enumerate_windows();
        let mut owned = windows.iter().filter(|r| r.pid == app.pid);
        let target = match opts.window_id {
            Some(id) => owned.find(|r| r.id == id).ok_or_else(|| {
                DriverError::Failed(format!(
                    "`{}` has no window {id}; computer_apps lists its window ids",
                    app.label()
                ))
            })?,
            None => owned.next().ok_or_else(|| {
                DriverError::Failed(format!(
                    "`{}` has no visible window (it may be minimized)",
                    app.label()
                ))
            })?,
        };
        let max_edge = if opts.max_edge == 0 {
            1024
        } else {
            opts.max_edge
        };
        let (png, image_w, image_h) = capture_window(target, max_edge)?;
        Ok(AppState {
            identity: app.clone(),
            window: target.info(),
            image_png: Some(png),
            image_w,
            image_h,
            // No tree yet: the caller sees this through `caps().tree == false`
            // and the note on `computer_info`.
            nodes: Vec::new(),
            omitted: 0,
            // PrintWindow renders the window itself, so cover does not change
            // what came back; we do not compute a z-order here.
            occluded: false,
        })
    }

    fn act(
        &mut self,
        _app: &AppIdentity,
        _action: ElementAction,
    ) -> Result<ActionReceipt, DriverError> {
        Err(DriverError::Unsupported(NO_ACTIONS.to_string()))
    }

    fn raise(&mut self, app: &AppIdentity) -> Result<(), DriverError> {
        let windows = enumerate_windows();
        let target = windows.iter().find(|r| r.pid == app.pid).ok_or_else(|| {
            DriverError::Failed(format!("`{}` has no visible window to raise", app.label()))
        })?;
        // SAFETY: `target.hwnd` came from this enumeration and is still live.
        let ok = unsafe {
            ShowWindow(target.hwnd, SW_RESTORE);
            SetForegroundWindow(target.hwnd)
        };
        if ok == 0 {
            return Err(DriverError::Failed(format!(
                "Windows refused to foreground `{}` (only the active app may steal focus)",
                app.label()
            )));
        }
        Ok(())
    }

    fn caps(&self) -> ElementCaps {
        ElementCaps {
            tree: false,
            window_image: true,
            background_actions: false,
            note: "Windows: window capture only; use foreground pixel tools for actions",
        }
    }
}
