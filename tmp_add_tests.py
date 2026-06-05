import os

test_section = """

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    fn test_app() -> App {
        App::new(TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None, config_profile: None,
            allow_shell: false, use_alt_screen: true,
            use_mouse_capture: false, use_bracketed_paste: true,
            max_subagents: 1, skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false, start_in_agent_mode: false,
            skip_onboarding: true, yolo: false,
            resume_session_id: None, initial_input: None,
        }, &Config::default())
    }
}
"""

count = 0
for root, dirs, files in os.walk('crates/tui/src/commands/groups'):
    for f in files:
        if f.endswith('_command.rs'):
            path = os.path.join(root, f)
            with open(path, 'r', encoding='utf-8') as fh:
                content = fh.read()
            if '#[cfg(test)]' not in content:
                content += test_section
                with open(path, 'w', encoding='utf-8') as fh:
                    fh.write(content)
                count += 1
                print(f'Added tests to {path}')

print(f'\\nTotal: {count} files updated')
