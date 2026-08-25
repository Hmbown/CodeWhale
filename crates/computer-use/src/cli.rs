//! Command line: `serve` (stdio MCP), plus operator diagnostics.

use std::path::PathBuf;

use crate::config::{Config, Mode, Target};

pub const USAGE: &str = "\
codewhale-computer-use — screenshots and input for Codewhale computer use

USAGE:
  codewhale-computer-use <COMMAND> [OPTIONS]

COMMANDS:
  setup                 Install the plugin bundle, request permissions, run a test capture
  serve                 Run the stdio MCP server (what the plugin bundle launches)
  doctor                Print target diagnostics and exit
  screenshot            Capture the screen to a PNG file (--out PATH, --grid)
  call <TOOL> [JSON]    Invoke one tool directly, e.g. call computer_click '{\"x\":10,\"y\":20}'
  tools                 Print the MCP tool catalog as JSON
  devices               List attached adb/hdc devices
  help, --help          Show this text
  --version             Show the version

OPTIONS (all commands):
  --config PATH         Config file (default ~/.codewhale/computer-use.toml)
  --target T            auto | desktop | android | harmony
  --mode M              act | observe
  --serial S            Android device serial (adb -s)
  --hdc-target K        HarmonyOS target key (hdc -t)
  --max-edge N          Longest screenshot edge in pixels (256..2048)
  --grid                Overlay a coordinate grid on screenshots
  --out PATH            (screenshot/call) where to write the returned image
  --force               (setup) overwrite a bundle directory that lacks Codewhale's install marker
  --no-permissions      (setup) report permission state without triggering OS prompts
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Setup,
    Serve,
    Doctor,
    Screenshot,
    Call { tool: String, args: String },
    Tools,
    Devices,
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub command: Command,
    pub config_path: Option<PathBuf>,
    pub target: Option<Target>,
    pub mode: Option<Mode>,
    pub serial: Option<String>,
    pub hdc_target: Option<String>,
    pub max_edge: Option<u32>,
    pub grid: bool,
    pub out: Option<PathBuf>,
    pub force: bool,
    pub no_permissions: bool,
}

pub fn parse(args: &[String]) -> Result<Cli, String> {
    let mut cli = Cli {
        command: Command::Help,
        config_path: None,
        target: None,
        mode: None,
        serial: None,
        hdc_target: None,
        max_edge: None,
        grid: false,
        out: None,
        force: false,
        no_permissions: false,
    };
    let mut positionals: Vec<String> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value_for = |name: &str| -> Result<String, String> {
            iter.next()
                .cloned()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match arg.as_str() {
            "--config" => cli.config_path = Some(PathBuf::from(value_for("--config")?)),
            "--target" => cli.target = Some(Target::parse(&value_for("--target")?)?),
            "--mode" => cli.mode = Some(Mode::parse(&value_for("--mode")?)?),
            "--serial" => cli.serial = Some(value_for("--serial")?),
            "--hdc-target" => cli.hdc_target = Some(value_for("--hdc-target")?),
            "--max-edge" => {
                let raw = value_for("--max-edge")?;
                cli.max_edge = Some(
                    raw.parse()
                        .map_err(|_| format!("--max-edge must be a number, got `{raw}`"))?,
                );
            }
            "--out" | "-o" => cli.out = Some(PathBuf::from(value_for("--out")?)),
            "--grid" => cli.grid = true,
            "--force" => cli.force = true,
            "--no-permissions" => cli.no_permissions = true,
            "--help" | "-h" | "help" => cli.command = Command::Help,
            "--version" | "-V" => cli.command = Command::Version,
            other if other.starts_with('-') => return Err(format!("unknown option `{other}`")),
            other => positionals.push(other.to_string()),
        }
    }
    if matches!(cli.command, Command::Help | Command::Version) && positionals.is_empty() {
        return Ok(cli);
    }
    let Some(first) = positionals.first() else {
        return Ok(cli);
    };
    cli.command = match first.as_str() {
        "setup" | "install" => Command::Setup,
        "serve" => Command::Serve,
        "doctor" | "info" => Command::Doctor,
        "screenshot" | "shot" => Command::Screenshot,
        "tools" => Command::Tools,
        "devices" => Command::Devices,
        "call" => {
            let tool = positionals
                .get(1)
                .cloned()
                .ok_or_else(|| "call requires a tool name".to_string())?;
            let args = positionals
                .get(2)
                .cloned()
                .unwrap_or_else(|| "{}".to_string());
            Command::Call { tool, args }
        }
        "help" => Command::Help,
        other => return Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    Ok(cli)
}

impl Cli {
    /// Build the effective config: file, then explicit flags.
    pub fn config(&self) -> Result<Config, String> {
        let mut cfg = Config::load(self.config_path.as_deref())?;
        if let Some(target) = self.target {
            cfg.target = target;
        }
        if let Some(mode) = self.mode {
            cfg.mode = mode;
        }
        if let Some(serial) = &self.serial {
            cfg.android.serial = serial.clone();
        }
        if let Some(key) = &self.hdc_target {
            cfg.harmony.target = key.clone();
        }
        if let Some(max_edge) = self.max_edge {
            cfg.max_edge = max_edge;
        }
        if self.grid {
            cfg.grid_default = true;
        }
        cfg.validate()?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_commands_and_options() {
        let cli = parse(&args(&[
            "serve", "--target", "android", "--serial", "abc", "--mode", "observe",
        ]))
        .unwrap();
        assert_eq!(cli.command, Command::Serve);
        assert_eq!(cli.target, Some(Target::Android));
        assert_eq!(cli.serial.as_deref(), Some("abc"));
        assert_eq!(cli.mode, Some(Mode::Observe));
        let cli = parse(&args(&[
            "call",
            "computer_click",
            "{\"x\":1}",
            "--out",
            "a.png",
        ]))
        .unwrap();
        assert_eq!(
            cli.command,
            Command::Call {
                tool: "computer_click".into(),
                args: "{\"x\":1}".into()
            }
        );
        assert_eq!(cli.out, Some(PathBuf::from("a.png")));
        let cli = parse(&args(&["setup", "--force", "--no-permissions"])).unwrap();
        assert_eq!(cli.command, Command::Setup);
        assert!(cli.force && cli.no_permissions);
        assert_eq!(parse(&args(&[])).unwrap().command, Command::Help);
        assert_eq!(
            parse(&args(&["--version"])).unwrap().command,
            Command::Version
        );
        assert!(parse(&args(&["bogus"])).is_err());
        assert!(parse(&args(&["serve", "--max-edge", "x"])).is_err());
        assert!(parse(&args(&["call"])).is_err());
    }

    #[test]
    fn flags_override_config() {
        let cli = parse(&args(&[
            "doctor",
            "--config",
            "/nonexistent/none.toml",
            "--max-edge",
            "800",
            "--grid",
        ]))
        .unwrap();
        let cfg = cli.config().unwrap();
        assert_eq!(cfg.max_edge, 800);
        assert!(cfg.grid_default);
    }
}
