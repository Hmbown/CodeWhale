// DeepSeek IDE - Tauri Shell
// Responsibilities:
//   - Native window management
//   - Launch Python extension layer as sidecar
//   - IPC bridge between frontend and extension layer

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ipc;

use tauri::Manager;
use tauri_plugin_shell::ShellExt;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // Launch Python extension layer sidecar
            let shell = app.shell();
            let sidecar = shell
                .sidecar("core")
                .expect("core sidecar not found in tauri.conf.json");
            let (_rx, _child) = sidecar
                .spawn()
                .expect("Failed to launch Python extension layer");

            // Open devtools in debug builds
            #[cfg(debug_assertions)]
            {
                let window = app.get_webview_window("main").unwrap();
                window.open_devtools();
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::send_message,
            ipc::stream_message,
            ipc::get_status,
            ipc::get_capabilities,
            ipc::get_browser_state,
            ipc::get_browser_summary,
            ipc::start_managed_process,
            ipc::restart_managed_process,
            ipc::stop_managed_process,
            ipc::navigate_browser,
            ipc::reload_browser_console,
            ipc::update_browser_ui,
            ipc::evaluate_browser,
            ipc::screenshot_browser,
            ipc::inspect_browser_dom,
            ipc::operate_browser_element,
            ipc::run_mcp_browser_task,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri runtime error");
}

