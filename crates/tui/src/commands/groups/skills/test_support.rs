use std::ffi::OsString;

use tempfile::TempDir;

use crate::config::Config;
use crate::tui::app::{App, TuiOptions};

pub(crate) struct IsolatedHome {
    _lock: std::sync::MutexGuard<'static, ()>,
    home_prev: Option<OsString>,
    userprofile_prev: Option<OsString>,
    homedrive_prev: Option<OsString>,
    homepath_prev: Option<OsString>,
}

impl IsolatedHome {
    pub(crate) fn new(tmpdir: &TempDir) -> Self {
        let lock = crate::test_support::lock_test_env();
        let home = tmpdir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let home_prev = std::env::var_os("HOME");
        let userprofile_prev = std::env::var_os("USERPROFILE");
        let homedrive_prev = std::env::var_os("HOMEDRIVE");
        let homepath_prev = std::env::var_os("HOMEPATH");
        // SAFETY: tests that mutate process env hold the shared test env
        // mutex for the full lifetime of this guard.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("USERPROFILE", &home);
            std::env::set_var("HOMEDRIVE", home.parent().unwrap_or(&home));
            std::env::set_var("HOMEPATH", home.file_name().unwrap_or_default());
        }
        Self {
            _lock: lock,
            home_prev,
            userprofile_prev,
            homedrive_prev,
            homepath_prev,
        }
    }

    unsafe fn restore_var(key: &str, value: Option<OsString>) {
        if let Some(value) = value {
            unsafe { std::env::set_var(key, value) };
        } else {
            unsafe { std::env::remove_var(key) };
        }
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        // SAFETY: the shared test env mutex is still held while Drop runs.
        unsafe {
            Self::restore_var("HOME", self.home_prev.take());
            Self::restore_var("USERPROFILE", self.userprofile_prev.take());
            Self::restore_var("HOMEDRIVE", self.homedrive_prev.take());
            Self::restore_var("HOMEPATH", self.homepath_prev.take());
        }
    }
}

pub(crate) fn create_test_app_with_tmpdir(tmpdir: &TempDir) -> App {
    let options = TuiOptions {
        model: "deepseek-v4-pro".to_string(),
        workspace: tmpdir.path().to_path_buf(),
        config_path: None,
        config_profile: None,
        allow_shell: false,
        use_alt_screen: true,
        use_mouse_capture: false,
        use_bracketed_paste: true,
        max_subagents: 1,
        skills_dir: tmpdir.path().join("skills"),
        memory_path: tmpdir.path().join("memory.md"),
        notes_path: tmpdir.path().join("notes.txt"),
        mcp_config_path: tmpdir.path().join("mcp.json"),
        use_memory: false,
        start_in_agent_mode: false,
        skip_onboarding: true,
        yolo: false,
        resume_session_id: None,
        initial_input: None,
    };
    let mut app = App::new(options, &Config::default());
    app.skills_dir = tmpdir.path().join("skills");
    app
}

pub(crate) fn create_skill_dir(tmpdir: &TempDir, skill_name: &str, skill_content: &str) {
    let skill_dir = tmpdir.path().join("skills").join(skill_name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), skill_content).unwrap();
}
