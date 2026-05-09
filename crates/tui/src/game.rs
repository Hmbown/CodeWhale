use std::path::{Path, PathBuf};

use anyhow::Result;
use deepseek_game::driver::{DriverResolver, LoadedDriver};
use deepseek_game::interaction::{build_playbook, format_playbook};
use deepseek_game::manifest::LoadedGame;
use deepseek_game::render::{RenderPanel, render_panels};
use deepseek_game::save::{LoadedSave, driver_lock, load_save};

#[derive(Debug, Clone)]
pub struct GameLaunchOptions {
    pub game_or_path: Option<PathBuf>,
    pub save: Option<String>,
    pub developer_mode: bool,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum GameSession {
    Loaded(LoadedGameSession),
    Notice(GameSessionNotice),
}

#[derive(Debug, Clone)]
pub struct LoadedGameSession {
    pub game_root: PathBuf,
    pub saves_root: PathBuf,
    pub driver_root: Option<PathBuf>,
    pub game_id: String,
    pub title: String,
    pub save_id: String,
    pub revision: u64,
    pub driver_id: String,
    pub driver_requirement: String,
    pub locked_driver_version: Option<String>,
    pub panels: Vec<RenderPanel>,
    pub skills: Vec<GameSkillCatalogEntry>,
    pub warnings: Vec<String>,
    pub developer_mode: bool,
}

#[derive(Debug, Clone)]
pub struct GameSkillCatalogEntry {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct GameSessionNotice {
    pub message: String,
    pub developer_mode: bool,
}

impl GameSession {
    pub fn developer_mode(&self) -> bool {
        match self {
            Self::Loaded(session) => session.developer_mode,
            Self::Notice(notice) => notice.developer_mode,
        }
    }

    pub fn set_developer_mode(&mut self, enabled: bool) {
        match self {
            Self::Loaded(session) => session.developer_mode = enabled,
            Self::Notice(notice) => notice.developer_mode = enabled,
        }
    }

    pub fn status_label(&self) -> String {
        match self {
            Self::Loaded(session) => {
                format!("Game Console: {} / {}", session.title, session.save_id)
            }
            Self::Notice(notice) => format!("Game Console: {}", notice.message),
        }
    }

    pub fn transcript_intro(&self) -> String {
        match self {
            Self::Loaded(session) => session.transcript_intro(),
            Self::Notice(notice) => {
                let mut lines = vec![
                    "Game Console".to_string(),
                    String::new(),
                    notice.message.clone(),
                ];
                if notice.developer_mode {
                    lines.push("Developer mode: on".to_string());
                }
                lines.join("\n")
            }
        }
    }

    pub fn status_report(&self) -> String {
        match self {
            Self::Loaded(session) => session.status_report(),
            Self::Notice(notice) => {
                let mut lines = vec![
                    "Game Console status".to_string(),
                    String::new(),
                    notice.message.clone(),
                ];
                lines.push(format!(
                    "Developer mode: {}",
                    if notice.developer_mode { "on" } else { "off" }
                ));
                lines.join("\n")
            }
        }
    }

    pub fn choices_report(&self) -> Result<String> {
        match self {
            Self::Loaded(session) => session.choices_report(),
            Self::Notice(notice) => Ok(format!(
                "No loaded Game Console session: {}",
                notice.message
            )),
        }
    }

    pub fn skill_directories(&self) -> Vec<PathBuf> {
        match self {
            Self::Loaded(session) => session.skill_directories(),
            Self::Notice(_) => Vec::new(),
        }
    }
}

impl LoadedGameSession {
    fn status_report(&self) -> String {
        let mut lines = vec![
            "Game Console status".to_string(),
            String::new(),
            format!("Game: {} ({})", self.title, self.game_id),
            format!("Save: {} @ revision {}", self.save_id, self.revision),
            format!(
                "Driver: {} {}",
                self.driver_id,
                self.locked_driver_version
                    .as_deref()
                    .unwrap_or(&self.driver_requirement)
            ),
            format!(
                "Developer mode: {}",
                if self.developer_mode { "on" } else { "off" }
            ),
        ];

        if self.developer_mode {
            lines.push(format!("Game root: {}", self.game_root.display()));
            lines.push(format!("Saves root: {}", self.saves_root.display()));
            if let Some(driver_root) = &self.driver_root {
                lines.push(format!("Driver root: {}", driver_root.display()));
            }
        }
        if !self.warnings.is_empty() {
            lines.push(String::new());
            lines.push("Warnings:".to_string());
            lines.extend(self.warnings.iter().map(|warning| format!("- {warning}")));
        }
        if !self.skills.is_empty() {
            lines.push(String::new());
            lines.push(format!("Loadable game skills: {}", self.skills.len()));
        }
        lines.join("\n")
    }

    fn transcript_intro(&self) -> String {
        let mut lines = vec![
            "Game Console".to_string(),
            String::new(),
            format!("{} ({})", self.title, self.game_id),
            format!("Save: {} @ revision {}", self.save_id, self.revision),
            format!(
                "Driver: {} {}",
                self.driver_id,
                self.locked_driver_version
                    .as_deref()
                    .unwrap_or(&self.driver_requirement)
            ),
        ];

        if self.developer_mode {
            lines.push(format!("Game root: {}", self.game_root.display()));
            lines.push(format!("Saves root: {}", self.saves_root.display()));
            if let Some(driver_root) = &self.driver_root {
                lines.push(format!("Driver root: {}", driver_root.display()));
            }
        }
        if !self.warnings.is_empty() {
            lines.push(String::new());
            lines.push("Warnings:".to_string());
            lines.extend(self.warnings.iter().map(|warning| format!("- {warning}")));
        }
        if !self.skills.is_empty() {
            lines.push(String::new());
            lines.push("Loadable game skills:".to_string());
            for skill in &self.skills {
                let description = if skill.description.trim().is_empty() {
                    String::new()
                } else {
                    format!(" - {}", skill.description.trim())
                };
                let path = if self.developer_mode {
                    format!(" @ {}", skill.path.display())
                } else {
                    String::new()
                };
                lines.push(format!(
                    "- {} ({}){}{}",
                    skill.name, skill.source, description, path
                ));
            }
            lines.push("Use load_skill when a turn needs that rule pack.".to_string());
        }
        if !self.panels.is_empty() {
            lines.push(String::new());
            lines.push("Panels:".to_string());
            for panel in &self.panels {
                lines.push(format!("## {}", panel.title));
                if !panel.body.is_empty() {
                    lines.push(panel.body.clone());
                }
            }
        }
        lines.join("\n")
    }

    fn choices_report(&self) -> Result<String> {
        let save = load_save(&self.saves_root, &self.save_id)?;
        Ok(format_playbook(&build_playbook(&save.state)))
    }

    fn skill_directories(&self) -> Vec<PathBuf> {
        let mut dirs = vec![
            self.game_root.join("skills"),
            self.saves_root.join(&self.save_id).join("skills"),
        ];
        if let Some(driver_root) = &self.driver_root {
            dirs.push(driver_root.join("skills"));
        }
        dirs.into_iter().filter(|dir| dir.is_dir()).collect()
    }
}

pub fn load_game_session(workspace: &Path, launch: GameLaunchOptions) -> Result<GameSession> {
    let Some(game_root) = resolve_game_root(workspace, launch.game_or_path.as_deref()) else {
        return Ok(GameSession::Notice(GameSessionNotice {
            message: "no game package selected".to_string(),
            developer_mode: launch.developer_mode,
        }));
    };

    let loaded_game = match deepseek_game::manifest::load_game(&game_root) {
        Ok(game) => game,
        Err(err) => {
            return Ok(GameSession::Notice(GameSessionNotice {
                message: format!("failed to load {}: {err}", game_root.display()),
                developer_mode: launch.developer_mode,
            }));
        }
    };
    let save_id = launch
        .save
        .or_else(|| loaded_game.manifest.game.default_save.clone())
        .unwrap_or_else(|| "default".to_string());
    let loaded_save = match load_save(&loaded_game.saves_root, &save_id) {
        Ok(save) => save,
        Err(err) => {
            return Ok(GameSession::Notice(GameSessionNotice {
                message: format!("failed to load save {save_id}: {err}"),
                developer_mode: launch.developer_mode,
            }));
        }
    };

    match build_loaded_session(loaded_game, loaded_save, launch.developer_mode) {
        Ok(session) => Ok(GameSession::Loaded(session)),
        Err(err) => Ok(GameSession::Notice(GameSessionNotice {
            message: format!("failed to load game session: {err}"),
            developer_mode: launch.developer_mode,
        })),
    }
}

fn resolve_game_root(workspace: &Path, explicit: Option<&Path>) -> Option<PathBuf> {
    explicit.map(Path::to_path_buf).or_else(|| {
        workspace
            .join("game.toml")
            .exists()
            .then(|| workspace.to_path_buf())
    })
}

fn build_loaded_session(
    loaded_game: LoadedGame,
    loaded_save: LoadedSave,
    developer_mode: bool,
) -> Result<LoadedGameSession> {
    let revision = loaded_save
        .state
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let locked_driver = driver_lock(&loaded_save.state)?;
    if locked_driver.id != loaded_game.manifest.driver.id {
        anyhow::bail!(
            "save locks driver {}, but game manifest requires {}",
            locked_driver.id,
            loaded_game.manifest.driver.id
        );
    }
    let resolved_driver = resolve_driver(&loaded_game, Some(&locked_driver.version))?;
    let locked_driver_version = resolved_driver.manifest.driver.version.clone();
    let skills = discover_game_skill_catalog(
        &loaded_game.root,
        &loaded_save.root,
        Some(&resolved_driver.root),
    );
    let mut warnings = loaded_game.warnings;
    warnings.extend(resolved_driver.warnings);
    Ok(LoadedGameSession {
        game_root: loaded_game.root,
        saves_root: loaded_game.saves_root,
        driver_root: Some(resolved_driver.root),
        game_id: loaded_game.manifest.game.id,
        title: loaded_game.manifest.game.title,
        save_id: loaded_save.id,
        revision,
        driver_id: loaded_game.manifest.driver.id,
        driver_requirement: loaded_game.manifest.driver.version,
        locked_driver_version: Some(locked_driver_version),
        panels: render_panels(&loaded_save.state),
        skills,
        warnings,
        developer_mode,
    })
}

fn discover_game_skill_catalog(
    game_root: &Path,
    save_root: &Path,
    driver_root: Option<&Path>,
) -> Vec<GameSkillCatalogEntry> {
    let mut roots = vec![
        ("game".to_string(), game_root.join("skills")),
        ("save".to_string(), save_root.join("skills")),
    ];
    if let Some(driver_root) = driver_root {
        roots.push(("driver".to_string(), driver_root.join("skills")));
    }

    let mut entries = Vec::new();
    for (source, root) in roots {
        if !root.is_dir() {
            continue;
        }
        let registry = crate::skills::SkillRegistry::discover(&root);
        for skill in registry.list() {
            if entries
                .iter()
                .any(|entry: &GameSkillCatalogEntry| entry.name == skill.name)
            {
                continue;
            }
            entries.push(GameSkillCatalogEntry {
                name: skill.name.clone(),
                description: skill.description.clone(),
                path: skill.path.clone(),
                source: source.clone(),
            });
        }
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

fn resolve_driver(loaded_game: &LoadedGame, locked_version: Option<&str>) -> Result<LoadedDriver> {
    let roots = driver_roots(&loaded_game.root);
    let resolver = DriverResolver::new(roots);
    let resolved = if let Some(version) = locked_version {
        resolver.resolve_exact(&loaded_game.manifest.driver.id, version)
    } else {
        resolver.resolve(
            &loaded_game.manifest.driver.id,
            &loaded_game.manifest.driver.version,
        )
    }?;
    Ok(resolved.loaded)
}

fn driver_roots(game_root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![game_root.join("drivers")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".deepseek").join("game-drivers"));
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use deepseek_game::script::DriverCall;
    use serde_json::json;

    #[test]
    fn bundled_reconciliation_demo_loads_with_local_driver() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/games/reconciliation-demo");
        let session = load_game_session(
            Path::new("."),
            GameLaunchOptions {
                game_or_path: Some(root),
                save: Some("default".to_string()),
                developer_mode: false,
            },
        )
        .expect("demo load should not error");

        let GameSession::Loaded(session) = session else {
            panic!("expected loaded demo game");
        };
        assert_eq!(session.game_id, "reconciliation-demo");
        assert_eq!(session.driver_id, "galgame");
        assert_eq!(session.locked_driver_version.as_deref(), Some("0.1.0"));
        assert!(session.driver_root.is_some());
        assert!(!session.panels.is_empty());
        assert!(session.warnings.is_empty(), "{:?}", session.warnings);
    }

    #[test]
    fn bundled_thirteen_angry_man_loads_with_deliberation_driver() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/games/thirteen-angry-man");
        let session = load_game_session(
            Path::new("."),
            GameLaunchOptions {
                game_or_path: Some(root),
                save: Some("default".to_string()),
                developer_mode: false,
            },
        )
        .expect("demo load should not error");

        let GameSession::Loaded(session) = session else {
            panic!("expected loaded deliberation game");
        };
        assert_eq!(session.game_id, "thirteen-angry-man");
        assert_eq!(session.driver_id, "deliberation-drama");
        assert_eq!(session.locked_driver_version.as_deref(), Some("0.1.0"));
        assert!(session.driver_root.is_some());
        assert!(
            session.panels.len() >= 6,
            "expected base panels plus action/story panels"
        );
        assert!(session.panels.iter().any(|panel| panel.id == "actions"));
        assert!(session.panels.iter().any(|panel| panel.id == "story"));
        assert!(session.warnings.is_empty(), "{:?}", session.warnings);

        let driver_root = session.driver_root.as_ref().expect("driver root");
        let driver = deepseek_game::driver::load_driver(driver_root).expect("driver should load");
        let result = deepseek_game::script::run_driver_function(
            driver_root,
            &driver.manifest,
            DriverCall {
                function: "advance_room".to_string(),
                args: [
                    ("action_type".to_string(), json!("reconstruction")),
                    ("clock_minutes".to_string(), json!(12)),
                    ("room_heat".to_string(), json!(3)),
                    ("fatigue".to_string(), json!(1)),
                    ("impatience".to_string(), json!(2)),
                    ("conflict_level".to_string(), json!(1)),
                    ("procedure_integrity".to_string(), json!(100)),
                ]
                .into_iter()
                .collect(),
            },
        )
        .expect("declared driver function should run");
        assert_eq!(result.result["clock_minutes"], 22);
        assert_eq!(result.result["time_delta"], 10);
    }

    #[test]
    fn save_locked_driver_must_resolve_exactly() {
        let temp = tempfile::tempdir().expect("tempdir");
        let game = temp.path().join("game");
        fs::create_dir_all(game.join("content")).expect("content dir");
        fs::create_dir_all(game.join("saves/default")).expect("save dir");
        fs::write(
            game.join("game.toml"),
            r#"
[game]
id = "strict-driver-test"
title = "Strict Driver Test"
version = "0.1.0"
default_save = "default"

[driver]
id = "deliberation-drama"
version = "^0.1"

[content]
roots = ["content"]

[saves]
root = "saves"
"#,
        )
        .expect("write manifest");
        fs::write(
            game.join("saves/default/STATE.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": 1,
                "revision": 0,
                "driver": {
                    "id": "deliberation-drama",
                    "version": "9.9.9"
                }
            }))
            .expect("serialize state"),
        )
        .expect("write state");
        fs::write(game.join("saves/default/TURN_LOG.jsonl"), "").expect("write turn log");

        let session = load_game_session(
            Path::new("."),
            GameLaunchOptions {
                game_or_path: Some(game),
                save: Some("default".to_string()),
                developer_mode: false,
            },
        )
        .expect("loading failures are represented as notices");

        let GameSession::Notice(notice) = session else {
            panic!("expected unresolved locked driver to produce a notice");
        };
        assert!(
            notice.message.contains("failed to load game session"),
            "{}",
            notice.message
        );
        assert!(
            notice.message.contains("9.9.9"),
            "notice should name the missing locked version: {}",
            notice.message
        );
    }
}
