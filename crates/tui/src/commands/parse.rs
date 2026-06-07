//! Slash command input parsing helpers.

pub(super) struct ParsedCommand<'a> {
    pub(super) name: String,
    pub(super) arg: Option<&'a str>,
}

pub(super) fn parse_slash_command(cmd: &str) -> ParsedCommand<'_> {
    let trimmed = cmd.trim();
    let mut parts = trimmed.splitn(2, ' ');
    let command = parts.next().unwrap_or_default().to_lowercase();
    let name = command.strip_prefix('/').unwrap_or(&command).to_string();
    let arg = parts.next().map(str::trim);

    ParsedCommand { name, arg }
}
