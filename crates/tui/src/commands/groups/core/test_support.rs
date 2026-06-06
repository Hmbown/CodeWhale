use std::ffi::OsString;
use std::path::PathBuf;

use tempfile::TempDir;

use crate::config::Config;
use crate::tui::app::{App, TuiOptions};

pub(crate) struct SettingsPathGuard {
    _tmp: TempDir,
    previous: Option<OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl SettingsPathGuard {
    pub(crate) fn new() -> Self {
        let lock = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("settings tempdir");
        let config_path = tmp.path().join(".deepseek").join("config.toml");
        std::fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
        let previous = std::env::var_os("DEEPSEEK_CONFIG_PATH");
        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            std::env::set_var("DEEPSEEK_CONFIG_PATH", &config_path);
        }
        Self {
            _tmp: tmp,
            previous,
            _lock: lock,
        }
    }
}

impl Drop for SettingsPathGuard {
    fn drop(&mut self) {
        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("DEEPSEEK_CONFIG_PATH", previous);
            } else {
                std::env::remove_var("DEEPSEEK_CONFIG_PATH");
            }
        }
    }
}

pub(crate) fn create_test_app() -> App {
    let options = TuiOptions {
        model: "deepseek-v4-pro".to_string(),
        workspace: PathBuf::from("/tmp/test-workspace"),
        config_path: None,
        config_profile: None,
        allow_shell: false,
        use_alt_screen: true,
        use_mouse_capture: false,
        use_bracketed_paste: true,
        max_subagents: 1,
        skills_dir: PathBuf::from("/tmp/test-skills"),
        memory_path: PathBuf::from("memory.md"),
        notes_path: PathBuf::from("notes.txt"),
        mcp_config_path: PathBuf::from("mcp.json"),
        use_memory: false,
        start_in_agent_mode: false,
        skip_onboarding: true,
        yolo: false,
        resume_session_id: None,
        initial_input: None,
    };
    let mut app = App::new(options, &Config::default());
    app.ui_locale = crate::localization::Locale::En;
    app.api_provider = crate::config::ApiProvider::Deepseek;
    app.model = "deepseek-v4-pro".to_string();
    app.auto_model = false;
    app.model_ids_passthrough = false;
    app
}
