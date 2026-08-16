//! Real-PTY journey for `/fleet setup`: create a project profile, reopen the
//! wizard for the same role, see the replace warning, switch the destination
//! to Personal, and save — with the resolved absolute path visible on the
//! Destination and Review screens before anything is written.

#![cfg(unix)]

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use crate::qa_harness::harness::{Harness, make_sealed_workspace};
use crate::qa_harness::keys;

static FLEET_SETUP_PTY_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    FLEET_SETUP_PTY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const BOOT_TIMEOUT: Duration = Duration::from_secs(20);
const STEP_TIMEOUT: Duration = Duration::from_secs(6);
const PASTE_GUARD_SETTLE: Duration = Duration::from_millis(250);

fn type_and_submit(h: &mut Harness, text: &str) -> anyhow::Result<()> {
    h.send(keys::key::text(text))?;
    h.wait_for_text(text, STEP_TIMEOUT)?;
    std::thread::sleep(PASTE_GUARD_SETTLE);
    h.pump();
    h.send(keys::key::enter())?;
    Ok(())
}

fn press(h: &mut Harness, bytes: Vec<u8>) -> anyhow::Result<()> {
    h.send(bytes)?;
    std::thread::sleep(Duration::from_millis(120));
    h.pump();
    Ok(())
}

#[test]
fn fleet_setup_journey_shows_destination_before_writing_and_never_replaces_silently()
-> anyhow::Result<()> {
    let _guard = lock();
    let ws = make_sealed_workspace()?;
    let codewhale_home = ws.home().join(".codewhale");
    let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
        .cwd(ws.workspace())
        .clear_env()
        .seal_home(ws.home())
        .env(
            "CODEWHALE_HOME",
            codewhale_home.to_str().expect("utf-8 home"),
        )
        .env("DEEPSEEK_API_KEY", "ci-test-key-not-real")
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("NO_ANIMATIONS", "1")
        .env("RUST_LOG", "warn")
        // No --no-project-config: project profiles must be a real, enabled
        // destination for this journey.
        .args([
            "--workspace",
            ws.workspace().to_str().expect("utf-8 workspace path"),
            "--skip-onboarding",
        ])
        .size(32, 120)
        .spawn()?;
    h.wait_for_text("Write a task", BOOT_TIMEOUT)?;

    // --- Create: manager · inherit · This project -----------------------
    type_and_submit(&mut h, "/fleet setup")?;
    h.wait_for_text("Choose a team role", STEP_TIMEOUT)?;
    // The destination is announced before it is chosen.
    h.wait_for_text("Saves to: choose in step 3", STEP_TIMEOUT)?;
    press(&mut h, keys::key::enter())?; // manager -> Model
    h.wait_for_text("Choose a model", STEP_TIMEOUT)?;
    press(&mut h, keys::key::enter())?; // inherit -> Destination
    h.wait_for_text("Where should this profile live?", STEP_TIMEOUT)?;
    h.wait_for_text("This project", STEP_TIMEOUT)?;
    h.wait_for_text("Personal", STEP_TIMEOUT)?;
    press(&mut h, keys::key::up())?; // highlight "This project"
    let project_target = ws
        .workspace()
        .join(".codewhale")
        .join("agents")
        .join("manager.toml");
    h.wait_for(
        |frame| {
            // The destination path can hard-wrap mid-token when the temp
            // directory is long (macOS /var/folders, Windows Temp, VIX
            // volumes), so compare with whitespace and borders removed.
            let text: String = frame
                .text()
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '│')
                .collect();
            text.contains("File:") && text.contains("agents") && text.contains("manager.toml")
        },
        STEP_TIMEOUT,
    )?;
    press(&mut h, keys::key::enter())?; // -> Review
    h.wait_for_text("Review & save", STEP_TIMEOUT)?;
    h.wait_for_text("Saves to: This project", STEP_TIMEOUT)?;
    h.wait_for_text("Save to this project", STEP_TIMEOUT)?;
    assert!(
        !project_target.exists(),
        "nothing may be written before the save control is activated"
    );
    // Tab moves focus; it must not change the destination.
    press(&mut h, keys::key::tab())?;
    h.wait_for_text("▸ Change destination", STEP_TIMEOUT)?;
    h.wait_for_text("Saves to: This project", STEP_TIMEOUT)?;
    press(&mut h, keys::key::backtab())?;
    h.wait_for_text("▸ Save to this project", STEP_TIMEOUT)?;
    press(&mut h, keys::key::enter())?; // save
    h.wait_for_text("Fleet project profile saved", STEP_TIMEOUT)?;
    assert!(project_target.is_file(), "{}", project_target.display());

    // --- Reopen: same role, replace warning, switch to Personal ---------
    type_and_submit(&mut h, "/fleet setup")?;
    h.wait_for_text("Choose a team role", STEP_TIMEOUT)?;
    press(&mut h, keys::key::enter())?; // manager
    h.wait_for_text("Choose a model", STEP_TIMEOUT)?;
    press(&mut h, keys::key::enter())?; // inherit
    h.wait_for_text("Where should this profile live?", STEP_TIMEOUT)?;
    press(&mut h, keys::key::up())?; // This project
    h.wait_for_text("Will replace the existing file", STEP_TIMEOUT)?;
    press(&mut h, keys::key::down())?; // Personal
    let personal_target = codewhale_home.join("agents").join("manager.toml");
    h.wait_for(
        |frame| {
            let text = frame.text();
            text.contains("already has a") && !text.contains("Will replace")
        },
        STEP_TIMEOUT,
    )?;
    press(&mut h, keys::key::enter())?; // -> Review
    h.wait_for_text("Saves to: Personal", STEP_TIMEOUT)?;
    h.wait_for_text("Save as Personal profile", STEP_TIMEOUT)?;
    assert!(!personal_target.exists());
    press(&mut h, keys::key::enter())?;
    h.wait_for_text("Fleet personal profile saved", STEP_TIMEOUT)?;
    assert!(personal_target.is_file(), "{}", personal_target.display());
    // The project file was not touched by the personal save.
    assert!(project_target.is_file());

    // --- Replace requires a second Enter --------------------------------
    type_and_submit(&mut h, "/fleet setup")?;
    h.wait_for_text("Choose a team role", STEP_TIMEOUT)?;
    press(&mut h, keys::key::enter())?;
    h.wait_for_text("Choose a model", STEP_TIMEOUT)?;
    press(&mut h, keys::key::enter())?;
    h.wait_for_text("Where should this profile live?", STEP_TIMEOUT)?;
    // Personal is highlighted (last chosen); it now exists.
    h.wait_for_text("Will replace the existing file", STEP_TIMEOUT)?;
    let before = std::fs::metadata(&personal_target)?.modified()?;
    press(&mut h, keys::key::enter())?; // -> Review
    h.wait_for_text("Replace Personal profile", STEP_TIMEOUT)?;
    press(&mut h, keys::key::enter())?; // arms
    h.wait_for_text("Press Enter again to replace manager.toml", STEP_TIMEOUT)?;
    assert_eq!(std::fs::metadata(&personal_target)?.modified()?, before);
    press(&mut h, keys::key::esc())?; // back to Destination — disarms, nothing written
    h.wait_for_text("Where should this profile live?", STEP_TIMEOUT)?;
    assert_eq!(std::fs::metadata(&personal_target)?.modified()?, before);
    press(&mut h, keys::key::esc())?;
    press(&mut h, keys::key::esc())?;
    press(&mut h, keys::key::esc())?; // Role -> close
    h.wait_for_text("Write a task", STEP_TIMEOUT)?;
    Ok(())
}
