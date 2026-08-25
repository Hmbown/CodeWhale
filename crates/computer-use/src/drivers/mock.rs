//! Scripted driver for tests: records every call, returns a synthetic frame.

use std::cell::RefCell;
use std::rc::Rc;

use crate::consent::AppIdentity;
use crate::driver::{
    AppAction, Button, Driver, DriverError, Point, RawFrame, ScrollDir, TargetInfo, TargetKind,
    UiNode,
};
use crate::elements::{
    ActionReceipt, AppInfo, AppState, ElementAction, ElementCaps, ElementDriver, ElementNode,
    StateOpts, WindowInfo,
};
use crate::keys::KeyCombo;

#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    Screenshot,
    Move(Point),
    Click(Point, Button, u32, u64),
    Drag(Point, Point, u64),
    Scroll(Point, ScrollDir, u32),
    Type(String),
    Key(String),
    UiTree,
    App(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ElementCall {
    Apps,
    State(String, StateOpts),
    Act(String, ElementAction),
    Raise(String),
}

pub struct MockDriver {
    pub calls: Rc<RefCell<Vec<Call>>>,
    pub width: u32,
    pub height: u32,
    pub kind: TargetKind,
    pub nodes: Vec<UiNode>,
    pub element_enabled: bool,
    pub apps_list: Vec<AppInfo>,
    pub element_nodes: Vec<ElementNode>,
    pub element_calls: Rc<RefCell<Vec<ElementCall>>>,
}

impl MockDriver {
    pub fn new(width: u32, height: u32) -> (Self, Rc<RefCell<Vec<Call>>>) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        (
            Self {
                calls: calls.clone(),
                width,
                height,
                kind: TargetKind::Desktop,
                nodes: Vec::new(),
                element_enabled: false,
                apps_list: Vec::new(),
                element_nodes: Vec::new(),
                element_calls: Rc::new(RefCell::new(Vec::new())),
            },
            calls,
        )
    }
}

impl Driver for MockDriver {
    fn info(&mut self) -> Result<TargetInfo, DriverError> {
        Ok(TargetInfo {
            kind: self.kind,
            driver: "mock".into(),
            device_w: self.width,
            device_h: self.height,
            notes: vec!["mock driver".into()],
            supports_ui_tree: !self.nodes.is_empty(),
            supports_apps: false,
        })
    }

    fn screenshot(&mut self) -> Result<RawFrame, DriverError> {
        self.calls.borrow_mut().push(Call::Screenshot);
        Ok(RawFrame {
            bytes: crate::frame::synthetic_png(self.width, self.height),
        })
    }

    fn move_to(&mut self, p: Point) -> Result<(), DriverError> {
        self.calls.borrow_mut().push(Call::Move(p));
        Ok(())
    }

    fn click(
        &mut self,
        p: Point,
        button: Button,
        clicks: u32,
        hold_ms: u64,
    ) -> Result<(), DriverError> {
        self.calls
            .borrow_mut()
            .push(Call::Click(p, button, clicks, hold_ms));
        Ok(())
    }

    fn drag(&mut self, from: Point, to: Point, duration_ms: u64) -> Result<(), DriverError> {
        self.calls
            .borrow_mut()
            .push(Call::Drag(from, to, duration_ms));
        Ok(())
    }

    fn scroll(&mut self, p: Point, dir: ScrollDir, amount: u32) -> Result<(), DriverError> {
        self.calls.borrow_mut().push(Call::Scroll(p, dir, amount));
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), DriverError> {
        self.calls.borrow_mut().push(Call::Type(text.to_string()));
        Ok(())
    }

    fn key(&mut self, combo: &KeyCombo) -> Result<(), DriverError> {
        self.calls.borrow_mut().push(Call::Key(combo.to_string()));
        Ok(())
    }

    fn ui_tree(&mut self) -> Result<Vec<UiNode>, DriverError> {
        self.calls.borrow_mut().push(Call::UiTree);
        if self.nodes.is_empty() {
            Err(DriverError::Unsupported(
                "no UI tree on the mock desktop".into(),
            ))
        } else {
            Ok(self.nodes.clone())
        }
    }

    fn app(&mut self, action: AppAction<'_>) -> Result<String, DriverError> {
        self.calls
            .borrow_mut()
            .push(Call::App(format!("{action:?}")));
        Ok(format!("{action:?}"))
    }

    fn devices(&mut self) -> Result<String, DriverError> {
        Ok("mock device".into())
    }

    fn element(&mut self) -> Option<&mut dyn ElementDriver> {
        if self.element_enabled {
            Some(self)
        } else {
            None
        }
    }
}

impl ElementDriver for MockDriver {
    fn apps(&mut self) -> Result<Vec<AppInfo>, DriverError> {
        self.element_calls.borrow_mut().push(ElementCall::Apps);
        Ok(self.apps_list.clone())
    }

    fn app_state(&mut self, app: &AppIdentity, opts: &StateOpts) -> Result<AppState, DriverError> {
        self.element_calls
            .borrow_mut()
            .push(ElementCall::State(app.label(), opts.clone()));
        let window = app
            .pid
            .checked_mul(0)
            .map(|_| ())
            .and_then(|_| {
                self.apps_list
                    .iter()
                    .find(|a| a.identity.label() == app.label())
            })
            .and_then(|a| a.windows.first().cloned())
            .unwrap_or(WindowInfo {
                id: 1,
                title: "Mock window".into(),
                x: 0,
                y: 0,
                w: self.width,
                h: self.height,
            });
        Ok(AppState {
            identity: app.clone(),
            window,
            image_png: if opts.mode == crate::elements::StateMode::Ax {
                None
            } else {
                Some(crate::frame::synthetic_png(self.width, self.height))
            },
            image_w: self.width,
            image_h: self.height,
            nodes: self.element_nodes.clone(),
            omitted: 0,
            occluded: false,
        })
    }

    fn act(
        &mut self,
        app: &AppIdentity,
        action: ElementAction,
    ) -> Result<ActionReceipt, DriverError> {
        self.element_calls
            .borrow_mut()
            .push(ElementCall::Act(app.label(), action.clone()));
        Ok(ActionReceipt {
            text: format!("mock action on {}", app.label()),
            verified: matches!(action, ElementAction::SetValue { .. }).then_some(true),
            moved: matches!(action, ElementAction::Scroll { .. }).then_some(true),
        })
    }

    fn raise(&mut self, app: &AppIdentity) -> Result<(), DriverError> {
        self.element_calls
            .borrow_mut()
            .push(ElementCall::Raise(app.label()));
        Ok(())
    }

    fn caps(&self) -> ElementCaps {
        ElementCaps {
            tree: true,
            window_image: true,
            background_actions: true,
            note: "mock element driver",
        }
    }
}
