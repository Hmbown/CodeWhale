//! The element surface for phones (Android `adb`, HarmonyOS `hdc`).
//!
//! Devices already publish a UI tree with pixel bounds, and they have one
//! foreground app filling the screen — so phase 2 costs them almost nothing:
//! the tree becomes the indexed element list, the "window" is the display,
//! and element actions map onto the phase-1 taps, swipes, and `input text`
//! the driver already performs. What phones do **not** get is background
//! operation: every action still goes to whatever is in front, which is why
//! [`caps`] reports `background_actions: false`.

use crate::consent::AppIdentity;
use crate::driver::{Button, Driver, DriverError, Point, ScrollDir, UiNode};
use crate::elements::{
    ActionReceipt, AppState, ElementAction, ElementCaps, ElementNode, StateMode, StateOpts,
    WindowInfo,
};

/// One "page" of scrolling in the device driver's half-screens/5 unit.
const SCROLL_UNITS_PER_PAGE: u32 = 10;

/// Long-press duration that phones read as "open the context menu".
const LONG_PRESS_MS: u64 = 600;

/// Map a device UI-tree node onto an element node. Device trees are already
/// in display pixels and there is exactly one full-screen window, so the
/// frame is copied straight across.
pub fn element_from_ui_node(index: usize, node: &UiNode) -> ElementNode {
    let (left, top, right, bottom) = node.bounds;
    let mut actions = Vec::new();
    if node.clickable {
        actions.push("press".to_string());
    }
    if node.scrollable {
        actions.push("scroll".to_string());
    }
    if node.editable {
        actions.push("set_value".to_string());
    }
    let title = if node.label.trim().is_empty() {
        node.id.clone()
    } else {
        node.label.clone()
    };
    ElementNode {
        index,
        role: node.class.clone(),
        title,
        x: left,
        y: top,
        w: right.saturating_sub(left),
        h: bottom.saturating_sub(top),
        actions,
        focused: node.focused,
        enabled: true,
        // Device trees do not expose a "password field" flag; the Skill's
        // stop rules cover credentials here.
        secure: false,
        depth: node.depth,
    }
}

/// `ps -A -o PID,NAME` (toybox on Android, BusyBox/toybox on OpenHarmony)
/// reduced to the app processes: package-like names, lowest pid wins.
pub fn parse_running_packages(output: &str) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((pid, name)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = pid.trim().parse::<u32>() else {
            // The header row and any banner text land here.
            continue;
        };
        let name = name.trim();
        // Kernel threads are bracketed; system daemons live under a path;
        // app processes are named after their package.
        if !name.contains('.')
            || name.starts_with('[')
            || name.contains('/')
            || name.contains(':')
            || name.ends_with(".so")
        {
            continue;
        }
        if out.iter().any(|(_, existing)| existing == name) {
            continue;
        }
        out.push((pid, name.to_string()));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// The identity of a device app: the package/bundle name is the whole
/// identity, so every alias is the same string.
pub fn device_identity(pid: u32, package: &str) -> AppIdentity {
    AppIdentity {
        pid,
        name: package.to_string(),
        bundle_id: package.to_string(),
        process_name: package.to_string(),
    }
}

/// Capture the current UI tree as one full-screen "window".
///
/// There is no window-scoped image on a phone: the display *is* the window,
/// and `computer_screenshot` already returns it at the model's pixel budget.
/// Element frames are therefore reported in display pixels, which keeps the
/// session's image→window mapping the identity.
pub fn snapshot(
    driver: &mut (impl Driver + ?Sized),
    app: &AppIdentity,
    opts: &StateOpts,
    device: (u32, u32),
) -> Result<AppState, DriverError> {
    if opts.mode == StateMode::Image {
        return Err(DriverError::Unsupported(
            "there is no window-scoped image on a phone; use computer_screenshot for pixels, or mode=ax for the element tree".to_string(),
        ));
    }
    let nodes: Vec<ElementNode> = driver
        .ui_tree()?
        .iter()
        .enumerate()
        .map(|(index, node)| element_from_ui_node(index, node))
        .collect();
    Ok(AppState {
        identity: app.clone(),
        window: WindowInfo {
            id: 0,
            title: app.label(),
            x: 0,
            y: 0,
            w: device.0,
            h: device.1,
        },
        image_png: None,
        image_w: device.0,
        image_h: device.1,
        nodes,
        omitted: 0,
        occluded: false,
    })
}

fn center(nodes: &[ElementNode], index: usize) -> Result<Point, DriverError> {
    let node = nodes.get(index).ok_or_else(|| {
        DriverError::Failed(format!(
            "index {index} is out of range; the last app-state listed {} elements",
            nodes.len()
        ))
    })?;
    let (x, y) = node.center();
    Ok(Point { x, y })
}

fn describe(nodes: &[ElementNode], index: usize) -> String {
    match nodes.get(index) {
        Some(node) if !node.title.trim().is_empty() => {
            format!("#{index} [{}] \"{}\"", node.role, node.title)
        }
        Some(node) => format!("#{index} [{}]", node.role),
        None => format!("#{index}"),
    }
}

/// Perform one element action by translating it into the phase-1 gestures
/// the device driver already knows.
pub fn act(
    driver: &mut (impl Driver + ?Sized),
    app: &AppIdentity,
    action: ElementAction,
    nodes: &[ElementNode],
) -> Result<ActionReceipt, DriverError> {
    match action {
        ElementAction::Press { index } => {
            let point = center(nodes, index)?;
            driver.click(point, Button::Left, 1, 0)?;
            Ok(ActionReceipt {
                text: format!(
                    "tapped {} at ({:.0}, {:.0})",
                    describe(nodes, index),
                    point.x,
                    point.y
                ),
                ..Default::default()
            })
        }
        ElementAction::Click {
            x,
            y,
            button,
            clicks,
            hold_ms,
        } => {
            driver.click(Point { x, y }, button, clicks, hold_ms)?;
            Ok(ActionReceipt {
                text: format!("tapped ({x:.0}, {y:.0}) on `{}`", app.label()),
                ..Default::default()
            })
        }
        ElementAction::SetValue { index, value } => {
            let point = center(nodes, index)?;
            driver.click(point, Button::Left, 1, 0)?;
            driver.type_text(&value)?;
            Ok(ActionReceipt {
                text: format!(
                    "focused {} and typed the value (the field was not cleared first; call computer_app_state to confirm what it now holds)",
                    describe(nodes, index)
                ),
                ..Default::default()
            })
        }
        ElementAction::Type {
            text,
            index,
            point,
            clear,
            submit,
            activate: _,
        } => {
            let focus = match (index, point) {
                (Some(index), _) => Some((center(nodes, index)?, describe(nodes, index))),
                (None, Some((x, y))) => Some((Point { x, y }, format!("({x:.0}, {y:.0})"))),
                (None, None) => None,
            };
            if let Some((at, _)) = &focus {
                driver.click(*at, Button::Left, 1, 0)?;
            }
            if clear {
                driver.key(&crate::keys::select_all())?;
                driver.key(&crate::keys::named(crate::keys::NamedKey::Backspace))?;
            }
            if !text.is_empty() {
                driver.type_text(&text)?;
            }
            if submit {
                driver.key(&crate::keys::named(crate::keys::NamedKey::Enter))?;
            }
            let mut bits: Vec<String> = Vec::new();
            if let Some((_, where_)) = focus {
                bits.push(format!("tapped {where_}"));
            }
            if clear {
                bits.push("cleared the field".into());
            }
            let count = text.chars().count();
            if count > 0 {
                bits.push(format!("typed {count} characters"));
            }
            if submit {
                bits.push("pressed enter".into());
            }
            Ok(ActionReceipt {
                text: if bits.is_empty() {
                    "typed 0 characters into the focused field".into()
                } else {
                    bits.join(", then ")
                },
                ..Default::default()
            })
        }
        ElementAction::Key { combo, activate: _ } => {
            driver.key(&combo)?;
            Ok(ActionReceipt {
                text: format!("sent {combo}"),
                ..Default::default()
            })
        }
        ElementAction::Menu { index } => {
            let point = center(nodes, index)?;
            driver.click(point, Button::Left, 1, LONG_PRESS_MS)?;
            Ok(ActionReceipt {
                text: format!(
                    "long-pressed {} ({LONG_PRESS_MS}ms), the phone equivalent of opening a context menu",
                    describe(nodes, index)
                ),
                ..Default::default()
            })
        }
        ElementAction::Scroll {
            index,
            point,
            dir,
            pages,
        } => {
            let at = match (index, point) {
                (Some(index), _) => center(nodes, index)?,
                (None, Some((x, y))) => Point { x, y },
                (None, None) => {
                    return Err(DriverError::Failed(
                        "scrolling a phone needs an element index or a point".to_string(),
                    ));
                }
            };
            driver.scroll(at, dir, pages.max(1).saturating_mul(SCROLL_UNITS_PER_PAGE))?;
            Ok(ActionReceipt {
                text: format!("swiped {} page(s) {}", pages.max(1), scroll_label(dir)),
                // Devices give no scroll position to compare; re-state to see
                // whether the content moved.
                moved: None,
                verified: None,
            })
        }
        ElementAction::SelectText { .. } => Err(DriverError::Unsupported(
            "phones expose no text-selection range; long-press the text (computer_element action=menu) and use the on-screen selection handles".to_string(),
        )),
        ElementAction::Drag {
            from,
            to,
            duration_ms,
        } => {
            driver.drag(
                Point {
                    x: from.0,
                    y: from.1,
                },
                Point { x: to.0, y: to.1 },
                duration_ms,
            )?;
            Ok(ActionReceipt {
                text: format!(
                    "swiped ({:.0}, {:.0}) → ({:.0}, {:.0})",
                    from.0, from.1, to.0, to.1
                ),
                ..Default::default()
            })
        }
    }
}

fn scroll_label(dir: ScrollDir) -> &'static str {
    match dir {
        ScrollDir::Up => "up",
        ScrollDir::Down => "down",
        ScrollDir::Left => "left",
        ScrollDir::Right => "right",
    }
}

pub fn caps(note: &'static str) -> ElementCaps {
    ElementCaps {
        tree: true,
        window_image: false,
        background_actions: false,
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_packages_keep_app_processes_only() {
        let ps = "\
  PID NAME
    1 init
  442 [kworker/0:1]
  910 /system/bin/surfaceflinger
 1201 com.android.settings
 1330 com.android.chrome
 1331 com.android.chrome:sandboxed_process0
 1402 com.android.settings
";
        let packages = parse_running_packages(ps);
        assert_eq!(
            packages,
            vec![
                (1330, "com.android.chrome".to_string()),
                (1201, "com.android.settings".to_string()),
            ],
            "kernel threads, daemons, child processes and duplicates drop out"
        );
    }

    #[test]
    fn ui_nodes_become_indexed_elements_with_actions() {
        let node = UiNode {
            class: "android.widget.EditText".into(),
            label: String::new(),
            id: "com.example:id/search".into(),
            bounds: (10, 20, 110, 60),
            clickable: true,
            scrollable: false,
            editable: true,
            focused: true,
            depth: 3,
        };
        let element = element_from_ui_node(7, &node);
        assert_eq!(element.index, 7);
        assert_eq!(element.role, "android.widget.EditText");
        // With no visible text the resource id is the best label there is.
        assert_eq!(element.title, "com.example:id/search");
        assert_eq!(
            (element.x, element.y, element.w, element.h),
            (10, 20, 100, 40)
        );
        assert_eq!(element.actions, vec!["press", "set_value"]);
        assert!(element.focused);
        assert!(element.is_interactive());
        assert_eq!(element.center(), (60.0, 40.0));
    }

    #[test]
    fn device_identity_answers_to_the_package_name() {
        let app = device_identity(1201, "com.android.settings");
        assert_eq!(app.label(), "com.android.settings");
        assert_eq!(app.aliases(), vec!["com.android.settings".to_string()]);
    }

    #[test]
    fn type_with_index_taps_then_types() {
        let (mut driver, calls) = crate::drivers::mock::MockDriver::new(200, 100);
        let node = UiNode {
            class: "android.widget.EditText".into(),
            label: "Search".into(),
            id: String::new(),
            bounds: (10, 20, 110, 60),
            clickable: true,
            scrollable: false,
            editable: true,
            focused: false,
            depth: 3,
        };
        let nodes = vec![element_from_ui_node(0, &node)];
        let app = device_identity(1, "com.example.app");
        let receipt = act(
            &mut driver,
            &app,
            ElementAction::Type {
                text: "hi".into(),
                index: Some(0),
                point: None,
                clear: false,
                submit: false,
                activate: false,
            },
            &nodes,
        )
        .unwrap();
        assert!(receipt.text.contains("tapped"), "{}", receipt.text);
        assert!(receipt.text.contains("Search"), "{}", receipt.text);
        let recorded = calls.borrow();
        assert_eq!(recorded.len(), 2, "{recorded:?}");
        match &recorded[0] {
            crate::drivers::mock::Call::Click(p, Button::Left, 1, 0) => {
                assert!(
                    (p.x - 60.0).abs() < 0.5 && (p.y - 40.0).abs() < 0.5,
                    "{p:?}"
                );
            }
            other => panic!("expected tap, got {other:?}"),
        }
        assert_eq!(recorded[1], crate::drivers::mock::Call::Type("hi".into()));
    }

    #[test]
    fn type_without_focus_only_types() {
        let (mut driver, calls) = crate::drivers::mock::MockDriver::new(200, 100);
        let app = device_identity(1, "com.example.app");
        act(
            &mut driver,
            &app,
            ElementAction::Type {
                text: "hi".into(),
                index: None,
                point: None,
                clear: false,
                submit: false,
                activate: false,
            },
            &[],
        )
        .unwrap();
        assert_eq!(
            *calls.borrow(),
            vec![crate::drivers::mock::Call::Type("hi".into())]
        );
    }
}
