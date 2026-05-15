pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            crate::ipc::send_message,
            crate::ipc::stream_message,
            crate::ipc::get_status,
            crate::ipc::get_capabilities,
            crate::ipc::get_browser_state,
            crate::ipc::get_browser_summary,
            crate::ipc::start_managed_process,
            crate::ipc::restart_managed_process,
            crate::ipc::stop_managed_process,
            crate::ipc::navigate_browser,
            crate::ipc::reload_browser_console,
            crate::ipc::update_browser_ui,
            crate::ipc::evaluate_browser,
            crate::ipc::screenshot_browser,
            crate::ipc::inspect_browser_dom,
            crate::ipc::operate_browser_element,
            crate::ipc::run_mcp_browser_task,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri runtime error");
}

