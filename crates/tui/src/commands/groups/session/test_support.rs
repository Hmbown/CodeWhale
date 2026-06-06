use tempfile::TempDir;

use crate::config::Config;
use crate::tui::app::{App, TuiOptions};

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
    App::new(options, &Config::default())
}
