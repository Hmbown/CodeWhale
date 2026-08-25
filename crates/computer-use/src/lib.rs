//! Computer-use MCP server for Codewhale.
//!
//! Screenshots and input on macOS, Windows, Linux (including HarmonyOS PC),
//! Android (`adb`), and HarmonyOS/OpenHarmony (`hdc`), spoken over stdio MCP
//! (`2024-11-05`) to the Codewhale plugin runtime. See
//! `docs/design/COMPUTER_USE_PLUGIN.md` and `docs/COMPUTER_USE.md`.

pub mod bundle;
pub mod cli;
pub mod config;
pub mod consent;
pub mod driver;
pub mod drivers;
pub mod elements;
pub mod frame;
pub mod keys;
pub mod mcp;
pub mod process;
pub mod session;
pub mod setup;

use std::io::Write;

use cli::Command;
use session::Session;

/// Entry point shared by the standalone binary and `codewhale computer-use`.
/// Returns the process exit code.
pub fn run(args: Vec<String>) -> i32 {
    match run_inner(&args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("codewhale-computer-use: {message}");
            2
        }
    }
}

fn run_inner(args: &[String]) -> Result<i32, String> {
    let cli = cli::parse(args)?;
    match &cli.command {
        Command::Help => {
            print!("{}", cli::USAGE);
            return Ok(0);
        }
        Command::Version => {
            println!("{} {}", mcp::SERVER_NAME, mcp::SERVER_VERSION);
            return Ok(0);
        }
        Command::Tools => {
            println!(
                "{}",
                serde_json::to_string_pretty(&session::tool_catalog()).map_err(|e| e.to_string())?
            );
            return Ok(0);
        }
        _ => {}
    }
    let cfg = cli.config()?;
    // The consent model hard-excludes the terminal that hosts Codewhale.
    #[cfg(target_os = "macos")]
    let cfg = {
        let mut cfg = cfg;
        if let Some((pid, name)) = crate::drivers::macos_ax::detect_host_terminal() {
            cfg.apps.host_terminal_pid = Some(pid);
            if cfg.apps.host_terminal.is_none() {
                cfg.apps.host_terminal = Some(name);
            }
        }
        cfg
    };
    if cli.command == Command::Setup {
        return Ok(setup::run(cfg, cli.force, cli.no_permissions));
    }
    let driver = match drivers::select_driver(&cfg) {
        Ok(driver) => driver,
        Err(e) if cli.command == Command::Serve => {
            // Keep serving so the model gets an actionable error from
            // computer_info instead of a dead MCP server.
            eprintln!("codewhale-computer-use: driver unavailable: {e}");
            Box::new(UnavailableDriver {
                message: e.to_string(),
            })
        }
        Err(e) => return Err(e.to_string()),
    };
    let mut session = Session::new(driver, cfg);
    match cli.command {
        Command::Serve => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            let code = mcp::McpServer::new(session).serve(stdin.lock(), stdout.lock());
            Ok(code)
        }
        Command::Doctor => {
            let info = session.call("computer_info", &serde_json::Value::Null);
            println!("{}", info.text);
            let devices = session.call("computer_devices", &serde_json::Value::Null);
            println!("{}", devices.text);
            println!("tools: {}", session::TOOL_NAMES.join(", "));
            Ok(if info.is_error { 1 } else { 0 })
        }
        Command::Devices => {
            let out = session.call("computer_devices", &serde_json::Value::Null);
            println!("{}", out.text);
            Ok(if out.is_error { 1 } else { 0 })
        }
        Command::Screenshot => {
            let out = session.call(
                "computer_screenshot",
                &serde_json::json!({ "grid": cli.grid }),
            );
            emit_outcome(
                &out,
                cli.out
                    .as_deref()
                    .or(Some(std::path::Path::new("screenshot.png"))),
            )
        }
        Command::Call { tool, args } => {
            let parsed: serde_json::Value = serde_json::from_str(&args)
                .map_err(|e| format!("tool arguments must be JSON: {e}"))?;
            let out = session.call(&tool, &parsed);
            emit_outcome(&out, cli.out.as_deref())
        }
        Command::Help | Command::Version | Command::Tools | Command::Setup => {
            unreachable!("handled above")
        }
    }
}

fn emit_outcome(
    out: &session::ToolOutcome,
    image_path: Option<&std::path::Path>,
) -> Result<i32, String> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{}", out.text).map_err(|e| e.to_string())?;
    if let Some(png) = &out.image_png {
        match image_path {
            Some(path) => {
                std::fs::write(path, png)
                    .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
                writeln!(stdout, "image: {} ({} bytes)", path.display(), png.len())
                    .map_err(|e| e.to_string())?;
            }
            None => writeln!(
                stdout,
                "image: {} bytes (pass --out PATH to save it)",
                png.len()
            )
            .map_err(|e| e.to_string())?,
        }
    }
    Ok(if out.is_error { 1 } else { 0 })
}

/// Placeholder driver used when no backend could start: every call returns
/// the startup error so the model (and operator) see the real cause.
struct UnavailableDriver {
    message: String,
}

impl UnavailableDriver {
    fn err<T>(&self) -> Result<T, driver::DriverError> {
        Err(driver::DriverError::Unavailable(self.message.clone()))
    }
}

impl driver::Driver for UnavailableDriver {
    fn info(&mut self) -> Result<driver::TargetInfo, driver::DriverError> {
        self.err()
    }
    fn screenshot(&mut self) -> Result<driver::RawFrame, driver::DriverError> {
        self.err()
    }
    fn move_to(&mut self, _p: driver::Point) -> Result<(), driver::DriverError> {
        self.err()
    }
    fn click(
        &mut self,
        _p: driver::Point,
        _b: driver::Button,
        _c: u32,
        _h: u64,
    ) -> Result<(), driver::DriverError> {
        self.err()
    }
    fn drag(
        &mut self,
        _f: driver::Point,
        _t: driver::Point,
        _d: u64,
    ) -> Result<(), driver::DriverError> {
        self.err()
    }
    fn scroll(
        &mut self,
        _p: driver::Point,
        _d: driver::ScrollDir,
        _a: u32,
    ) -> Result<(), driver::DriverError> {
        self.err()
    }
    fn type_text(&mut self, _t: &str) -> Result<(), driver::DriverError> {
        self.err()
    }
    fn key(&mut self, _c: &keys::KeyCombo) -> Result<(), driver::DriverError> {
        self.err()
    }
    fn ui_tree(&mut self) -> Result<Vec<driver::UiNode>, driver::DriverError> {
        self.err()
    }
    fn app(&mut self, _a: driver::AppAction<'_>) -> Result<String, driver::DriverError> {
        self.err()
    }
    fn devices(&mut self) -> Result<String, driver::DriverError> {
        self.err()
    }
}
