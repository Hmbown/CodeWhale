//! Local media attachment commands.

use std::path::{Path, PathBuf};

use crate::commands::CommandResult;
use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "attach",
    aliases: &["image", "media", "fujian"],
    usage: "/attach <path>",
    description_id: MessageId::CmdAttachDescription,
};

pub(in crate::commands) struct AttachCmd;

impl RegisterCommand for AttachCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        attach(app, arg)
    }
}

fn attach(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some(raw_path) = arg.map(str::trim).filter(|value| !value.is_empty()) else {
        return CommandResult::error("Usage: /attach <image-or-video-path>");
    };

    let path = resolve_attachment_path(raw_path, &app.workspace);
    let Ok(path) = path.canonicalize() else {
        return CommandResult::error(format!("Attachment not found: {}", path.display()));
    };
    if !path.is_file() {
        return CommandResult::error(format!("Attachment is not a file: {}", path.display()));
    }

    let Some(kind) = media_kind(&path) else {
        return CommandResult::error(
            "Unsupported attachment type. /attach is for image/video paths; use @path for text files or directories.",
        );
    };

    app.insert_media_attachment(kind, &path, None);
    CommandResult::message(format!("Attached {kind}: {}", path.display()))
}

fn resolve_attachment_path(raw_path: &str, workspace: &Path) -> PathBuf {
    let unquoted = raw_path.trim().trim_matches('"').trim_matches('\'');
    let path = expand_home(unquoted);
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    } else if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

fn media_kind(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ppm" => Some("image"),
        "mp4" | "mov" | "m4v" | "webm" | "avi" | "mkv" => Some("video"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use tempfile::TempDir;

    fn app_with_workspace(tmpdir: &TempDir) -> App {
        App::new(
            TuiOptions {
                use_alt_screen: false,
                skills_dir: tmpdir.path().join("skills"),
                memory_path: tmpdir.path().join("memory.md"),
                notes_path: tmpdir.path().join("notes.txt"),
                mcp_config_path: tmpdir.path().join("mcp.json"),
                ..crate::test_support::test_tui_options(tmpdir.path())
            },
            &Config::default(),
        )
    }

    #[test]
    fn attach_inserts_image_reference() {
        let tmpdir = TempDir::new().expect("tempdir");
        let image_path = tmpdir.path().join("photo.png");
        std::fs::write(&image_path, b"not actually decoded").expect("write image fixture");
        let mut app = app_with_workspace(&tmpdir);

        let result = attach(&mut app, Some("photo.png"));

        assert!(result.message.expect("message").contains("Attached image"));
        assert!(app.input.contains("[Attached image:"));
        let canonical_path = image_path.canonicalize().expect("canonical image path");
        assert!(app.input.contains(&canonical_path.display().to_string()));
    }

    #[test]
    fn attach_rejects_unsupported_extension() {
        let tmpdir = TempDir::new().expect("tempdir");
        std::fs::write(tmpdir.path().join("notes.txt"), b"text").expect("write fixture");
        let mut app = app_with_workspace(&tmpdir);

        let result = attach(&mut app, Some("notes.txt"));

        assert!(
            result
                .message
                .expect("message")
                .contains("Unsupported attachment type")
        );
        assert!(app.input.is_empty());
    }
}
