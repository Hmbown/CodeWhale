//! Platform shell resolution for shell-command tools.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellPlatform {
    Windows,
    Unix,
}

impl ShellPlatform {
    #[must_use]
    pub(crate) fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellInvocation {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) raw_payload_on_windows: bool,
}

impl ShellInvocation {
    #[must_use]
    pub(crate) fn display_command(&self) -> Option<String> {
        if self.program == "sh" && self.args.len() == 2 && self.args[0] == "-c" {
            return Some(self.args[1].clone());
        }

        if shell_program_stem(&self.program)
            .is_some_and(|stem| matches!(stem.as_str(), "bash" | "zsh" | "fish" | "sh"))
            && self.args.len() == 2
            && self.args[0] == "-c"
        {
            return Some(self.args[1].clone());
        }

        if shell_program_stem(&self.program).is_some_and(|stem| stem == "cmd")
            && self.args.len() == 2
            && self.args[0].eq_ignore_ascii_case("/C")
        {
            let raw = &self.args[1];
            return Some(
                raw.strip_prefix("chcp 65001 >NUL & ")
                    .unwrap_or(raw)
                    .to_string(),
            );
        }

        if shell_program_stem(&self.program)
            .is_some_and(|stem| matches!(stem.as_str(), "pwsh" | "powershell"))
            && let Some((idx, _)) = self
                .args
                .iter()
                .enumerate()
                .find(|(_, arg)| arg.eq_ignore_ascii_case("-Command"))
            && let Some(command) = self.args.get(idx + 1)
        {
            return Some(command.clone());
        }

        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ShellProbe {
    pub(crate) shell: Option<String>,
    pub(crate) comspec: Option<String>,
    pub(crate) pwsh_on_path: bool,
}

impl ShellProbe {
    #[must_use]
    pub(crate) fn from_env() -> Self {
        Self {
            shell: std::env::var("SHELL")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            comspec: std::env::var("COMSPEC")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            pwsh_on_path: command_on_path("pwsh.exe") || command_on_path("pwsh"),
        }
    }
}

#[must_use]
pub(crate) fn shell_invocation(command: &str) -> ShellInvocation {
    shell_invocation_for_platform(command, ShellPlatform::current(), &ShellProbe::from_env())
}

#[must_use]
pub(crate) fn shell_invocation_for_platform(
    command: &str,
    platform: ShellPlatform,
    probe: &ShellProbe,
) -> ShellInvocation {
    match platform {
        ShellPlatform::Unix => unix_shell_invocation(command),
        ShellPlatform::Windows => windows_shell_invocation(command, probe),
    }
}

fn unix_shell_invocation(command: &str) -> ShellInvocation {
    ShellInvocation {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), command.to_string()],
        raw_payload_on_windows: false,
    }
}

fn windows_shell_invocation(command: &str, probe: &ShellProbe) -> ShellInvocation {
    if let Some(shell) = probe
        .shell
        .as_deref()
        .and_then(|shell| invocation_from_shell_env(shell, command))
    {
        return shell;
    }

    // Default Windows resolution is intentionally pwsh.exe -> cmd.exe. Windows
    // PowerShell 5.x can still be selected explicitly through SHELL, but it is
    // not used as an implicit fallback.
    if probe.pwsh_on_path {
        return powershell_invocation("pwsh.exe", command);
    }

    if let Some(comspec) = probe
        .comspec
        .as_deref()
        .filter(|value| shell_program_stem(value).is_some_and(|stem| stem == "cmd"))
    {
        return cmd_invocation(comspec, command);
    }

    cmd_invocation("cmd", command)
}

fn invocation_from_shell_env(shell: &str, command: &str) -> Option<ShellInvocation> {
    let stem = shell_program_stem(shell)?;
    match stem.as_str() {
        "pwsh" | "powershell" => Some(powershell_invocation(shell, command)),
        "cmd" => Some(cmd_invocation(shell, command)),
        "bash" | "zsh" | "fish" | "sh" => Some(posix_like_invocation(
            windows_posix_shell_program(shell, &stem),
            command,
        )),
        _ => None,
    }
}

fn cmd_invocation(program: &str, command: &str) -> ShellInvocation {
    ShellInvocation {
        program: program.to_string(),
        args: vec!["/C".to_string(), format!("chcp 65001 >NUL & {command}")],
        raw_payload_on_windows: true,
    }
}

fn powershell_invocation(program: &str, command: &str) -> ShellInvocation {
    ShellInvocation {
        program: program.to_string(),
        args: vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            command.to_string(),
        ],
        raw_payload_on_windows: false,
    }
}

fn posix_like_invocation(program: &str, command: &str) -> ShellInvocation {
    ShellInvocation {
        program: program.to_string(),
        args: vec!["-c".to_string(), command.to_string()],
        raw_payload_on_windows: false,
    }
}

fn windows_posix_shell_program<'a>(shell: &'a str, stem: &'a str) -> &'a str {
    if shell.trim_start().starts_with('/') {
        stem
    } else {
        shell
    }
}

pub(crate) fn shell_program_stem(program: &str) -> Option<String> {
    let normalized = program.trim().replace('\\', "/");
    let filename = normalized.rsplit('/').next()?.trim().to_ascii_lowercase();
    let stem = filename.strip_suffix(".exe").unwrap_or(&filename);
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_string())
    }
}

#[cfg(windows)]
fn command_on_path(program: &str) -> bool {
    use std::path::PathBuf;

    let candidate = PathBuf::from(program);
    if candidate.components().count() > 1 {
        return candidate.is_file();
    }

    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

#[cfg(not(windows))]
fn command_on_path(_program: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> ShellProbe {
        ShellProbe::default()
    }

    #[test]
    fn unix_shell_stays_sh_c() {
        let invocation = shell_invocation_for_platform("printf ok", ShellPlatform::Unix, &probe());
        assert_eq!(invocation.program, "sh");
        assert_eq!(invocation.args, ["-c", "printf ok"]);
        assert!(!invocation.raw_payload_on_windows);
        assert_eq!(invocation.display_command().as_deref(), Some("printf ok"));
    }

    #[test]
    fn windows_prefers_shell_env_powershell() {
        let invocation = shell_invocation_for_platform(
            r#"Remove-Item -Path "target file.txt" -Force"#,
            ShellPlatform::Windows,
            &ShellProbe {
                shell: Some(r"C:\Program Files\PowerShell\7\pwsh.exe".to_string()),
                ..probe()
            },
        );

        assert_eq!(
            invocation.program,
            r"C:\Program Files\PowerShell\7\pwsh.exe"
        );
        assert_eq!(
            invocation.args,
            [
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                r#"Remove-Item -Path "target file.txt" -Force"#
            ]
        );
        assert!(!invocation.raw_payload_on_windows);
        assert_eq!(
            invocation.display_command().as_deref(),
            Some(r#"Remove-Item -Path "target file.txt" -Force"#)
        );
    }

    #[test]
    fn windows_shell_env_can_select_windows_powershell() {
        let invocation = shell_invocation_for_platform(
            "Get-ChildItem",
            ShellPlatform::Windows,
            &ShellProbe {
                shell: Some(
                    r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".to_string(),
                ),
                comspec: Some(r"C:\Windows\System32\cmd.exe".to_string()),
                pwsh_on_path: false,
            },
        );

        assert_eq!(
            invocation.program,
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
        );
        assert_eq!(
            invocation.args,
            ["-NoProfile", "-NonInteractive", "-Command", "Get-ChildItem"]
        );
        assert!(!invocation.raw_payload_on_windows);
        assert_eq!(
            invocation.display_command().as_deref(),
            Some("Get-ChildItem")
        );
    }

    #[test]
    fn windows_uses_pwsh_before_cmd_when_available() {
        let invocation = shell_invocation_for_platform(
            "Get-ChildItem",
            ShellPlatform::Windows,
            &ShellProbe {
                comspec: Some(r"C:\Windows\System32\cmd.exe".to_string()),
                pwsh_on_path: true,
                ..probe()
            },
        );

        assert_eq!(invocation.program, "pwsh.exe");
        assert_eq!(
            invocation.args,
            ["-NoProfile", "-NonInteractive", "-Command", "Get-ChildItem"]
        );
        assert!(!invocation.raw_payload_on_windows);
    }

    #[test]
    fn windows_without_pwsh_falls_straight_to_cmd_not_windows_powershell() {
        let invocation = shell_invocation_for_platform(
            "git status --short",
            ShellPlatform::Windows,
            &ShellProbe {
                comspec: Some(
                    r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".to_string(),
                ),
                pwsh_on_path: false,
                ..probe()
            },
        );

        assert_eq!(invocation.program, "cmd");
        assert_eq!(
            invocation.args,
            ["/C", "chcp 65001 >NUL & git status --short"]
        );
        assert!(invocation.raw_payload_on_windows);
        assert_eq!(
            invocation.display_command().as_deref(),
            Some("git status --short")
        );
    }

    #[test]
    fn windows_falls_back_to_comspec_cmd_with_utf8_prefix() {
        let invocation = shell_invocation_for_platform(
            "git status --short",
            ShellPlatform::Windows,
            &ShellProbe {
                comspec: Some(r"C:\Windows\System32\cmd.exe".to_string()),
                pwsh_on_path: false,
                ..probe()
            },
        );

        assert_eq!(invocation.program, r"C:\Windows\System32\cmd.exe");
        assert_eq!(
            invocation.args,
            ["/C", "chcp 65001 >NUL & git status --short"]
        );
        assert!(invocation.raw_payload_on_windows);
        assert_eq!(
            invocation.display_command().as_deref(),
            Some("git status --short")
        );
    }

    #[test]
    fn windows_honors_posix_like_shell_env() {
        let invocation = shell_invocation_for_platform(
            "printf ok",
            ShellPlatform::Windows,
            &ShellProbe {
                shell: Some(r"C:\Program Files\Git\usr\bin\bash.exe".to_string()),
                pwsh_on_path: true,
                ..probe()
            },
        );

        assert_eq!(invocation.program, r"C:\Program Files\Git\usr\bin\bash.exe");
        assert_eq!(invocation.args, ["-c", "printf ok"]);
        assert!(!invocation.raw_payload_on_windows);
    }

    #[test]
    fn windows_posix_shell_env_with_unix_path_uses_stem() {
        let invocation = shell_invocation_for_platform(
            "printf ok",
            ShellPlatform::Windows,
            &ShellProbe {
                shell: Some("/usr/bin/bash".to_string()),
                ..probe()
            },
        );

        assert_eq!(invocation.program, "bash");
        assert_eq!(invocation.args, ["-c", "printf ok"]);
    }
}
