//! Host terminal window control (Windows): pin-to-top mini-window toggle.
//!
//! The TUI runs inside a terminal emulator, so its window is owned by the
//! OS. This module drives that host window directly through Win32: a window
//! handle is resolved via `GetConsoleWindow` (classic console hosts) or, when
//! that yields nothing — ConPTY hosts such as Windows Terminal and VS Code's
//! integrated terminal have no classic console window — by validating the
//! foreground window and then walking the parent process chain for the
//! nearest visible top-level window. `SetWindowPos` then toggles
//! always-on-top while shrinking/restoring the size: the "pin" action, like
//! a video player's PiP button. On non-Windows platforms every entry is a
//! no-op.
//!
//! The interaction entry is the right-click context menu (see
//! `crate::tui::mouse_ui::build_context_menu_entries`): a single pin item
//! toggles the host window between its normal state and a small
//! always-on-top window.
//!
//! Known limitation: with multiple host windows (several Windows Terminal or
//! VS Code windows) the ancestor-window fallback may resolve to a sibling
//! window rather than the one containing this tab — Win32 exposes no public
//! tab→window mapping. The foreground-window check mitigates this for the
//! common case (the user just right-clicked inside the host).

/// Default pixel size of the pinned (always-on-top) mini window.
/// The user can resize the terminal window while pinned; this is the default.
#[cfg(windows)]
const PINNED_W: i32 = 640;
#[cfg(windows)]
const PINNED_H: i32 = 400;

/// How many parent hops the fallback window walk may take before giving up
/// (guards against pathological process chains / loops).
#[cfg(windows)]
const MAX_ANCESTOR_HOPS: u32 = 8;

#[cfg(windows)]
mod imp {
    use super::*;
    use std::mem::size_of;
    use std::sync::Mutex;
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows::Win32::System::Console::GetConsoleWindow;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GetForegroundWindow, GetWindow, GetWindowRect,
        GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOPMOST, IsWindowVisible, IsZoomed,
        SW_MAXIMIZE, SW_RESTORE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow,
    };
    use windows_core::BOOL;

    /// Pin state: remembers the pre-pin window rect so unpinning restores it,
    /// plus whether the window was maximized (unpin restores maximized then,
    /// not the ordinary recorded rect).
    struct State {
        pinned: bool,
        saved_rect: Option<RECT>,
        was_maximized: bool,
    }

    impl State {
        const fn new() -> Self {
            Self {
                pinned: false,
                saved_rect: None,
                was_maximized: false,
            }
        }
    }

    static STATE: Mutex<State> = Mutex::new(State::new());

    /// The host window the user sees, if one can be resolved.
    ///
    /// Classic console hosts (conhost, legacy cmd windows) hand back a real
    /// window from `GetConsoleWindow`. ConPTY hosts (Windows Terminal, VS
    /// Code integrated terminal) have no classic console window, so first
    /// check the foreground window (the user just right-clicked inside the
    /// host, so it is almost certainly the host window) and then fall back
    /// to the parent process chain.
    fn console_hwnd() -> Option<HWND> {
        // ConPTY hosts (Windows Terminal sets WT_SESSION, VS Code sets
        // TERM_PROGRAM) have no meaningful console window: GetConsoleWindow
        // may return a hidden ConPTY window whose SetWindowPos visibly does
        // nothing. Skip it entirely and resolve the real host window.
        let conpty = std::env::var("WT_SESSION").is_ok() || std::env::var("TERM_PROGRAM").is_ok();
        if !conpty {
            let hwnd = unsafe { GetConsoleWindow() };
            if !hwnd.is_invalid() && unsafe { IsWindowVisible(hwnd) }.as_bool() {
                tracing::debug!("window_control: host window resolved via GetConsoleWindow");
                return Some(hwnd);
            }
        }
        tracing::debug!(
            conpty,
            "resolving host window (GetConsoleWindow skipped or unusable)"
        );
        if let Some(hwnd) = foreground_window_in_parent_chain(std::process::id()) {
            tracing::debug!("window_control: host window resolved via foreground check");
            return Some(hwnd);
        }
        if let Some(hwnd) = ancestor_top_level_window(std::process::id()) {
            tracing::debug!("window_control: host window resolved via ancestor walk");
            return Some(hwnd);
        }
        None
    }

    /// The foreground window, if it is visible and its process belongs to
    /// this process's parent chain (i.e. it is the host application's
    /// window). Visibility is required — a hidden foreground window cannot
    /// be the host the user is looking at.
    fn foreground_window_in_parent_chain(pid: u32) -> Option<HWND> {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_invalid() || !unsafe { IsWindowVisible(foreground) }.as_bool() {
            return None;
        }
        let mut fg_pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(foreground, Some(&mut fg_pid));
        }
        if fg_pid == 0 {
            return None;
        }
        let mut current = parent_process_id(pid);
        while let Some(p) = current {
            if p == fg_pid {
                return Some(foreground);
            }
            current = parent_process_id(p);
        }
        None
    }

    /// Nearest visible top-level window owned by the given process or any of
    /// its ancestors (parents first, then grandparents, …). Desktop-shell
    /// processes (explorer.exe owns the taskbar/desktop windows) are skipped
    /// — pinning those would be nonsensical.
    fn ancestor_top_level_window(pid: u32) -> Option<HWND> {
        let mut current = parent_process_id(pid);
        let mut hops = 0u32;
        while let Some(pid) = current {
            if hops >= MAX_ANCESTOR_HOPS {
                return None;
            }
            hops += 1;
            if let Some((_, name)) = process_entry(pid)
                && is_desktop_shell(&name)
            {
                current = parent_process_id(pid);
                continue;
            }
            if let Some(hwnd) = visible_top_level_window_for_pid(pid) {
                return Some(hwnd);
            }
            current = parent_process_id(pid);
        }
        None
    }

    /// Skip processes whose top-level windows are the desktop/taskbar or
    /// other shell chrome — never something to pin.
    fn is_desktop_shell(name: &str) -> bool {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "explorer.exe" | "dwm.exe" | "shell experience host.exe"
        )
    }

    /// Look up a process's parent PID and image name from a toolhelp
    /// snapshot. The snapshot handle is always closed.
    fn process_entry(pid: u32) -> Option<(u32, String)> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = None;
        let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
        while ok {
            if entry.th32ProcessID == pid {
                let name_len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..name_len]);
                found = Some((entry.th32ParentProcessID, name));
                break;
            }
            ok = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
        }
        unsafe {
            let _ = CloseHandle(snapshot);
        }
        found
    }

    fn parent_process_id(pid: u32) -> Option<u32> {
        process_entry(pid).map(|(ppid, _)| ppid)
    }

    /// First visible, unowned (true top-level) window owned by `pid`, if any.
    fn visible_top_level_window_for_pid(pid: u32) -> Option<HWND> {
        struct Ctx {
            target: u32,
            found: Option<HWND>,
        }
        let mut ctx = Ctx {
            target: pid,
            found: None,
        };
        unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let ctx = unsafe { &mut *(lparam.0 as *mut Ctx) };
            let mut wpid = 0u32;
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut wpid));
                if wpid == ctx.target && IsWindowVisible(hwnd).as_bool() {
                    // Skip owned popups/child windows; only true top-levels.
                    let owner = GetWindow(hwnd, GW_OWNER).unwrap_or_default();
                    if owner.0.is_null() {
                        ctx.found = Some(hwnd);
                        return BOOL(0); // stop enumeration
                    }
                }
            }
            BOOL(1)
        }
        unsafe {
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut Ctx as isize));
        }
        ctx.found
    }

    pub(super) fn toggle_pin() -> bool {
        let Some(hwnd) = console_hwnd() else {
            tracing::warn!(
                "window_control: no host window resolved (GetConsoleWindow null, no host window found)"
            );
            return false;
        };
        let mut state = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
        if state.pinned {
            // Unpin: drop always-on-top. When the window was maximized before
            // pinning, restore maximized — the recorded rect is the restored
            // ordinary size and would leave the window un-maximized. No
            // SWP_NOACTIVATE: the window should be (and stay) the foreground
            // window so its chrome (e.g. Windows Terminal's tabs) reacts to
            // clicks immediately.
            let flags = SWP_SHOWWINDOW;
            let restore_maximized = state.was_maximized;
            let saved = state.saved_rect.take();
            let result = if restore_maximized {
                unsafe {
                    SetWindowPos(
                        hwnd,
                        Some(HWND_NOTOPMOST),
                        0,
                        0,
                        0,
                        0,
                        flags | SWP_NOMOVE | SWP_NOSIZE,
                    )
                }
            } else if let Some(rect) = saved {
                unsafe {
                    SetWindowPos(
                        hwnd,
                        Some(HWND_NOTOPMOST),
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        flags,
                    )
                }
            } else {
                // No recorded rect (initial pin failed or window was moved):
                // just clear the always-on-top level, keep position/size.
                unsafe {
                    SetWindowPos(
                        hwnd,
                        Some(HWND_NOTOPMOST),
                        0,
                        0,
                        0,
                        0,
                        flags | SWP_NOMOVE | SWP_NOSIZE,
                    )
                }
            };
            if let Err(err) = result {
                // Keep `pinned` and put the saved rect back so the next click
                // retries instead of permanently losing the restore geometry.
                if !restore_maximized {
                    state.saved_rect = saved;
                }
                tracing::warn!(?err, "window_control: unpin SetWindowPos failed");
                return true;
            }
            if !restore_maximized && let Some(rect) = saved {
                // The size restore is also applied asynchronously: success of
                // SetWindowPos does not mean the window actually grew back.
                // Read the rect back and retry once (mirror of the pin path).
                std::thread::sleep(std::time::Duration::from_millis(60));
                let mut after = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                let applied = unsafe { GetWindowRect(hwnd, &mut after) }.is_ok()
                    && after.right - after.left == rect.right - rect.left
                    && after.bottom - after.top == rect.bottom - rect.top;
                tracing::info!(
                    applied,
                    w = after.right - after.left,
                    h = after.bottom - after.top,
                    "window_control: unpin result"
                );
                if !applied {
                    tracing::warn!("window_control: unpinned size did not stick; retrying once");
                    let _ = unsafe {
                        SetWindowPos(
                            hwnd,
                            Some(HWND_NOTOPMOST),
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                            flags,
                        )
                    };
                    std::thread::sleep(std::time::Duration::from_millis(60));
                }
            }
            if restore_maximized {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                }
            }
            state.pinned = false;
        } else {
            // Pin: a maximized window ignores SetWindowPos size/position
            // (only the z-order takes effect), so un-maximize first — this
            // is why the pin seemed to do nothing but always-on-top when the
            // host was maximized. Remember the pre-pin maximized state so
            // unpin can restore it instead of the ordinary recorded rect.
            // Then record the current rect, go always-on-top, and shrink to
            // the mini size.
            let was_maximized = unsafe { IsZoomed(hwnd) }.as_bool();
            state.was_maximized = was_maximized;
            if was_maximized {
                tracing::info!("window_control: host window maximized; restoring before pin");
                unsafe {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                // ShowWindow(SW_RESTORE) is delivered cross-process and
                // applied asynchronously: until WS_MAXIMIZE is actually
                // cleared, GetWindowRect still reports the maximized rect and
                // SetWindowPos discards the size. Poll for the restore to
                // land (bounded) instead of racing it.
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(800);
                while unsafe { IsZoomed(hwnd) }.as_bool() && std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(15));
                }
                if unsafe { IsZoomed(hwnd) }.as_bool() {
                    tracing::warn!("window_control: window still maximized after restore wait");
                }
            }
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            // The rect is only stored when GetWindowRect succeeds; a failure
            // means unpin falls back to "clear level only" instead of moving
            // the window to (0,0) with zero size.
            match unsafe { GetWindowRect(hwnd, &mut rect) } {
                Ok(()) => state.saved_rect = Some(rect),
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        "window_control: GetWindowRect failed; unpin will only clear always-on-top"
                    );
                }
            }
            // Keep the window at its current position while shrinking; only
            // when we have a recorded rect may SetWindowPos move it (it
            // won't move anywhere — the position is the recorded one).
            let (x, y, pos_flags) = match state.saved_rect {
                Some(r) => (r.left, r.top, SWP_SHOWWINDOW),
                None => (0, 0, SWP_SHOWWINDOW | SWP_NOMOVE),
            };
            if let Err(err) = unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    x,
                    y,
                    PINNED_W,
                    PINNED_H,
                    pos_flags,
                )
            } {
                // Pin failed: stay unpinned and drop the (now stale) saved
                // rect so a later unpin cannot act on geometry we never used.
                tracing::warn!(
                    ?err,
                    code = err.code().0,
                    "window_control: pin SetWindowPos failed"
                );
                state.saved_rect = None;
                return false;
            }
            // SetWindowPos is also applied asynchronously: success does not
            // mean the size stuck. Read the rect back and retry once.
            std::thread::sleep(std::time::Duration::from_millis(60));
            let mut after = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let applied = unsafe { GetWindowRect(hwnd, &mut after) }.is_ok()
                && after.right - after.left == PINNED_W
                && after.bottom - after.top == PINNED_H;
            tracing::info!(
                applied,
                w = after.right - after.left,
                h = after.bottom - after.top,
                "window_control: pin result"
            );
            if !applied {
                tracing::warn!("window_control: pinned size did not stick; retrying once");
                let _ = unsafe {
                    SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        x,
                        y,
                        PINNED_W,
                        PINNED_H,
                        pos_flags,
                    )
                };
                std::thread::sleep(std::time::Duration::from_millis(60));
            }
            state.pinned = true;
        }
        state.pinned
    }

    pub(super) fn pinned() -> bool {
        STATE.lock().map(|state| state.pinned).unwrap_or(false)
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn toggle_pin() -> bool {
        false
    }

    pub(super) fn pinned() -> bool {
        false
    }
}

/// Whether host-window control is available on this platform.
/// Only Windows consoles can be driven from inside the TUI.
pub(crate) fn available() -> bool {
    cfg!(windows)
}

/// Pin/unpin the host terminal window (normal window ↔ always-on-top mini
/// window). Returns the new pinned state.
pub(crate) fn toggle_pin() -> bool {
    imp::toggle_pin()
}

/// Whether the host window is currently the pinned (always-on-top mini)
/// state. The TUI reads this each frame to switch to the mini-window layout.
pub(crate) fn pinned() -> bool {
    imp::pinned()
}
