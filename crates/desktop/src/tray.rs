//! System tray setup.

use tauri::{
    App, Manager,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

pub fn setup(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    let icon = app
        .default_window_icon()
        .ok_or("Application window icon not found")?;
    let tray = TrayIconBuilder::new()
        .icon(icon.clone())
        .tooltip("CodeWhale Desktop")
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;

    // Store the tray icon in Tauri's managed state to keep it alive.
    // In Tauri v2, dropping the TrayIcon removes it from the system tray.
    app.manage(tray);

    Ok(())
}
