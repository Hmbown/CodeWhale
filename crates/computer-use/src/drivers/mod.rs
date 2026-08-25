//! Backend selection.
//!
//! Device drivers (`adb`, `hdc`) are pure process wrappers and compile on
//! every target. Desktop drivers are gated per OS: OpenHarmony and HarmonyOS
//! PC hosts report `target_os = "linux"` and use the Linux driver.

use crate::config::{Config, Target};
use crate::driver::{Driver, DriverError};

pub mod android;
pub mod device_elements;
pub mod harmony;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub mod macos_ax;
#[cfg(test)]
pub mod mock;
#[cfg(target_os = "windows")]
pub mod windows;

pub fn select_driver(cfg: &Config) -> Result<Box<dyn Driver>, DriverError> {
    match cfg.effective_target() {
        Target::Android => Ok(Box::new(android::AndroidDriver::new(&cfg.android)?)),
        Target::Harmony => Ok(Box::new(harmony::HarmonyDriver::new(&cfg.harmony)?)),
        Target::Desktop | Target::Auto => desktop(cfg),
    }
}

#[cfg(target_os = "macos")]
fn desktop(_cfg: &Config) -> Result<Box<dyn Driver>, DriverError> {
    Ok(Box::new(macos::MacDriver::new()?))
}

#[cfg(target_os = "windows")]
fn desktop(_cfg: &Config) -> Result<Box<dyn Driver>, DriverError> {
    Ok(Box::new(windows::WindowsDriver::new()?))
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
fn desktop(cfg: &Config) -> Result<Box<dyn Driver>, DriverError> {
    Ok(Box::new(linux::LinuxDriver::new(&cfg.linux)?))
}

#[cfg(target_os = "android")]
fn desktop(_cfg: &Config) -> Result<Box<dyn Driver>, DriverError> {
    Err(DriverError::Unavailable(
        "there is no desktop on Android; enable wireless debugging, run `adb connect localhost:<port>` in Termux, and set target = \"android\"".to_string(),
    ))
}

#[cfg(not(any(unix, target_os = "windows")))]
fn desktop(_cfg: &Config) -> Result<Box<dyn Driver>, DriverError> {
    Err(DriverError::Unsupported(
        "no desktop driver for this platform".to_string(),
    ))
}
