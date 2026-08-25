//! List-stage action chords and footer hints for `/provider`.
//!
//! Extracted from `provider_picker.rs` (#5586). The wide footer advertises
//! `a-z` jump; letters bound to a categorical list action stay reserved
//! while the query is empty, and every other letter is type-ahead.
//! Row-dependent chords (`x` revoke, `e` consent) only reserve the letter
//! when the selected row can use them.
//!
//! LM Studio and DS4 are filled-in custom forms, not catalog filters, so
//! they do not steal `i`/`d` from type-ahead. Reach them via `P` (template
//! list) or the dedicated setup constructors, not a per-preset hotkey.

use std::borrow::Cow;

use crossterm::event::KeyModifiers;

use crate::localization::MessageId;
use crate::provider_readiness::CredentialState;
use crate::tui::views::{ActionHint, ViewAction, ViewEvent};

use super::{ProviderListView, ProviderPickerView};

/// Categorical / row-dependent list-stage char actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ListCharAction {
    ToggleView,
    ShowLocal,
    CustomForm,
    Templates,
    EditKey,
    OpenModels,
    RevokeExternalConsent,
    EnterExternalConsent,
    TestConnection,
    TypeAhead(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ListKeyContext {
    pub query_empty: bool,
    pub row_visible: bool,
    pub credential_state: CredentialState,
    pub has_external_consent_target: bool,
}

/// Classify a list-stage `KeyCode::Char` the way `handle_key` used to match
/// arms, minus the LM Studio / DS4 presets that stole `i`/`d`.
pub(super) fn classify_list_char(
    c: char,
    modifiers: KeyModifiers,
    ctx: ListKeyContext,
) -> Option<ListCharAction> {
    if modifiers.contains(KeyModifiers::CONTROL) {
        if c.eq_ignore_ascii_case(&'t') && ctx.row_visible {
            return Some(ListCharAction::TestConnection);
        }
        return None;
    }
    if !modifiers.is_empty() {
        return None;
    }
    if ctx.query_empty {
        if c.eq_ignore_ascii_case(&'x')
            && ctx.row_visible
            && ctx.credential_state == CredentialState::ExternalConsent
        {
            return Some(ListCharAction::RevokeExternalConsent);
        }
        if c.eq_ignore_ascii_case(&'e') && ctx.row_visible && ctx.has_external_consent_target {
            return Some(ListCharAction::EnterExternalConsent);
        }
        if c.eq_ignore_ascii_case(&'r') && ctx.row_visible {
            return Some(ListCharAction::EditKey);
        }
        if c.eq_ignore_ascii_case(&'a') {
            return Some(ListCharAction::ToggleView);
        }
        if c.eq_ignore_ascii_case(&'l') {
            return Some(ListCharAction::ShowLocal);
        }
        if c.eq_ignore_ascii_case(&'c') {
            return Some(ListCharAction::CustomForm);
        }
        if c.eq_ignore_ascii_case(&'p') {
            return Some(ListCharAction::Templates);
        }
        if c.eq_ignore_ascii_case(&'m') && ctx.row_visible {
            return Some(ListCharAction::OpenModels);
        }
    }
    Some(ListCharAction::TypeAhead(c))
}

impl ProviderPickerView {
    fn list_key_context(&self) -> ListKeyContext {
        let row_visible = self.row_visible(self.selected_idx);
        ListKeyContext {
            query_empty: self.query.is_empty(),
            row_visible,
            credential_state: self
                .rows
                .get(self.selected_idx)
                .map(|row| row.credential_state)
                .unwrap_or(CredentialState::MissingKey),
            has_external_consent_target: row_visible
                && self.selected_external_consent_target().is_some(),
        }
    }

    pub(super) fn handle_list_char(&mut self, c: char, modifiers: KeyModifiers) -> ViewAction {
        match classify_list_char(c, modifiers, self.list_key_context()) {
            Some(ListCharAction::ToggleView) => {
                self.toggle_view();
                ViewAction::None
            }
            Some(ListCharAction::ShowLocal) => {
                self.show_local_routes();
                ViewAction::None
            }
            Some(ListCharAction::CustomForm) => {
                self.enter_custom_form();
                ViewAction::None
            }
            Some(ListCharAction::Templates) => {
                self.enter_template_list();
                ViewAction::None
            }
            Some(ListCharAction::EditKey) => {
                self.begin_setup();
                ViewAction::None
            }
            Some(ListCharAction::OpenModels) => {
                let provider = self.selected_provider();
                let provider_id = self.selected_provider_id();
                ViewAction::EmitAndClose(ViewEvent::ProviderPickerOpenModels {
                    provider,
                    provider_id,
                })
            }
            Some(ListCharAction::RevokeExternalConsent) => {
                ViewAction::EmitAndClose(ViewEvent::ProviderPickerExternalConsentRevoked {
                    provider: self.selected_provider(),
                })
            }
            Some(ListCharAction::EnterExternalConsent) => {
                self.enter_external_consent_choice();
                ViewAction::None
            }
            Some(ListCharAction::TestConnection) => {
                ViewAction::EmitAndClose(ViewEvent::ProviderPickerTestConnection {
                    provider: self.selected_provider(),
                    provider_id: self.selected_provider_id(),
                    catalog_view: self.view == ProviderListView::Catalog,
                })
            }
            Some(ListCharAction::TypeAhead(ch)) => {
                let mut query = self.query.clone();
                query.push(ch);
                self.update_query(query);
                ViewAction::None
            }
            None => ViewAction::None,
        }
    }

    /// Action-bar hints for the list stage. `X` is advertised only when the
    /// highlighted row can actually revoke external consent.
    pub(super) fn list_stage_action_hints(
        &self,
        enter_action: Cow<'static, str>,
    ) -> Vec<ActionHint> {
        if self.onboarding_mode {
            let mut hints = vec![
                ActionHint::new("↑↓", self.tr(MessageId::PickerActionMove)),
                ActionHint::new("Enter", enter_action),
            ];
            if self.view == ProviderListView::Local {
                hints.push(ActionHint::new(
                    "A",
                    self.tr(MessageId::PickerActionBrowseAll),
                ));
            }
            hints.extend([
                ActionHint::new("Ctrl+O", self.tr(MessageId::OnboardProviderOffline)),
                ActionHint::new("Esc", self.tr(MessageId::OnboardActionBack)),
            ]);
            return hints;
        }

        let view_action = match self.view {
            ProviderListView::Configured => self.tr(MessageId::PickerActionBrowseAll),
            ProviderListView::Catalog => self.tr(MessageId::PickerActionConfigured),
            ProviderListView::Local => self.tr(MessageId::PickerActionBrowseAll),
        };
        let search_active = !self.query.trim().is_empty();
        if search_active {
            return vec![
                // Two-stage Esc (clear the query, then cancel) reads as one
                // hint instead of a duplicated key.
                ActionHint::new(
                    "Esc",
                    format!(
                        "{} / {}",
                        self.tr(MessageId::PickerActionClear),
                        self.tr(MessageId::PickerActionCancel)
                    ),
                ),
                ActionHint::new("↑↓", self.tr(MessageId::PickerActionMove)),
                ActionHint::new("Enter", enter_action),
                ActionHint::new("A", view_action),
                ActionHint::new("L", "local only"),
                ActionHint::new("C", self.tr(MessageId::PickerActionCustom)),
            ];
        }

        let mut hints = vec![
            ActionHint::new("↑↓", self.tr(MessageId::PickerActionMove)),
            ActionHint::new("a-z", self.tr(MessageId::PickerActionJump)),
            ActionHint::new("Enter", enter_action),
            ActionHint::new("A", view_action),
            ActionHint::new("L", "local only"),
            ActionHint::new("C", self.tr(MessageId::PickerActionCustom)),
            ActionHint::new("P", self.tr(MessageId::PickerActionTemplates)),
            ActionHint::new("C-t", self.tr(MessageId::PickerActionTestConnection)),
            ActionHint::new("R", self.tr(MessageId::PickerActionEditKey)),
        ];
        if self.row_visible(self.selected_idx)
            && self.rows[self.selected_idx].credential_state == CredentialState::ExternalConsent
        {
            hints.push(ActionHint::new(
                "X",
                self.tr(MessageId::ProviderExternalActionRevoke),
            ));
        }
        if self.list_key_context().has_external_consent_target {
            hints.push(ActionHint::new(
                "E",
                self.tr(MessageId::ProviderExternalActionChoices),
            ));
        }
        hints.push(ActionHint::new("M", self.tr(MessageId::PickerActionModels)));
        hints.push(ActionHint::new(
            "Esc",
            self.tr(MessageId::PickerActionCancel),
        ));
        hints
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_list() -> ListKeyContext {
        ListKeyContext {
            query_empty: true,
            row_visible: true,
            credential_state: CredentialState::Saved,
            has_external_consent_target: false,
        }
    }

    #[test]
    fn categorical_letters_stay_reserved() {
        let ctx = empty_list();
        assert_eq!(
            classify_list_char('a', KeyModifiers::NONE, ctx),
            Some(ListCharAction::ToggleView)
        );
        assert_eq!(
            classify_list_char('l', KeyModifiers::NONE, ctx),
            Some(ListCharAction::ShowLocal)
        );
        assert_eq!(
            classify_list_char('c', KeyModifiers::NONE, ctx),
            Some(ListCharAction::CustomForm)
        );
        assert_eq!(
            classify_list_char('p', KeyModifiers::NONE, ctx),
            Some(ListCharAction::Templates)
        );
        assert_eq!(
            classify_list_char('r', KeyModifiers::NONE, ctx),
            Some(ListCharAction::EditKey)
        );
        assert_eq!(
            classify_list_char('m', KeyModifiers::NONE, ctx),
            Some(ListCharAction::OpenModels)
        );
    }

    #[test]
    fn formerly_stolen_preset_letters_are_type_ahead() {
        let ctx = empty_list();
        assert_eq!(
            classify_list_char('d', KeyModifiers::NONE, ctx),
            Some(ListCharAction::TypeAhead('d'))
        );
        assert_eq!(
            classify_list_char('i', KeyModifiers::NONE, ctx),
            Some(ListCharAction::TypeAhead('i'))
        );
        assert_eq!(
            classify_list_char('D', KeyModifiers::NONE, ctx),
            Some(ListCharAction::TypeAhead('D'))
        );
        assert_eq!(
            classify_list_char('I', KeyModifiers::NONE, ctx),
            Some(ListCharAction::TypeAhead('I'))
        );
    }

    #[test]
    fn row_dependent_letters_only_reserve_when_usable() {
        let idle = empty_list();
        assert_eq!(
            classify_list_char('x', KeyModifiers::NONE, idle),
            Some(ListCharAction::TypeAhead('x'))
        );
        assert_eq!(
            classify_list_char('e', KeyModifiers::NONE, idle),
            Some(ListCharAction::TypeAhead('e'))
        );

        let revoke = ListKeyContext {
            credential_state: CredentialState::ExternalConsent,
            ..idle
        };
        assert_eq!(
            classify_list_char('x', KeyModifiers::NONE, revoke),
            Some(ListCharAction::RevokeExternalConsent)
        );

        let consent = ListKeyContext {
            has_external_consent_target: true,
            ..idle
        };
        assert_eq!(
            classify_list_char('e', KeyModifiers::NONE, consent),
            Some(ListCharAction::EnterExternalConsent)
        );
    }

    #[test]
    fn typing_releases_reserved_letters() {
        let ctx = ListKeyContext {
            query_empty: false,
            ..empty_list()
        };
        for c in ['a', 'l', 'c', 'p', 'r', 'm', 'd', 'i'] {
            assert_eq!(
                classify_list_char(c, KeyModifiers::NONE, ctx),
                Some(ListCharAction::TypeAhead(c)),
                "{c}"
            );
        }
    }

    #[test]
    fn ctrl_t_probes_even_during_search() {
        let ctx = ListKeyContext {
            query_empty: false,
            ..empty_list()
        };
        assert_eq!(
            classify_list_char('t', KeyModifiers::CONTROL, ctx),
            Some(ListCharAction::TestConnection)
        );
        assert_eq!(
            classify_list_char('t', KeyModifiers::CONTROL, empty_list()),
            Some(ListCharAction::TestConnection)
        );
        assert_eq!(
            classify_list_char('t', KeyModifiers::NONE, empty_list()),
            Some(ListCharAction::TypeAhead('t'))
        );
    }
}
