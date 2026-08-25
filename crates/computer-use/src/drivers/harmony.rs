//! HarmonyOS / OpenHarmony driver over `hdc` and the on-device `uitest`.
//!
//! Screenshots: `uitest screenCap` (falls back to `snapshot_display`), pulled
//! with `hdc file recv`. Input: `uitest uiInput …`. UI tree: `uitest
//! dumpLayout`. Apps: `aa start` / `bm dump`.

use std::path::PathBuf;
use std::time::Duration;

use crate::config::HarmonyConfig;
use crate::consent::AppIdentity;
use crate::driver::{
    AppAction, Button, Driver, DriverError, Point, RawFrame, ScrollDir, TargetInfo, TargetKind,
    UiNode,
};
use crate::drivers::device_elements;
use crate::elements::{
    ActionReceipt, AppInfo, AppState, ElementAction, ElementCaps, ElementDriver, ElementNode,
    StateOpts,
};
use crate::keys::{Key, KeyCombo, NamedKey};
use crate::process::{self, sh_quote};

const SHOT_TIMEOUT: Duration = Duration::from_secs(30);
const REMOTE_SHOT: &str = "/data/local/tmp/codewhale_shot.png";
const REMOTE_SHOT_JPEG: &str = "/data/local/tmp/codewhale_shot.jpeg";
const REMOTE_LAYOUT: &str = "/data/local/tmp/codewhale_layout.json";

pub struct HarmonyDriver {
    hdc: PathBuf,
    target: String,
    size: Option<(u32, u32)>,
    last_tap: Option<Point>,
    /// Last `computer_app_state` tree, so `#index` keeps meaning something.
    element_nodes: Vec<ElementNode>,
}

fn sdk_candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for var in [
        "HDC_HOME",
        "OHOS_SDK_HOME",
        "DEVECO_SDK_HOME",
        "OHOS_NATIVE_SDK",
    ] {
        if let Ok(root) = std::env::var(var)
            && !root.trim().is_empty()
        {
            let root = PathBuf::from(root.trim());
            dirs.push(root.join("toolchains"));
            dirs.push(root.join("default/openharmony/toolchains"));
            dirs.push(root.join("../toolchains"));
            dirs.push(root.clone());
        }
    }
    if let Some(home) = process::home() {
        for base in [
            "Library/OpenHarmony/Sdk",
            "Library/Huawei/Sdk/openharmony",
            "OpenHarmony/Sdk",
            "Huawei/Sdk/openharmony",
            "AppData/Local/OpenHarmony/Sdk",
            "AppData/Local/Huawei/Sdk/openharmony",
        ] {
            let base = home.join(base);
            if let Ok(entries) = std::fs::read_dir(&base) {
                for entry in entries.flatten() {
                    dirs.push(entry.path().join("toolchains"));
                }
            }
        }
    }
    dirs.push(PathBuf::from(
        "/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/toolchains",
    ));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs
}

/// Parse `hdc list targets` output into target keys.
pub fn parse_targets(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty() && !l.starts_with('[') && !l.to_ascii_lowercase().contains("empty")
        })
        .map(|l| l.split_whitespace().next().unwrap_or(l).to_string())
        .collect()
}

impl HarmonyDriver {
    pub fn new(cfg: &HarmonyConfig) -> Result<Self, DriverError> {
        let hdc = process::find_binary(&cfg.hdc, "hdc", &sdk_candidates()).ok_or_else(|| {
            DriverError::Unavailable(
                "hdc was not found; install the OpenHarmony/DevEco SDK toolchains (or set [harmony].hdc in ~/.codewhale/computer-use.toml)".to_string(),
            )
        })?;
        let mut driver = Self {
            hdc,
            target: cfg.target.trim().to_string(),
            size: None,
            last_tap: None,
            element_nodes: Vec::new(),
        };
        if driver.target.is_empty() {
            let out = process::run_ok(&driver.hdc, &["list", "targets"], process::DEFAULT_TIMEOUT)?;
            let targets = parse_targets(&out.stdout_text());
            match targets.as_slice() {
                [] => {
                    return Err(DriverError::Unavailable(
                        "no HarmonyOS device is connected (hdc list targets is empty); enable USB debugging or `hdc tconn host:port`".to_string(),
                    ));
                }
                [one] => driver.target = one.clone(),
                many => {
                    return Err(DriverError::Unavailable(format!(
                        "several HarmonyOS targets are connected ({}); set [harmony].target in ~/.codewhale/computer-use.toml",
                        many.join(", ")
                    )));
                }
            }
        }
        Ok(driver)
    }

    fn hdc(&self, args: &[&str], timeout: Duration) -> Result<process::Output, DriverError> {
        let mut full: Vec<&str> = vec!["-t", &self.target];
        full.extend_from_slice(args);
        process::run_ok(&self.hdc, &full, timeout)
    }

    fn shell(&self, command: &str) -> Result<String, DriverError> {
        Ok(self
            .hdc(&["shell", command], process::DEFAULT_TIMEOUT)?
            .stdout_text())
    }

    fn uiinput(&self, command: &str) -> Result<(), DriverError> {
        let out = self.shell(&format!("uitest uiInput {command}"))?;
        let lower = out.to_ascii_lowercase();
        if lower.contains("error") || lower.contains("invalid") || lower.contains("usage") {
            return Err(DriverError::Failed(format!(
                "uitest uiInput {command}: {}",
                process::tail(&out, 300)
            )));
        }
        Ok(())
    }

    fn pull(&self, remote: &str) -> Result<Vec<u8>, DriverError> {
        let dir = std::env::temp_dir();
        let local = dir.join(format!(
            "codewhale-cu-{}-{}",
            std::process::id(),
            remote.rsplit('/').next().unwrap_or("file")
        ));
        let local_str = local.to_string_lossy().into_owned();
        let result = self
            .hdc(&["file", "recv", remote, &local_str], SHOT_TIMEOUT)
            .and_then(|_| {
                std::fs::read(&local)
                    .map_err(|e| DriverError::Failed(format!("failed to read pulled file: {e}")))
            });
        let _ = std::fs::remove_file(&local);
        let _ = self.shell(&format!("rm -f {}", sh_quote(remote)));
        result
    }

    fn screen_size(&mut self) -> Result<(u32, u32), DriverError> {
        if let Some(size) = self.size {
            return Ok(size);
        }
        let frame = self.screenshot()?;
        let img = crate::frame::decode(&frame.bytes).map_err(DriverError::Failed)?;
        use image::GenericImageView;
        let size = img.dimensions();
        self.size = Some(size);
        Ok(size)
    }

    fn swipe(&self, from: Point, to: Point, duration_ms: u64) -> Result<(), DriverError> {
        let distance = ((to.x - from.x).powi(2) + (to.y - from.y).powi(2)).sqrt();
        // uitest wants a velocity in px/s (200..=40000).
        let velocity = if duration_ms == 0 {
            600.0
        } else {
            (distance / (duration_ms as f64 / 1000.0)).clamp(200.0, 40_000.0)
        };
        self.uiinput(&format!(
            "swipe {} {} {} {} {}",
            from.x.round() as i64,
            from.y.round() as i64,
            to.x.round() as i64,
            to.y.round() as i64,
            velocity.round() as i64
        ))
    }
}

/// OpenHarmony `KeyCode` values (`@ohos.multimodalInput.keyCode`).
fn keycode(key: &Key) -> Result<i32, DriverError> {
    Ok(match key {
        Key::Named(named) => match named {
            NamedKey::Enter => 2054,
            NamedKey::Tab => 2049,
            NamedKey::Escape => 2070,
            NamedKey::Backspace => 2055,
            NamedKey::Delete => 2071,
            NamedKey::Space => 2050,
            NamedKey::Up => 2012,
            NamedKey::Down => 2013,
            NamedKey::Left => 2014,
            NamedKey::Right => 2015,
            NamedKey::Home => 2081,
            NamedKey::End => 2082,
            NamedKey::PageUp => 2068,
            NamedKey::PageDown => 2069,
            NamedKey::Insert => 2083,
            NamedKey::CapsLock => 2074,
            NamedKey::F(n) => 2118 + i32::from(*n),
            NamedKey::Back => 2,
            NamedKey::AppHome => 1,
            NamedKey::Recents => 2067,
            NamedKey::Power => 18,
            NamedKey::VolumeUp => 16,
            NamedKey::VolumeDown => 17,
            NamedKey::Menu => 2067,
        },
        Key::Char(c) => {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() {
                2017 + (c as i32 - 'a' as i32)
            } else if c.is_ascii_digit() {
                2000 + (c as i32 - '0' as i32)
            } else {
                match c {
                    ' ' => 2050,
                    ',' => 2043,
                    '.' => 2044,
                    '-' => 2057,
                    '=' => 2058,
                    '+' => 2066,
                    '/' => 2064,
                    '\\' => 2061,
                    ';' => 2062,
                    '\'' => 2063,
                    '[' => 2059,
                    ']' => 2060,
                    '`' => 2056,
                    '@' => 2065,
                    '\t' => 2049,
                    '\n' => 2054,
                    other => {
                        return Err(DriverError::Failed(format!(
                            "no HarmonyOS keycode for `{other}`; use computer_type for text"
                        )));
                    }
                }
            }
        }
    })
}

impl Driver for HarmonyDriver {
    fn info(&mut self) -> Result<TargetInfo, DriverError> {
        let (w, h) = self.screen_size()?;
        let mut notes = vec![
            format!("hdc: {}", self.hdc.display()),
            format!("target: {}", self.target),
        ];
        if let Ok(model) = self.shell("param get const.product.model") {
            let model = model.trim();
            if !model.is_empty() && !model.contains("fail") {
                notes.push(format!("model: {model}"));
            }
        }
        if let Ok(version) = self.shell("param get const.ohos.fullname") {
            let version = version.trim();
            if !version.is_empty() && !version.contains("fail") {
                notes.push(format!("os: {version}"));
            }
        }
        notes.push("right-click = long press; hover is a no-op; text input targets the last tapped control when the device lacks focused-text input".to_string());
        Ok(TargetInfo {
            kind: TargetKind::Harmony,
            driver: "hdc-uitest".into(),
            device_w: w,
            device_h: h,
            notes,
            supports_ui_tree: true,
            supports_apps: true,
        })
    }

    fn screenshot(&mut self) -> Result<RawFrame, DriverError> {
        let bytes = match self.shell(&format!("uitest screenCap -p {REMOTE_SHOT}")) {
            Ok(out)
                if !out.to_ascii_lowercase().contains("fail")
                    && !out.to_ascii_lowercase().contains("error") =>
            {
                self.pull(REMOTE_SHOT)?
            }
            _ => {
                self.shell(&format!("snapshot_display -f {REMOTE_SHOT_JPEG}"))?;
                self.pull(REMOTE_SHOT_JPEG)?
            }
        };
        if bytes.is_empty() {
            return Err(DriverError::Failed("screenshot file was empty".to_string()));
        }
        let img = crate::frame::decode(&bytes).map_err(DriverError::Failed)?;
        use image::GenericImageView;
        self.size = Some(img.dimensions());
        Ok(RawFrame { bytes })
    }

    fn move_to(&mut self, _p: Point) -> Result<(), DriverError> {
        Ok(())
    }

    fn click(
        &mut self,
        p: Point,
        button: Button,
        clicks: u32,
        hold_ms: u64,
    ) -> Result<(), DriverError> {
        let (x, y) = (p.x.round() as i64, p.y.round() as i64);
        self.last_tap = Some(p);
        if hold_ms > 0 || button == Button::Right {
            return self.uiinput(&format!("longClick {x} {y}"));
        }
        match clicks {
            2 => self.uiinput(&format!("doubleClick {x} {y}")),
            3 => {
                self.uiinput(&format!("doubleClick {x} {y}"))?;
                self.uiinput(&format!("click {x} {y}"))
            }
            _ => self.uiinput(&format!("click {x} {y}")),
        }
    }

    fn drag(&mut self, from: Point, to: Point, duration_ms: u64) -> Result<(), DriverError> {
        self.swipe(from, to, duration_ms)
    }

    fn scroll(&mut self, p: Point, dir: ScrollDir, amount: u32) -> Result<(), DriverError> {
        let (w, h) = self.screen_size()?;
        let step_y = f64::from(h) * 0.12 * f64::from(amount.max(1));
        let step_x = f64::from(w) * 0.12 * f64::from(amount.max(1));
        let (dx, dy) = match dir {
            ScrollDir::Down => (0.0, -step_y),
            ScrollDir::Up => (0.0, step_y),
            ScrollDir::Right => (-step_x, 0.0),
            ScrollDir::Left => (step_x, 0.0),
        };
        let clamp = |v: f64, max: u32| v.clamp(1.0, f64::from(max.saturating_sub(2)));
        let from = Point {
            x: clamp(p.x - dx / 2.0, w),
            y: clamp(p.y - dy / 2.0, h),
        };
        let to = Point {
            x: clamp(p.x + dx / 2.0, w),
            y: clamp(p.y + dy / 2.0, h),
        };
        self.swipe(from, to, 300)
    }

    fn type_text(&mut self, text: &str) -> Result<(), DriverError> {
        for (i, segment) in text.split('\n').enumerate() {
            if i > 0 {
                self.uiinput("keyEvent 2054")?;
            }
            if segment.is_empty() {
                continue;
            }
            // Newer uitest types into the focused control; older builds need
            // a coordinate and tap the control first.
            if self.uiinput(&format!("text {}", sh_quote(segment))).is_ok() {
                continue;
            }
            let (w, h) = self.screen_size()?;
            let p = self.last_tap.unwrap_or(Point {
                x: f64::from(w) / 2.0,
                y: f64::from(h) / 2.0,
            });
            self.uiinput(&format!(
                "inputText {} {} {}",
                p.x.round() as i64,
                p.y.round() as i64,
                sh_quote(segment)
            ))?;
        }
        Ok(())
    }

    fn key(&mut self, combo: &KeyCombo) -> Result<(), DriverError> {
        let mut codes: Vec<i32> = Vec::new();
        if combo.modifiers.ctrl {
            codes.push(2072);
        }
        if combo.modifiers.alt {
            codes.push(2045);
        }
        if combo.modifiers.shift || matches!(combo.key, Key::Char(c) if c.is_ascii_uppercase()) {
            codes.push(2047);
        }
        if combo.modifiers.meta {
            codes.push(2076);
        }
        codes.push(keycode(&combo.key)?);
        if codes.len() > 3 {
            return Err(DriverError::Failed(
                "uitest keyEvent accepts at most two modifiers".to_string(),
            ));
        }
        let args: Vec<String> = codes.iter().map(i32::to_string).collect();
        self.uiinput(&format!("keyEvent {}", args.join(" ")))
    }

    fn ui_tree(&mut self) -> Result<Vec<UiNode>, DriverError> {
        let out = self.shell(&format!("uitest dumpLayout -p {REMOTE_LAYOUT}"))?;
        if out.to_ascii_lowercase().contains("fail") {
            return Err(DriverError::Failed(format!(
                "uitest dumpLayout: {}",
                process::tail(&out, 200)
            )));
        }
        let json = self.pull(REMOTE_LAYOUT)?;
        parse_dump_layout(&String::from_utf8_lossy(&json))
    }

    fn app(&mut self, action: AppAction<'_>) -> Result<String, DriverError> {
        match action {
            AppAction::Launch(name) => {
                let (bundle, ability) = match name.split_once('/') {
                    Some((b, a)) => (b.to_string(), a.to_string()),
                    None => (name.to_string(), "EntryAbility".to_string()),
                };
                let out = self.shell(&format!(
                    "aa start -b {} -a {}",
                    sh_quote(&bundle),
                    sh_quote(&ability)
                ))?;
                if out.to_ascii_lowercase().contains("error")
                    || out.to_ascii_lowercase().contains("fail")
                {
                    return Err(DriverError::Failed(format!(
                        "aa start {bundle}/{ability}: {}",
                        process::tail(&out, 300)
                    )));
                }
                Ok(format!("launched {bundle}/{ability}"))
            }
            AppAction::List => {
                let out = self.shell("bm dump -a")?;
                let mut names: Vec<&str> = out
                    .lines()
                    .map(str::trim)
                    .filter(|l| l.contains('.') && !l.contains(' ') && !l.ends_with(':'))
                    .collect();
                names.sort_unstable();
                names.dedup();
                Ok(format!("{} bundles:\n{}", names.len(), names.join("\n")))
            }
            AppAction::Current => {
                let out = self.shell("aa dump -l")?;
                let line = out
                    .lines()
                    .find(|l| l.contains("bundle name") || l.contains("bundleName"))
                    .map(|l| l.trim().to_string())
                    .unwrap_or_else(|| process::tail(&out, 300));
                Ok(format!("current: {line}"))
            }
        }
    }

    fn devices(&mut self) -> Result<String, DriverError> {
        let out = process::run_ok(
            &self.hdc,
            &["list", "targets", "-v"],
            process::DEFAULT_TIMEOUT,
        )?;
        Ok(format!(
            "selected: {}\n{}",
            self.target,
            out.stdout_text().trim()
        ))
    }

    fn element(&mut self) -> Option<&mut dyn ElementDriver> {
        Some(self)
    }
}

/// Parse `uitest dumpLayout` JSON into flat nodes (document order).
pub fn parse_dump_layout(json: &str) -> Result<Vec<UiNode>, DriverError> {
    let root: serde_json::Value = serde_json::from_str(json.trim())
        .map_err(|e| DriverError::Failed(format!("dumpLayout is not valid JSON: {e}")))?;
    let mut nodes = Vec::new();
    walk_layout(&root, 0, &mut nodes);
    Ok(nodes)
}

fn walk_layout(value: &serde_json::Value, depth: u8, out: &mut Vec<UiNode>) {
    let attrs = value.get("attributes").unwrap_or(value);
    let get = |name: &str| {
        attrs
            .get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
    };
    let is_true = |name: &str| {
        attrs
            .get(name)
            .map(|v| v.as_bool().unwrap_or(v.as_str() == Some("true")))
            .unwrap_or(false)
    };
    if let Some(bounds) = crate::drivers::android::parse_bounds(get("bounds")) {
        let text = get("text");
        let label = if text.is_empty() {
            get("description")
        } else {
            text
        };
        let class = get("type");
        out.push(UiNode {
            class: class.to_string(),
            label: label.chars().take(80).collect(),
            id: if get("id").is_empty() {
                get("key").to_string()
            } else {
                get("id").to_string()
            },
            bounds,
            clickable: is_true("clickable") || is_true("longClickable") || is_true("checkable"),
            scrollable: is_true("scrollable"),
            editable: class.contains("TextInput")
                || class.contains("TextArea")
                || class.contains("Search"),
            focused: is_true("focused"),
            depth,
        });
    }
    if let Some(children) = value.get("children").and_then(serde_json::Value::as_array) {
        for child in children {
            walk_layout(child, depth.saturating_add(1), out);
        }
    }
}

// ---------------------------------------------------------------------------
// Element surface (phase 2): the uitest layout tree, addressed by index.
// ---------------------------------------------------------------------------

impl ElementDriver for HarmonyDriver {
    fn apps(&mut self) -> Result<Vec<AppInfo>, DriverError> {
        let out = self.shell("ps -A -o PID,NAME")?;
        let packages = device_elements::parse_running_packages(&out);
        if packages.is_empty() {
            return Err(DriverError::Failed(
                "`ps -A -o PID,NAME` listed no app processes; computer_app with action=list shows installed bundles instead".to_string(),
            ));
        }
        let (w, h) = self.screen_size()?;
        Ok(packages
            .into_iter()
            .map(|(pid, bundle)| {
                let identity = device_elements::device_identity(pid, &bundle);
                AppInfo {
                    windows: vec![crate::elements::WindowInfo {
                        id: 0,
                        title: identity.label(),
                        x: 0,
                        y: 0,
                        w,
                        h,
                    }],
                    identity,
                }
            })
            .collect())
    }

    fn app_state(&mut self, app: &AppIdentity, opts: &StateOpts) -> Result<AppState, DriverError> {
        let device = self.screen_size()?;
        let state = device_elements::snapshot(self, app, opts, device)?;
        self.element_nodes = state.nodes.clone();
        Ok(state)
    }

    fn act(
        &mut self,
        app: &AppIdentity,
        action: ElementAction,
    ) -> Result<ActionReceipt, DriverError> {
        let nodes = std::mem::take(&mut self.element_nodes);
        let outcome = device_elements::act(self, app, action, &nodes);
        self.element_nodes = nodes;
        outcome
    }

    fn raise(&mut self, app: &AppIdentity) -> Result<(), DriverError> {
        let bundle = if app.bundle_id.is_empty() {
            app.name.clone()
        } else {
            app.bundle_id.clone()
        };
        Driver::app(self, AppAction::Launch(&bundle))?;
        Ok(())
    }

    fn caps(&self) -> ElementCaps {
        device_elements::caps(
            "HarmonyOS: uitest layout tree with indexed taps and text; actions go to the foreground app, not the background",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_target_list() {
        assert_eq!(parse_targets("[Empty]\n"), Vec::<String>::new());
        assert_eq!(
            parse_targets("7001005458323933328a01fce16d3800\n"),
            vec!["7001005458323933328a01fce16d3800".to_string()]
        );
        assert_eq!(
            parse_targets("127.0.0.1:5555\tConnected\n"),
            vec!["127.0.0.1:5555".to_string()]
        );
    }

    #[test]
    fn parses_dump_layout() {
        let json = r#"{"attributes":{"accessibilityId":"1","bounds":"[0,0][1260,2720]","type":"root"},"children":[{"attributes":{"accessibilityId":"2","bounds":"[100,200][400,300]","clickable":"true","text":"Continue","type":"Button","id":"btn_go","enabled":"true","focused":"false"},"children":[]},{"attributes":{"bounds":"[100,400][1100,520]","type":"TextInput","description":"Search","focused":"true"},"children":[]}]}"#;
        let nodes = parse_dump_layout(json).unwrap();
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[1].label, "Continue");
        assert_eq!(nodes[1].id, "btn_go");
        assert!(nodes[1].clickable);
        assert_eq!(nodes[1].center(), (250, 250));
        assert_eq!(nodes[1].depth, 1);
        assert!(nodes[2].editable && nodes[2].focused);
        assert_eq!(nodes[2].label, "Search");
        assert!(parse_dump_layout("nope").is_err());
    }

    #[test]
    fn keycodes_follow_ohos_table() {
        assert_eq!(keycode(&Key::Named(NamedKey::Back)).unwrap(), 2);
        assert_eq!(keycode(&Key::Named(NamedKey::Enter)).unwrap(), 2054);
        assert_eq!(keycode(&Key::Char('a')).unwrap(), 2017);
        assert_eq!(keycode(&Key::Char('Z')).unwrap(), 2042);
        assert_eq!(keycode(&Key::Char('0')).unwrap(), 2000);
        assert_eq!(keycode(&Key::Named(NamedKey::F(1))).unwrap(), 2119);
    }
}
