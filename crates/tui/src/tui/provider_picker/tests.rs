use super::*;
use crate::config::has_api_key_for;
use crate::test_support::EnvVarGuard;
use crossterm::event::{KeyEvent, KeyModifiers};

// Environment-mutating tests in this module hold the process-wide
// `lock_test_env()` (via `crate::test_support`), the same barrier every
// other module's env tests use. A module-private mutex cannot serialize
// against the rest of the suite, so sibling tests raced on shared
// provider env vars (EXAMPLE_API_KEY, OPENROUTER_API_KEY, ...) and a panic
// while holding it cascaded PoisonError failures into unrelated tests.

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

fn move_to_provider(picker: &mut ProviderPickerView, provider: ApiProvider) {
    // The target may be hidden by the default configured-only view
    // (#3830); switch to the full catalog so navigation can still reach
    // it, matching what a user pressing `A` would do.
    if let Some(idx) = picker.rows.iter().position(|row| row.provider == provider)
        && !picker.row_visible(idx)
    {
        picker.toggle_view();
    }
    let max_steps = picker.rows.len();
    for _ in 0..max_steps {
        if picker.selected_provider() == provider {
            return;
        }
        picker.handle_key(key(KeyCode::Down));
    }
    panic!("provider {provider:?} not found in picker");
}

fn render_text(picker: &ProviderPickerView, width: u16, height: u16) -> String {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    picker.render(area, &mut buf);
    (0..height)
        .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn failed_live_catalog_refresh_names_the_working_fallback() {
    assert_eq!(
        catalog_freshness_title_suffix_for(ModelsDevFreshness::Failed),
        " · refresh failed; catalog available"
    );
}

#[test]
fn provider_picker_semantically_truncates_dense_rows_at_narrow_width() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    picker.toggle_view();

    let text = render_text(&picker, 64, 16);
    assert!(text.contains('…'), "{text}");
    for (idx, line) in text.lines().enumerate() {
        assert!(
            crate::tui::ui_text::text_display_width(line) <= 64,
            "line {idx} overflows: {line:?}"
        );
    }
}

#[test]
fn type_ahead_jumps_to_provider_by_first_letter() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    // Z.ai isn't configured, so it's hidden by the default view (#3830);
    // browse the full catalog like a user pressing `A` would.
    picker.toggle_view();
    // Search for "zai" — unique enough to match only Z.ai.
    for c in "zai".chars() {
        picker.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(picker.query, "zai");
    let filtered = picker.filtered_rows();
    assert!(!filtered.is_empty(), "search for 'zai' must match Z.ai");
    assert!(
        filtered
            .iter()
            .any(|(_, row)| row.provider == ApiProvider::Zai),
        "Z.ai must be in filtered results: {:?}",
        filtered
            .iter()
            .map(|(_, r)| &r.display_name)
            .collect::<Vec<_>>()
    );
    assert_eq!(picker.selected_provider(), ApiProvider::Zai);
}

#[test]
fn type_ahead_jumps_to_deepseek_on_previously_stolen_d() {
    // `d` used to open the DS4 custom form before type-ahead. The footer
    // advertises a-z jump; DS4 is a filled-in custom form, not a
    // catalog filter, so the letter belongs to DeepSeek and friends.
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Ollama, &config);
    picker.toggle_view();
    assert_eq!(picker.stage, Stage::List);

    // First letter previously opened the DS4 form instead of searching.
    picker.handle_key(key(KeyCode::Char('d')));
    assert_eq!(picker.query, "d");
    assert_eq!(picker.stage, Stage::List);
    assert!(
        picker
            .filtered_rows()
            .iter()
            .any(|(_, row)| row.provider == ApiProvider::Deepseek),
        "DeepSeek must be reachable by the letter DS4 used to steal"
    );

    // Host is unique; model ids like deepseek-v4-flash also live on other
    // rows, so a name-only query is not enough to land on DeepSeek.
    for c in "eepseek.com".chars() {
        picker.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(picker.query, "deepseek.com");
    assert_eq!(picker.stage, Stage::List);
    assert_eq!(picker.selected_provider(), ApiProvider::Deepseek);
}

#[test]
fn type_ahead_i_does_not_open_lm_studio() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new_for_setup(ApiProvider::Deepseek, None, &config, None);
    assert_eq!(picker.stage, Stage::List);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('i'))),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::List);
    assert_eq!(picker.query, "i");
}

#[test]
fn list_footer_advertises_jump_not_lm_studio_or_ds4() {
    let config = Config::default();
    let picker = ProviderPickerView::new_for_setup(ApiProvider::Deepseek, None, &config, None);
    let rendered = render_text(&picker, 100, 28);
    assert!(rendered.contains("a-z"), "{rendered}");
    assert!(
        !rendered.contains("I LM Studio"),
        "LM Studio must not steal i from type-ahead: {rendered}"
    );
    assert!(
        !rendered.contains("D DS4"),
        "DS4 must not steal d from type-ahead: {rendered}"
    );
}

#[test]
fn compact_base_url_strips_scheme_and_caps_length() {
    // Short URLs pass through unchanged (scheme + trailing slash stripped).
    assert_eq!(
        compact_base_url("https://api.deepseek.com/"),
        "api.deepseek.com"
    );
    assert_eq!(
        compact_base_url("http://localhost:9000/v1"),
        "localhost:9000/v1"
    );
    // A long URL is capped so it can't dominate the hint row.
    let long = compact_base_url("https://api-us-west-2.example-region.company.com/v1/openai");
    assert!(long.ends_with("..."), "expected an ellipsis, got {long:?}");
    assert!(
        long.chars().count() <= 24,
        "capped to 24 cols, got {long:?}"
    );
}

#[test]
fn mouse_scroll_moves_selection_in_list_stage() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    // Scroll across the full catalog (#3830), not just the configured
    // subset, which would only contain the active provider here.
    picker.toggle_view();
    let before = picker.selected_idx;
    picker.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_ne!(
        picker.selected_idx, before,
        "scroll down should advance the selection"
    );
}

#[test]
fn picker_lists_all_providers() {
    let config = Config::default();
    let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    let names: Vec<_> = picker
        .rows
        .iter()
        .map(|row| row.display_name.as_str())
        .collect();

    // Catalog surface: one identity per vendor (not dual-wire / plan kinds).
    assert_eq!(names.len(), ApiProvider::catalog().len());
    assert!(names.contains(&"DeepSeek"));
    assert!(names.contains(&"Alibaba Cloud Model Studio"));
    // Dialect is wire config — no second MiniMax / Model Studio rows.
    assert_eq!(
        names
            .iter()
            .filter(|name| name.contains("Alibaba Cloud Model Studio"))
            .count(),
        1
    );
    assert_eq!(names.iter().filter(|name| **name == "MiniMax").count(), 1);
    assert_eq!(names.iter().filter(|name| **name == "DeepSeek").count(), 1);

    // Providers are presented in neutral case-insensitive alphabetical
    // order by display name (#3076), not `ApiProvider::all()` order.
    let mut expected = names.clone();
    expected.sort_by_key(|name| name.to_ascii_lowercase());
    assert_eq!(
        names, expected,
        "provider picker must list providers in case-insensitive alphabetical order"
    );
    // DeepSeek is no longer hard-coded first.
    assert_ne!(names.first(), Some(&"DeepSeek"));
}

#[test]
fn default_view_shows_only_configured_providers() {
    // #3830: with nothing but the active provider set up, the default
    // list view excludes the unconfigured catalog noise — even though
    // `rows` (the underlying data) still has every provider, per
    // `picker_lists_all_providers` above. Doesn't assert an exact count:
    // `OpenaiCodex` reads a real OAuth file from disk in
    // `has_api_key_for`, so it's legitimately "configured" on a machine
    // with a prior Codex login and must not make this test host-dependent.
    let config = Config::default();
    let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

    assert_eq!(picker.view, ProviderListView::Configured);
    let visible: Vec<ApiProvider> = picker
        .filtered_rows()
        .iter()
        .map(|(_, row)| row.provider)
        .collect();
    assert!(visible.contains(&ApiProvider::Deepseek), "{visible:?}");
    assert!(
        !visible.contains(&ApiProvider::Custom),
        "the unused custom-provider placeholder slot isn't \"configured\": {visible:?}"
    );
    for unconfigured in [
        ApiProvider::Zai,
        ApiProvider::Openrouter,
        ApiProvider::Novita,
        ApiProvider::Ollama,
    ] {
        assert!(
            !visible.contains(&unconfigured),
            "{unconfigured:?} has no credentials and isn't active: {visible:?}"
        );
    }
    assert!(
        picker.rows.len() > visible.len(),
        "underlying data keeps every provider"
    );
}

#[test]
fn explicit_provider_config_marks_provider_configured_without_active_or_key() {
    // #3830: a non-default `[providers.<name>]` entry (here just a base
    // URL override, no key) counts as "configured" even though the
    // provider is neither active nor has working credentials.
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            openrouter: crate::config::ProviderConfig {
                base_url: Some("https://custom.openrouter.example/v1".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    let row = picker
        .rows
        .iter()
        .find(|row| row.provider == ApiProvider::Openrouter)
        .expect("openrouter row");
    assert!(row.is_configured);
    assert!(!row.has_key, "explicit config doesn't imply a working key");
}

#[test]
fn empty_provider_headers_do_not_mark_provider_configured() {
    let _env = crate::test_support::lock_test_env();
    let _anthropic_key = crate::test_support::EnvVarGuard::remove("ANTHROPIC_API_KEY");
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            anthropic: crate::config::ProviderConfig {
                http_headers: Some(std::collections::HashMap::new()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    let anthropic = picker
        .rows
        .iter()
        .find(|row| row.provider == ApiProvider::Anthropic)
        .expect("anthropic row");

    assert!(
        !anthropic.is_configured,
        "an empty deserialized header table is default state, not setup"
    );
}

#[test]
fn non_empty_provider_headers_mark_provider_configured() {
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            anthropic: crate::config::ProviderConfig {
                http_headers: Some(std::collections::HashMap::from([(
                    "X-Route".to_string(),
                    "custom".to_string(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    let anthropic = picker
        .rows
        .iter()
        .find(|row| row.provider == ApiProvider::Anthropic)
        .expect("anthropic row");

    assert!(
        anthropic.is_configured,
        "a user-authored header is meaningful explicit provider setup"
    );
}

#[test]
fn blank_provider_header_entries_do_not_mark_provider_configured() {
    let _env = crate::test_support::lock_test_env();
    let _anthropic_key = crate::test_support::EnvVarGuard::remove("ANTHROPIC_API_KEY");
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            anthropic: crate::config::ProviderConfig {
                http_headers: Some(std::collections::HashMap::from([
                    (" ".to_string(), "value".to_string()),
                    ("X-Blank".to_string(), "   ".to_string()),
                ])),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    assert!(!crate::config::provider_is_configured_for_active(
        &config,
        ApiProvider::Anthropic,
        ApiProvider::Deepseek,
    ));
}

#[test]
fn self_hosted_provider_not_auto_configured_without_explicit_setup() {
    // #3830: `has_api_key_for` always reports `true` for self-hosted
    // providers (no auth required to route to them) — that must not, on
    // its own, make Ollama/Sglang/Vllm show up in the default
    // configured-only view for every user regardless of setup.
    let config = Config::default();
    let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    let ollama = picker
        .rows
        .iter()
        .find(|row| row.provider == ApiProvider::Ollama)
        .expect("ollama row");
    assert!(
        ollama.has_key,
        "self-hosted providers report has_key unconditionally"
    );
    assert!(
        !ollama.is_configured,
        "but that alone must not mark them configured"
    );

    // Active self-hosted provider still counts as configured.
    let active_picker = ProviderPickerView::new(ApiProvider::Ollama, &config);
    let active_ollama = active_picker
        .rows
        .iter()
        .find(|row| row.provider == ApiProvider::Ollama)
        .expect("ollama row");
    assert!(active_ollama.is_configured);
}

#[test]
fn toggle_view_reveals_full_catalog_and_back() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    let configured_count = picker.filtered_rows().len();
    assert_eq!(picker.view, ProviderListView::Configured);

    let action = picker.handle_key(key(KeyCode::Char('a')));
    assert!(matches!(action, ViewAction::None));
    assert_eq!(picker.view, ProviderListView::Catalog);
    assert_eq!(picker.filtered_rows().len(), picker.rows.len());
    assert!(picker.filtered_rows().len() > configured_count);

    picker.handle_key(key(KeyCode::Char('A')));
    assert_eq!(picker.view, ProviderListView::Configured);
    assert_eq!(picker.filtered_rows().len(), configured_count);
}

#[test]
fn key_entry_hint_uses_metadata_env_vars() {
    assert_eq!(
        ProviderPickerView::env_var_for(ApiProvider::NvidiaNim),
        "NVIDIA_API_KEY / NVIDIA_NIM_API_KEY"
    );
}

#[test]
fn key_entry_hint_includes_provider_credential_url() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::NvidiaNim);
    picker.handle_key(key(KeyCode::Enter));

    let rendered = render_text(&picker, 120, 20);

    assert!(rendered.contains("NVIDIA_API_KEY / NVIDIA_NIM_API_KEY"));
    assert!(rendered.contains("https://build.nvidia.com/settings/api-keys"));
}

#[test]
fn zai_key_entry_wraps_long_environment_guidance_without_hiding_credentials_url() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Zai);
    picker.handle_key(key(KeyCode::Enter));

    // Reproduce the width from the dogfood screenshot: the old renderer
    // allocated one row per logical line, so the long env-var sentence
    // clipped and displaced the credentials URL.
    let rendered = render_text(&picker, 100, 20);

    for name in [
        "ZAI_API_KEY",
        "Z_AI_API_KEY",
        "ZHIPU_API_KEY",
        "GLM_API_KEY",
    ] {
        assert!(rendered.contains(name), "missing {name}:\n{rendered}");
    }
    assert!(rendered.contains("re-open /provider."), "{rendered}");
    assert!(
        rendered.contains("Credentials: https://z.ai/model-api"),
        "{rendered}"
    );
}

#[test]
fn kimi_key_entry_uses_the_direct_api_key_console_without_oauth_copy() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Moonshot);
    picker.handle_key(key(KeyCode::Enter));

    let rendered = render_text(&picker, 120, 20);

    assert!(rendered.contains("https://platform.kimi.ai/console/api-keys"));
    assert!(rendered.contains("paste key here"));
    assert!(!rendered.contains("OAuth"));
    assert!(!rendered.contains("device login"));
}

#[test]
fn kimi_code_plan_key_entry_uses_membership_route_guidance() {
    let config = Config {
        provider: Some("moonshot".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            moonshot: crate::config::ProviderConfig {
                base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut picker = ProviderPickerView::new(ApiProvider::Moonshot, &config);
    assert_eq!(picker.selected_provider(), ApiProvider::Moonshot);
    picker.handle_key(key(KeyCode::Enter));

    let rendered = render_text(&picker, 120, 24);

    assert!(rendered.contains("https://www.kimi.com/code/console"));
    assert!(rendered.contains("api.kimi.com/coding/v1"));
    assert!(rendered.contains("does not import Kimi CLI credentials"));
    assert!(!rendered.contains("https://platform.kimi.ai/console/api-keys"));
    assert!(!rendered.contains("OAuth"));
}

#[test]
fn recovery_picker_keeps_active_route_and_esc_makes_no_change() {
    let config = Config {
        provider: Some("moonshot".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            moonshot: crate::config::ProviderConfig {
                base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut picker = ProviderPickerView::new(ApiProvider::Moonshot, &config);

    assert_eq!(picker.stage, Stage::List);
    assert_eq!(picker.selected_provider(), ApiProvider::Moonshot);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Esc)),
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerDismissed { .. })
    ));
    assert_eq!(config.provider.as_deref(), Some("moonshot"));
    assert_eq!(
        config
            .provider_config_for(ApiProvider::Moonshot)
            .and_then(|entry| entry.base_url.as_deref()),
        Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL)
    );
}

#[test]
fn recovery_model_pick_restores_exact_kimi_code_k3_without_catalog_leakage() {
    let mut config = Config {
        provider: Some("moonshot".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            moonshot: crate::config::ProviderConfig {
                base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let picker = ProviderPickerView::new_for_model_pick_after_validation(
        ApiProvider::Moonshot,
        ApiProvider::Moonshot,
        &config,
        None,
        "validated-key".to_string(),
        None,
    )
    .expect("Kimi route row");

    assert_eq!(picker.selected_model.as_deref(), Some("k3"));
    assert_eq!(
        picker
            .model_options
            .iter()
            .filter(|model| model.eq_ignore_ascii_case("k3"))
            .count(),
        1,
        "the current wire model must be appended once, case-insensitively"
    );

    config
        .providers
        .as_mut()
        .expect("providers")
        .moonshot
        .base_url = Some(crate::config::DEFAULT_MOONSHOT_BASE_URL.to_string());
    let generic = ProviderPickerView::new_for_model_pick_after_validation(
        ApiProvider::Moonshot,
        ApiProvider::Moonshot,
        &config,
        None,
        "validated-key".to_string(),
        None,
    )
    .expect("generic Moonshot row");
    assert!(
        !generic
            .model_options
            .iter()
            .any(|model| model.eq_ignore_ascii_case("k3")),
        "bare K3 stays route-local and must not be added to generic Moonshot"
    );
}

#[test]
fn setup_provider_key_entry_matrix_keeps_hosted_codex_and_local_hints_distinct() {
    let _guard = crate::test_support::lock_test_env();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let codewhale_home = tmp.path().join(".codewhale");
    let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
    let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
    let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
    let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
    let _codex_key = crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
    let _codex_legacy_key = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
    let config = Config::default();

    let hosted = ProviderPickerView::new_for_setup(
        ApiProvider::Openai,
        Some(ApiProvider::Deepseek),
        &config,
        None,
    );
    assert_eq!(hosted.stage, Stage::KeyEntry);
    assert_eq!(hosted.selected_provider(), ApiProvider::Deepseek);
    let hosted_text = render_text(&hosted, 120, 20);
    assert!(hosted_text.contains("DEEPSEEK_API_KEY"), "{hosted_text}");
    assert!(
        hosted_text.contains("Credentials: https://platform.deepseek.com/api_keys"),
        "{hosted_text}"
    );
    assert!(!hosted_text.contains("OAuth login"), "{hosted_text}");

    let codex = ProviderPickerView::new_for_setup(
        ApiProvider::Deepseek,
        Some(ApiProvider::OpenaiCodex),
        &config,
        None,
    );
    assert_eq!(codex.stage, Stage::KeyEntry);
    assert_eq!(codex.selected_provider(), ApiProvider::OpenaiCodex);
    let codex_text = render_text(&codex, 120, 20);
    assert!(codex_text.contains("OAuth login"), "{codex_text}");
    assert!(
        codex_text.contains("OPENAI_CODEX_ACCESS_TOKEN"),
        "{codex_text}"
    );
    assert!(codex_text.contains("external-consent"), "{codex_text}");
    assert!(!codex_text.contains("Credentials:"), "{codex_text}");
    assert!(!codex_text.contains("(paste key here)"), "{codex_text}");

    let local = ProviderPickerView::new_for_setup(
        ApiProvider::Deepseek,
        Some(ApiProvider::Ollama),
        &config,
        None,
    );
    assert_eq!(local.stage, Stage::List);
    assert_eq!(local.selected_provider(), ApiProvider::Ollama);
    let local_text = render_text(&local, 120, 20);
    assert!(!local_text.contains("Credentials:"), "{local_text}");

    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "my_thing".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("https://api.example.com/v1".to_string()),
            model: Some("vendor/custom-model-v1".to_string()),
            api_key_env: Some("EXAMPLE_API_KEY".to_string()),
            ..Default::default()
        },
    );
    let _custom_key = crate::test_support::EnvVarGuard::remove("EXAMPLE_API_KEY");
    let custom_config = Config {
        provider: Some("my_thing".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Config::default()
    };
    let custom_picker =
        ProviderPickerView::new_for_setup(ApiProvider::Custom, None, &custom_config, None);
    let custom_row = &custom_picker.rows[custom_picker.selected_idx];
    assert_eq!(custom_row.provider, ApiProvider::Custom);
    assert_eq!(custom_row.provider_id, "my_thing");
    assert!(
        custom_row
            .messages
            .iter()
            .any(|message| message.contains("EXAMPLE_API_KEY")),
        "custom setup row should name its configured auth env var: {:?}",
        custom_row.messages
    );
    let custom_text = render_text(&custom_picker, 120, 20);
    assert!(custom_text.contains("my_thing"), "{custom_text}");
    assert!(custom_text.contains("EXAMPLE_API_KEY"), "{custom_text}");
    assert!(!custom_text.contains("Credentials:"), "{custom_text}");
}

#[test]
fn provider_dashboard_row_models_local_readiness_without_rendering() {
    let config = Config::default();
    let row = ProviderDashboardRow::from_config(ApiProvider::Ollama, ApiProvider::Ollama, &config);

    assert_eq!(row.provider_id, "ollama");
    assert_eq!(row.auth_status, ProviderAuthStatus::Local);
    assert_eq!(row.readiness, ResolvedProviderReadiness::LocalUnchecked);
    assert_eq!(row.supported_protocols, vec!["chat".to_string()]);
    assert_eq!(row.usage_meter, "cost: local");
    assert!(row.base_url.contains("localhost:11434"));
    assert!(row.is_active);
}

#[test]
fn ollama_cloud_row_requires_credentials_and_is_not_labeled_local() {
    let _env_lock = crate::test_support::lock_test_env();
    let temp = tempfile::tempdir().expect("isolated credential home");
    let _home = EnvVarGuard::set("CODEWHALE_HOME", temp.path());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _ollama_cloud_key = EnvVarGuard::remove("OLLAMA_CLOUD_API_KEY");
    let _ollama_key = EnvVarGuard::remove("OLLAMA_API_KEY");
    let _cli_source = EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
    let _cli_key = EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");

    let mut config = Config {
        provider: Some("ollama".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            ollama: crate::config::ProviderConfig {
                base_url: Some(codewhale_config::provider::OLLAMA_CLOUD_BASE_URL.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    assert_eq!(config.api_provider(), ApiProvider::OllamaCloud);
    let missing = ProviderDashboardRow::from_config(
        ApiProvider::OllamaCloud,
        ApiProvider::OllamaCloud,
        &config,
    );
    assert_eq!(missing.auth_status, ProviderAuthStatus::Missing);
    assert_eq!(missing.readiness, ResolvedProviderReadiness::MissingKey);
    assert_eq!(missing.usage_meter, "cost: unknown");
    assert!(!missing.compact_hint().contains("(self-hosted)"));
    assert!(
        missing
            .messages
            .iter()
            .any(|message| message.contains("OLLAMA_API_KEY")),
        "missing Cloud key guidance: {:?}",
        missing.messages
    );

    config.providers.as_mut().expect("providers").ollama.api_key =
        Some("ollama-cloud-key".to_string());
    let configured = ProviderDashboardRow::from_config(
        ApiProvider::OllamaCloud,
        ApiProvider::OllamaCloud,
        &config,
    );
    assert_eq!(configured.auth_status, ProviderAuthStatus::Configured);
    assert_eq!(
        configured.readiness,
        ResolvedProviderReadiness::SavedUnchecked
    );
    assert!(!configured.compact_hint().contains("(self-hosted)"));
}

#[test]
fn deepseek_cn_row_uses_shared_readiness_and_strict_model_validation() {
    let _lock = crate::test_support::lock_test_env();
    let _key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
    let missing = Config {
        provider: Some("deepseek-cn".to_string()),
        ..Default::default()
    };
    let missing_row = ProviderDashboardRow::from_config(
        ApiProvider::DeepseekCN,
        ApiProvider::DeepseekCN,
        &missing,
    );
    assert_eq!(missing_row.readiness, ResolvedProviderReadiness::MissingKey);
    assert_ne!(missing_row.auth_status, ProviderAuthStatus::Legacy);

    let configured = Config {
        provider: Some("deepseek-cn".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            deepseek_cn: crate::config::ProviderConfig {
                api_key: Some("deepseek-cn-test-key".to_string()),
                model: Some("deepseek-v4-pro".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let configured_row = ProviderDashboardRow::from_config(
        ApiProvider::DeepseekCN,
        ApiProvider::DeepseekCN,
        &configured,
    );
    assert_eq!(
        configured_row.readiness,
        ResolvedProviderReadiness::SavedUnchecked
    );

    let mut invalid = configured;
    invalid
        .providers
        .as_mut()
        .expect("providers")
        .deepseek_cn
        .model = Some("anthropic/claude-foreign".to_string());
    let invalid_row = ProviderDashboardRow::from_config(
        ApiProvider::DeepseekCN,
        ApiProvider::DeepseekCN,
        &invalid,
    );
    assert_eq!(
        invalid_row.readiness,
        ResolvedProviderReadiness::InvalidRoute
    );
}

#[test]
fn provider_health_requires_observed_success_and_keeps_failure_reason() {
    let config = Config {
        api_key: Some("saved-key".to_string()),
        ..Config::default()
    };
    let unchecked = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    let row = unchecked
        .rows
        .iter()
        .find(|row| row.provider == ApiProvider::Deepseek)
        .expect("DeepSeek row");
    assert_eq!(row.readiness, ResolvedProviderReadiness::SavedUnchecked);

    let mut health = ProviderReadinessSnapshot::default();
    health.record_success(&config, ApiProvider::Deepseek, "deepseek-v4-pro");
    let ready =
        ProviderPickerView::new(ApiProvider::Deepseek, &config).with_provider_health(&health);
    assert_eq!(
        ready
            .rows
            .iter()
            .find(|row| row.provider == ApiProvider::Deepseek)
            .unwrap()
            .readiness,
        ResolvedProviderReadiness::Ready
    );

    health.record_failure_message(
        &config,
        ApiProvider::Deepseek,
        "deepseek-v4-pro",
        crate::error_taxonomy::ErrorCategory::Authentication,
        "credential rejected",
    );
    let failed =
        ProviderPickerView::new(ApiProvider::Deepseek, &config).with_provider_health(&health);
    let row = failed
        .rows
        .iter()
        .find(|row| row.provider == ApiProvider::Deepseek)
        .unwrap();
    assert!(row.readiness.label().contains("last check failed"));
    assert!(
        row.messages
            .iter()
            .any(|message| message == "credential rejected")
    );
}

#[test]
fn openai_codex_row_is_experimental_and_tagged_in_hint() {
    let config = Config::default();
    let row =
        ProviderDashboardRow::from_config(ApiProvider::OpenaiCodex, ApiProvider::Deepseek, &config);

    // #2984: maturity is a separate axis from auth/readiness.
    assert_eq!(row.maturity, ProviderMaturity::Experimental);
    assert!(
        row.compact_hint().contains("experimental"),
        "experimental maturity must surface in the hint, got {:?}",
        row.compact_hint()
    );
}

#[test]
fn mainstream_provider_is_supported_without_experimental_tag() {
    let config = Config::default();
    let row =
        ProviderDashboardRow::from_config(ApiProvider::Deepseek, ApiProvider::Deepseek, &config);

    // #2984: supported integrations stay noise-free (no tag).
    assert_eq!(row.maturity, ProviderMaturity::Supported);
    assert!(
        !row.compact_hint().contains("experimental"),
        "supported providers must omit the experimental tag, got {:?}",
        row.compact_hint()
    );
}

#[test]
fn provider_dashboard_row_surfaces_glm_reasoning_controls() {
    let config = Config {
        reasoning_effort: Some("max".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            zai: crate::config::ProviderConfig {
                api_key: Some("zai-key".to_string()),
                model: Some("GLM-5.2".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let row = ProviderDashboardRow::from_config(ApiProvider::Zai, ApiProvider::Zai, &config);

    assert_eq!(row.default_route.wire_model, "GLM-5.2");
    assert_eq!(row.reasoning.support, ProviderReasoningSupport::Supported);
    assert_eq!(
        row.reasoning.controls,
        vec!["high".to_string(), "max".to_string()]
    );
    assert_eq!(
        row.reasoning.stream_visibility,
        ProviderReasoningStreamVisibility::StructuredThinking
    );
    assert_eq!(row.reasoning.selected_control.as_deref(), Some("max"));
    assert!(row.compact_hint().contains("reasoning:high/max"));
    assert!(row.compact_hint().contains("stream:structured"));
}

#[test]
fn provider_dashboard_row_surfaces_modelstudio_structured_thinking() {
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            modelstudio_token_plan: crate::config::ProviderConfig {
                api_key: Some("modelstudio-key".to_string()),
                model: Some("qwen3.8-max".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let row = ProviderDashboardRow::from_config(
        ApiProvider::ModelstudioTokenPlan,
        ApiProvider::ModelstudioTokenPlan,
        &config,
    );

    assert_eq!(row.reasoning.support, ProviderReasoningSupport::Supported);
    assert_eq!(
        row.reasoning.stream_visibility,
        ProviderReasoningStreamVisibility::StructuredThinking
    );
    assert!(row.compact_hint().contains("stream:structured"));
}

#[test]
fn provider_dashboard_row_surfaces_kimi_code_k3_reasoning_only_on_exact_route() {
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            moonshot: crate::config::ProviderConfig {
                api_key: Some("kimi-code-key".to_string()),
                base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let row =
        ProviderDashboardRow::from_config(ApiProvider::Moonshot, ApiProvider::Moonshot, &config);

    assert_eq!(
        row.default_route.wire_model,
        crate::config::KIMI_CODE_K3_MODEL
    );
    assert_eq!(row.reasoning.support, ProviderReasoningSupport::Supported);
    assert_eq!(
        row.reasoning.stream_visibility,
        ProviderReasoningStreamVisibility::StructuredThinking
    );
    assert_eq!(
        row.reasoning.controls,
        vec!["low".to_string(), "high".to_string(), "max".to_string()]
    );
    assert_eq!(
        row.capabilities.context_window,
        Some(262_144),
        "the picker must show the route-effective K3 baseline, not the generic fallback"
    );
    assert_eq!(
        row.capabilities.context_window_source.as_deref(),
        Some("static Kimi Code safe floor"),
        "the picker must name the provenance instead of presenting a bare limit as provider fact"
    );
    assert!(
        row.compact_hint()
            .contains("ctx:262K(static Kimi Code safe floor)"),
        "the compact picker receipt must retain context provenance"
    );

    let mut direct = config.clone();
    direct
        .providers
        .as_mut()
        .expect("providers")
        .moonshot
        .base_url = Some(crate::config::DEFAULT_MOONSHOT_BASE_URL.to_string());
    let direct_row =
        ProviderDashboardRow::from_config(ApiProvider::Moonshot, ApiProvider::Moonshot, &direct);
    assert_ne!(
        direct_row.reasoning.support,
        ProviderReasoningSupport::Supported,
        "generic Moonshot k3 must not inherit Kimi Code's route-owned capability"
    );
    // The generic model-facts table now carries the same conservative
    // number for bare `k3`, so the route-ownership distinction lives in
    // provenance: the direct Moonshot row must never claim the Kimi Code
    // route-owned floor as its source.
    assert_ne!(
        direct_row.capabilities.context_window_source.as_deref(),
        Some("static Kimi Code safe floor")
    );
}

#[test]
fn provider_row_query_matches_default_route_model_and_wire_id() {
    // #4141: cross-field search must also match the default route's display
    // model name and wire model id, keeping this picker consistent with the
    // model picker (`model_row_matches_query`). Z.ai's provider key,
    // display name, kind, and base URL contain no "glm", so a "glm" match
    // can only come from the route's model/wire fields.
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            zai: crate::config::ProviderConfig {
                api_key: Some("zai-key".to_string()),
                model: Some("GLM-5.2".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let row = ProviderDashboardRow::from_config(ApiProvider::Zai, ApiProvider::Zai, &config);
    assert_eq!(row.default_route.wire_model, "GLM-5.2");

    // Wire model id + display model name, case-insensitively.
    assert!(row.matches_query("glm-5.2"));
    assert!(row.matches_query("GLM"));
    // Provider name still matches, and an unrelated token still does not.
    assert!(row.matches_query("zhipu"));
    assert!(!row.matches_query("anthropic"));
}

#[test]
fn provider_dashboard_row_surfaces_zai_concurrency_cap() {
    let config = Config::default();
    let row = ProviderDashboardRow::from_config(ApiProvider::Zai, ApiProvider::Deepseek, &config);

    assert_eq!(
        row.request_concurrency.limit,
        Some(crate::config::DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY)
    );
    assert_eq!(row.request_concurrency.active, None);
    assert!(
        row.compact_hint().contains("req:cap 3"),
        "Z.ai's effective default cap must surface in /provider, got {:?}",
        row.compact_hint()
    );
}

#[test]
fn provider_dashboard_row_surfaces_active_provider_requests() {
    let config = Config::default();
    let runtime_status = ProviderRuntimeStatus {
        provider: ApiProvider::Zai,
        request_concurrency_limit: Some(crate::config::DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY),
        active_provider_requests: 2,
    };
    let mut picker = ProviderPickerView::new_with_runtime_status(
        ApiProvider::Zai,
        &config,
        Some(runtime_status),
    );

    move_to_provider(&mut picker, ApiProvider::Zai);
    let row = &picker.rows[picker.selected_idx];

    assert_eq!(
        row.request_concurrency.limit,
        Some(crate::config::DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY)
    );
    assert_eq!(row.request_concurrency.active, Some(2));
    assert!(
        row.compact_hint().contains("req:2/3"),
        "active runtime concurrency must surface in /provider, got {:?}",
        row.compact_hint()
    );
}

#[test]
fn provider_dashboard_row_surfaces_codex_reasoning_scale() {
    let config = Config {
        reasoning_effort: Some("max".to_string()),
        ..Config::default()
    };
    let row = ProviderDashboardRow::from_config(
        ApiProvider::OpenaiCodex,
        ApiProvider::OpenaiCodex,
        &config,
    );

    assert_eq!(row.reasoning.support, ProviderReasoningSupport::Supported);
    assert_eq!(
        row.reasoning.controls,
        vec![
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
            "xhigh".to_string(),
        ]
    );
    assert_eq!(
        row.reasoning.stream_visibility,
        ProviderReasoningStreamVisibility::StructuredThinking
    );
    assert_eq!(row.reasoning.selected_control.as_deref(), Some("xhigh"));
    assert!(
        row.compact_hint()
            .contains("reasoning:low/medium/high/xhigh")
    );
}

#[test]
fn provider_dashboard_row_surfaces_capability_and_metadata_badges() {
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            deepseek: crate::config::ProviderConfig {
                api_key: Some("deepseek-key".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let row =
        ProviderDashboardRow::from_config(ApiProvider::Deepseek, ApiProvider::Deepseek, &config);

    // Metadata badges are projected from the resolved capability profile,
    // never hardcoded per UI surface.
    assert!(row.capabilities.context_window.is_some());
    assert!(row.capabilities.max_output.is_some());
    let hint = row.compact_hint();
    assert!(hint.contains("ctx:"), "metadata badge missing: {hint}");
    assert!(hint.contains("out:"), "metadata badge missing: {hint}");
    // Capability cluster present (tri-state; unknown renders `?`, never
    // silently omitted).
    for badge in ["tools:", "json:", "stream:", "cache:"] {
        assert!(
            hint.contains(badge),
            "capability badge {badge} missing: {hint}"
        );
    }
}

#[test]
fn provider_dashboard_row_classifies_model_origin() {
    // Default: no configured model override.
    let config = Config::default();
    let row =
        ProviderDashboardRow::from_config(ApiProvider::Deepseek, ApiProvider::Deepseek, &config);
    assert_eq!(row.model_origin, ProviderModelOrigin::Default);
    assert!(row.compact_hint().contains("origin:default"));

    // Saved: a configured model override for the provider.
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            deepseek: crate::config::ProviderConfig {
                api_key: Some("k".to_string()),
                model: Some("deepseek-v4-flash".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let row =
        ProviderDashboardRow::from_config(ApiProvider::Deepseek, ApiProvider::Deepseek, &config);
    assert_eq!(row.model_origin, ProviderModelOrigin::Saved);
    assert!(row.compact_hint().contains("origin:saved"));
}

#[test]
fn model_origin_classifier_covers_default_saved_custom() {
    assert_eq!(
        ProviderModelOrigin::for_provider(ApiProvider::Deepseek, false),
        ProviderModelOrigin::Default
    );
    assert_eq!(
        ProviderModelOrigin::for_provider(ApiProvider::Deepseek, true),
        ProviderModelOrigin::Saved
    );
    assert_eq!(
        ProviderModelOrigin::for_provider(ApiProvider::Custom, false),
        ProviderModelOrigin::Custom
    );
    // An explicit saved model still wins for a custom provider.
    assert_eq!(
        ProviderModelOrigin::for_provider(ApiProvider::Custom, true),
        ProviderModelOrigin::Saved
    );
}

#[test]
fn self_hosted_provider_row_marks_self_hosted_in_hint() {
    let _env_lock = crate::test_support::lock_test_env();
    let _sglang_key = crate::test_support::EnvVarGuard::remove("SGLANG_API_KEY");
    let _sglang_base_url = crate::test_support::EnvVarGuard::remove("SGLANG_BASE_URL");
    let _vllm_key = crate::test_support::EnvVarGuard::remove("VLLM_API_KEY");
    let _vllm_base_url = crate::test_support::EnvVarGuard::remove("VLLM_BASE_URL");
    let _ollama_key = crate::test_support::EnvVarGuard::remove("OLLAMA_API_KEY");
    let _ollama_base_url = crate::test_support::EnvVarGuard::remove("OLLAMA_BASE_URL");

    let config = Config::default();
    let row = ProviderDashboardRow::from_config(ApiProvider::Ollama, ApiProvider::Ollama, &config);
    assert_eq!(row.auth_status, ProviderAuthStatus::Local);
    assert!(
        row.compact_hint().contains("(self-hosted)"),
        "self-hosted hint missing: {}",
        row.compact_hint()
    );

    let sglang =
        ProviderDashboardRow::from_config(ApiProvider::Sglang, ApiProvider::Sglang, &config);
    assert_eq!(sglang.auth_status, ProviderAuthStatus::Optional);
    assert!(
        sglang.compact_hint().contains("(self-hosted)"),
        "self-hosted hint missing for SGLang: {}",
        sglang.compact_hint()
    );
}

#[test]
fn protected_self_hosted_row_requires_its_configured_auth_mode() {
    let _env_lock = crate::test_support::lock_test_env();
    let temp = tempfile::tempdir().expect("isolated credential home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path());
    let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _vllm_key = crate::test_support::EnvVarGuard::remove("VLLM_API_KEY");
    let _vllm_base_url = crate::test_support::EnvVarGuard::remove("VLLM_BASE_URL");
    let config = Config {
        provider: Some("vllm".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            vllm: crate::config::ProviderConfig {
                auth_mode: Some("api_key".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };

    let row = ProviderDashboardRow::from_config(ApiProvider::Vllm, ApiProvider::Vllm, &config);

    assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
    assert_eq!(row.credential_state, CredentialState::MissingKey);
    assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
    assert!(row.compact_hint().contains("(self-hosted)"));
}

#[test]
fn self_hosted_reasoning_visibility_covers_vllm() {
    assert_eq!(
        default_reasoning_stream_visibility(ApiProvider::Sglang),
        ProviderReasoningStreamVisibility::StructuredThinking
    );
    assert_eq!(
        default_reasoning_stream_visibility(ApiProvider::Vllm),
        ProviderReasoningStreamVisibility::StructuredThinking
    );
}

#[test]
fn humanize_token_count_is_compact_and_marks_unknown() {
    assert_eq!(humanize_token_count(None), "?");
    assert_eq!(humanize_token_count(Some(1_000_000)), "1M");
    assert_eq!(humanize_token_count(Some(1_500_000)), "1.5M");
    assert_eq!(humanize_token_count(Some(131_072)), "131K");
    assert_eq!(humanize_token_count(Some(512)), "512");
}

#[test]
fn provider_dashboard_row_uses_route_resolver_for_custom_openai_endpoint() {
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            openai: crate::config::ProviderConfig {
                api_key: Some("openai-key".to_string()),
                base_url: Some("http://localhost:9000/v1".to_string()),
                model: Some("custom-model".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let row = ProviderDashboardRow::from_config(ApiProvider::Openai, ApiProvider::Openai, &config);

    assert_eq!(row.provider_id, "openai");
    assert_eq!(row.auth_status, ProviderAuthStatus::Configured);
    assert_eq!(row.readiness, ResolvedProviderReadiness::SavedUnchecked);
    assert_eq!(row.base_url, "http://localhost:9000/v1");
    assert_eq!(row.default_route.logical_model, "custom-model");
    assert_eq!(row.default_route.wire_model, "custom-model");
    assert_eq!(row.supported_protocols, vec!["chat".to_string()]);
}

#[test]
fn custom_endpoint_cannot_claim_official_xai_oauth_readiness() {
    let _lock = crate::test_support::lock_test_env();
    let temp = tempfile::tempdir().expect("isolated oauth home");
    let _xai_key = EnvVarGuard::remove("XAI_API_KEY");
    let missing_grok_auth = temp.path().join("missing.json");
    let _grok_auth = EnvVarGuard::set(
        "GROK_AUTH_PATH",
        missing_grok_auth.to_str().expect("utf8 test path"),
    );
    let config = Config {
        provider: Some("xai".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            xai: crate::config::ProviderConfig {
                base_url: Some("https://gateway.example.test/v1".to_string()),
                model: Some("private-grok".to_string()),
                auth_mode: Some("oauth".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };

    let row = ProviderDashboardRow::from_config(ApiProvider::Xai, ApiProvider::Xai, &config);

    assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
    assert_eq!(row.credential_state, CredentialState::MissingKey);
    assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
    assert!(!row.compact_hint().contains("oauth"));
}

#[test]
fn explicit_no_auth_custom_row_is_distinct_and_usable() {
    let custom = std::collections::HashMap::from([(
        "no-auth-gateway".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("https://gateway.example.test/v1".to_string()),
            model: Some("private-model".to_string()),
            auth_mode: Some("no-auth".to_string()),
            ..Default::default()
        },
    )]);
    let config = Config {
        provider: Some("no-auth-gateway".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Config::default()
    };

    let picker = ProviderPickerView::new(ApiProvider::Custom, &config);
    let row = picker
        .rows
        .iter()
        .find(|row| row.provider_id == "no-auth-gateway")
        .expect("configured no-auth row");

    assert_eq!(row.auth_status, ProviderAuthStatus::NoAuth);
    assert_eq!(row.credential_state, CredentialState::NoAuth);
    assert_eq!(row.readiness, ResolvedProviderReadiness::NoAuthUnchecked);
    assert!(picker.selected_has_key());
    assert!(row.compact_hint().contains("auth:none"));
}

#[test]
fn unresolved_custom_auth_metadata_does_not_mark_picker_row_configured() {
    let custom = std::collections::HashMap::from([(
        "metadata-only".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("https://gateway.example.test/v1".to_string()),
            model: Some("private-model".to_string()),
            auth: Some(codewhale_config::ProviderAuthSourceToml {
                source: codewhale_config::AuthSourceKind::Command,
                command: vec!["secret-tool".to_string(), "lookup".to_string()],
                timeout_ms: Some(2_000),
                secret_id: None,
            }),
            ..Default::default()
        },
    )]);
    let config = Config {
        provider: Some("metadata-only".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Config::default()
    };

    let picker = ProviderPickerView::new(ApiProvider::Custom, &config);
    let row = picker
        .rows
        .iter()
        .find(|row| row.provider_id == "metadata-only")
        .expect("metadata-only row remains visible for repair");

    assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
    assert_eq!(row.credential_state, CredentialState::MissingKey);
    assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
}

#[test]
fn provider_picker_lists_configured_custom_provider_readiness() {
    let _lock = crate::test_support::lock_test_env();
    let _example_key = EnvVarGuard::remove("EXAMPLE_API_KEY");
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "my_thing".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("https://api.example.com/v1".to_string()),
            model: Some("vendor/custom-model-v1".to_string()),
            api_key: Some(crate::config::API_KEYRING_SENTINEL.to_string()),
            api_key_env: Some("EXAMPLE_API_KEY".to_string()),
            ..Default::default()
        },
    );
    let config = Config {
        provider: Some("my_thing".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Config::default()
    };

    let picker = ProviderPickerView::new(ApiProvider::Custom, &config);
    let row = picker
        .rows
        .iter()
        .find(|row| row.provider_id == "my_thing")
        .expect("configured custom provider row");

    assert_eq!(row.provider, ApiProvider::Custom);
    assert_eq!(row.display_name, "my_thing (custom)");
    assert_eq!(row.kind, "openai-compatible");
    assert!(row.is_active);
    assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
    assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
    assert_eq!(row.base_url, "https://api.example.com/v1");
    assert_eq!(row.supported_protocols, vec!["chat".to_string()]);
    assert_eq!(row.default_route.logical_model, "vendor/custom-model-v1");
    assert_eq!(row.default_route.wire_model, "vendor/custom-model-v1");
    assert_eq!(row.model_origin, ProviderModelOrigin::Saved);
    assert!(
        row.messages
            .iter()
            .any(|message| message.contains("EXAMPLE_API_KEY")),
        "custom row should name the configured auth env var: {:?}",
        row.messages
    );
    assert_eq!(picker.rows[picker.selected_idx].provider_id, "my_thing");
}

#[test]
fn provider_picker_marks_only_exact_active_custom_row() {
    let custom = std::collections::HashMap::from([
        (
            "custom-a".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("http://127.0.0.1:18181/v1".to_string()),
                model: Some("model-a".to_string()),
                api_key: Some("test-key-a".to_string()),
                ..Default::default()
            },
        ),
        (
            "custom-b".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("http://127.0.0.1:18182/v1".to_string()),
                model: Some("model-b".to_string()),
                api_key: Some("test-key-b".to_string()),
                ..Default::default()
            },
        ),
    ]);
    let config = Config {
        provider: Some("custom-a".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Config::default()
    };

    let rows = custom_provider_dashboard_rows(ApiProvider::Custom, &config, None);
    let active_ids: Vec<_> = rows
        .iter()
        .filter(|row| row.is_active)
        .map(|row| row.provider_id.as_str())
        .collect();

    assert_eq!(active_ids, vec!["custom-a"]);
}

#[test]
fn provider_picker_marks_custom_provider_ready_when_env_auth_is_set() {
    let _lock = crate::test_support::lock_test_env();
    let _example_key = EnvVarGuard::set("EXAMPLE_API_KEY", "sk-test");
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "my_thing".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("https://api.example.com/v1".to_string()),
            model: Some("custom-model-v1".to_string()),
            api_key_env: Some("EXAMPLE_API_KEY".to_string()),
            ..Default::default()
        },
    );
    let config = Config {
        provider: Some("my_thing".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Config::default()
    };

    let picker = ProviderPickerView::new(ApiProvider::Custom, &config);
    let row = picker
        .rows
        .iter()
        .find(|row| row.provider_id == "my_thing")
        .expect("configured custom provider row");

    assert_eq!(row.auth_status, ProviderAuthStatus::Configured);
    assert_eq!(row.readiness, ResolvedProviderReadiness::SavedUnchecked);
    assert!(row.has_key);
    assert!(
        !row.messages
            .iter()
            .any(|message| message.contains("EXAMPLE_API_KEY")),
        "configured custom auth should not report missing env var: {:?}",
        row.messages
    );
}

#[test]
fn custom_provider_form_emits_named_provider_without_secret_value() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('c'))),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::CustomForm);
    for ch in "acme_ai".chars() {
        picker.handle_key(key(KeyCode::Char(ch)));
    }
    picker.handle_key(key(KeyCode::Enter));
    for ch in "https://api.acme.example/v1".chars() {
        picker.handle_key(key(KeyCode::Char(ch)));
    }
    picker.handle_key(key(KeyCode::Enter));
    for ch in "acme/code-1".chars() {
        picker.handle_key(key(KeyCode::Char(ch)));
    }
    picker.handle_key(key(KeyCode::Enter));
    for ch in "ACME_API_KEY".chars() {
        picker.handle_key(key(KeyCode::Char(ch)));
    }

    let action = picker.handle_key(key(KeyCode::Enter));
    match action {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerCustomProviderSubmitted {
            provider_id,
            base_url,
            model,
            api_key_env,
        }) => {
            assert_eq!(provider_id, "acme_ai");
            assert_eq!(base_url, "https://api.acme.example/v1");
            assert_eq!(model.as_deref(), Some("acme/code-1"));
            assert_eq!(api_key_env.as_deref(), Some("ACME_API_KEY"));
        }
        other => panic!("expected custom provider submit event, got {other:?}"),
    }
}

#[test]
fn sensenova_preset_fills_published_openai_host() {
    // Reached through the template list like every other compatible
    // template, not a dedicated key: SenseNova has no standing the
    // others lack, and a per-provider hotkey stole a letter from the
    // a-z jump the footer advertises.
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('p'))),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::TemplateList);
    let index = provider_setup_templates()
        .iter()
        .position(|template| template.id == codewhale_config::SENSENOVA_TEMPLATE_ID)
        .expect("SenseNova template");
    while picker.template_selected_idx < index {
        picker.handle_key(key(KeyCode::Down));
    }
    while picker.template_selected_idx > index {
        picker.handle_key(key(KeyCode::Up));
    }
    assert!(matches!(
        picker.handle_key(key(KeyCode::Enter)),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::CustomForm);
    assert_eq!(picker.custom_provider_id, "sensenova");
    assert_eq!(
        picker.custom_provider_base_url,
        codewhale_config::SENSENOVA_BASE_URL
    );
    assert_eq!(
        picker.custom_provider_model,
        codewhale_config::SENSENOVA_DEFAULT_MODEL
    );
    assert_eq!(
        picker.custom_provider_api_key_env,
        codewhale_config::SENSENOVA_API_KEY_ENV
    );
    let rendered = render_text(&picker, 100, 20);
    assert!(rendered.contains("SenseNova"), "{rendered}");
    assert!(
        !rendered.contains("Custom provider"),
        "built-in template must not look like a blank custom form: {rendered}"
    );
}

#[test]
fn p_opens_template_list_with_catalog_rows() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('p'))),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::TemplateList);
    let rendered = render_text(&picker, 100, 24);
    assert!(rendered.contains("OpenCode Zen"), "{rendered}");
    assert!(rendered.contains("OpenCode Go"), "{rendered}");
    assert!(rendered.contains("SenseNova"), "{rendered}");
    assert!(rendered.contains("Agnes"), "{rendered}");
    assert!(
        rendered.contains("no published") || rendered.contains("unpublished"),
        "{rendered}"
    );
    assert!(
        rendered.contains("https://opencode.ai/docs/zen/"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("minimax-m2.7"),
        "template detail must not dump the Zen roster: {rendered}"
    );
}

#[test]
fn template_list_enter_on_unpublished_agnes_does_not_invent_a_url() {
    let config = Config::default();
    let mut picker =
        ProviderPickerView::new_for_template_setup(ApiProvider::Deepseek, "agnes", &config, None)
            .expect("agnes template");
    assert_eq!(picker.stage, Stage::TemplateList);
    let action = picker.handle_key(key(KeyCode::Enter));
    match action {
        ViewAction::Emit(ViewEvent::StatusMessage { message }) => {
            assert!(
                message.to_ascii_lowercase().contains("no published"),
                "{message}"
            );
        }
        other => panic!("expected unpublished status, got {other:?}"),
    }
    assert!(picker.custom_provider_base_url.is_empty());
}

fn template_list_click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn template_list_mouse_selects_row_and_second_click_activates() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('p'))),
        ViewAction::None
    ));
    let area = Rect::new(0, 0, 100, 24);
    let mut buf = Buffer::empty(area);
    picker.render(area, &mut buf);
    let (rect, idx) = picker
        .template_row_hitboxes
        .borrow()
        .iter()
        .copied()
        .find(|(_, row_idx)| *row_idx == 2)
        .expect("SenseNova row hitbox");
    assert_eq!(
        provider_setup_templates()[idx].id,
        codewhale_config::SENSENOVA_TEMPLATE_ID
    );
    let click = template_list_click(rect.x, rect.y);
    assert!(matches!(picker.handle_mouse(click), ViewAction::None));
    assert_eq!(picker.template_selected_idx, idx);
    assert_eq!(picker.stage, Stage::TemplateList);
    picker.handle_mouse(click);
    assert_eq!(picker.stage, Stage::CustomForm);
    assert_eq!(picker.custom_provider_id, "sensenova");
    assert_eq!(
        picker.custom_provider_base_url,
        codewhale_config::SENSENOVA_BASE_URL
    );
}

#[test]
fn template_list_mouse_second_click_on_unpublished_does_not_invent_a_url() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('p'))),
        ViewAction::None
    ));
    let area = Rect::new(0, 0, 100, 24);
    let mut buf = Buffer::empty(area);
    picker.render(area, &mut buf);
    let (rect, idx) = picker
        .template_row_hitboxes
        .borrow()
        .iter()
        .copied()
        .find(|(_, row_idx)| provider_setup_templates()[*row_idx].is_unpublished())
        .expect("Agnes row hitbox");
    let click = template_list_click(rect.x, rect.y);
    assert!(matches!(picker.handle_mouse(click), ViewAction::None));
    assert_eq!(picker.template_selected_idx, idx);
    match picker.handle_mouse(click) {
        ViewAction::Emit(ViewEvent::StatusMessage { message }) => {
            assert!(
                message.to_ascii_lowercase().contains("no published"),
                "{message}"
            );
        }
        other => panic!("expected unpublished status, got {other:?}"),
    }
    assert!(picker.custom_provider_base_url.is_empty());
    assert_eq!(picker.stage, Stage::TemplateList);
}

#[test]
fn template_list_compact_40x12_keeps_selection_without_clipping() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('p'))),
        ViewAction::None
    ));

    for selected in [0usize, provider_setup_templates().len().saturating_sub(1)] {
        picker.template_selected_idx = selected;
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);
        let rendered = render_text(&picker, 40, 12);
        let selected_template = &provider_setup_templates()[selected];
        assert!(
            rendered.contains(selected_template.display_name),
            "40x12 must keep selected {} visible:\n{rendered}",
            selected_template.display_name
        );
        assert!(
            rendered.contains(crate::tui::glyphs::SELECTION),
            "40x12 must show the selection marker:\n{rendered}"
        );
        for (idx, line) in rendered.lines().enumerate() {
            assert!(
                crate::tui::ui_text::text_display_width(line) <= 40,
                "40x12 line {idx} clips: {line:?}\n{rendered}"
            );
        }
        let hitboxes = picker.template_row_hitboxes.borrow().clone();
        assert!(
            !hitboxes.is_empty(),
            "40x12 must register template hitboxes:\n{rendered}"
        );
        assert!(
            hitboxes.iter().any(|(_, idx)| *idx == selected),
            "40x12 hitboxes must include selected {selected}: {hitboxes:?}\n{rendered}"
        );
        for (rect, idx) in &hitboxes {
            assert!(
                rect.y < 12 && rect.x < 40,
                "hitbox for {idx} is outside 40x12: {rect:?}"
            );
            let row = (0..40)
                .map(|x| buf[(x, rect.y)].symbol())
                .collect::<String>();
            assert!(
                row.contains(provider_setup_templates()[*idx].display_name)
                    || row.contains(provider_setup_templates()[*idx].id),
                "40x12 hitbox y={} should map to {}: {row:?}",
                rect.y,
                provider_setup_templates()[*idx].display_name
            );
        }
    }
}

#[test]
fn template_list_uses_locale_for_kinds_labels_and_guidance() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config)
        .with_locale(crate::localization::Locale::ZhHans);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('p'))),
        ViewAction::None
    ));
    let rendered = render_text(&picker, 100, 24);
    // TestBackend stores the continuation cell of each wide CJK glyph
    // as a space. Collapse whitespace for language-copy assertions
    // while retaining the original cell dump for English-leak checks.
    let compact: String = rendered.chars().filter(|ch| !ch.is_whitespace()).collect();
    assert!(rendered.contains("OpenCode Zen"), "{rendered}");
    assert!(rendered.contains("SenseNova"), "{rendered}");
    assert!(rendered.contains("Agnes"), "{rendered}");
    assert!(compact.contains("仅密钥"), "{rendered}");
    assert!(compact.contains("兼容"), "{rendered}");
    assert!(
        compact.contains("基础URL"),
        "localized Base URL label missing: {rendered}"
    );
    assert!(
        compact.contains("模型：") || compact.contains("模型:"),
        "localized Model label missing: {rendered}"
    );
    assert!(
        !rendered.contains("key-only"),
        "English kind leaked: {rendered}"
    );
    assert!(
        !rendered.contains("Base URL:"),
        "English Base URL leaked: {rendered}"
    );
    assert!(
        !rendered.contains("Create or copy an OpenCode Zen API key"),
        "English guidance leaked: {rendered}"
    );
    picker.template_selected_idx = provider_setup_templates()
        .iter()
        .position(|template| template.is_unpublished())
        .expect("agnes");
    let unpublished = render_text(&picker, 100, 24);
    let unpublished_compact: String = unpublished
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    assert!(
        unpublished_compact.contains("未公布") || unpublished_compact.contains("没有"),
        "{unpublished}"
    );
    assert!(
        !unpublished.contains("unpublished"),
        "English unpublished kind leaked: {unpublished}"
    );
}

#[test]
fn t_emits_test_connection_for_the_selected_row() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    let expected_catalog = picker.view == ProviderListView::Catalog;
    let action = picker.handle_key(ctrl(KeyCode::Char('t')));
    match action {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerTestConnection {
            provider,
            provider_id,
            catalog_view,
        }) => {
            assert_eq!(provider, picker.selected_provider());
            assert_eq!(provider_id, picker.selected_provider_id());
            assert_eq!(catalog_view, expected_catalog);
        }
        other => panic!("expected test-connection event, got {other:?}"),
    }
}

#[test]
fn t_on_configured_view_does_not_force_the_full_catalog() {
    let _lock = crate::test_support::lock_test_env();
    let _key = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "sk-test");
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    assert_eq!(picker.view, ProviderListView::Configured);
    match picker.handle_key(ctrl(KeyCode::Char('t'))) {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerTestConnection {
            catalog_view, ..
        }) => {
            assert!(!catalog_view);
        }
        other => panic!("expected test-connection event, got {other:?}"),
    }
}

#[test]
fn plain_t_stays_type_ahead_and_does_not_probe() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    picker.toggle_view();
    let action = picker.handle_key(key(KeyCode::Char('t')));
    assert!(matches!(action, ViewAction::None));
    assert_eq!(picker.query, "t");
    assert_eq!(picker.stage, Stage::List);
}

#[test]
fn lm_studio_preset_is_loopback_keyless_and_requests_the_loaded_model() {
    let config = Config::default();
    let mut picker =
        ProviderPickerView::new_for_lm_studio_setup(ApiProvider::Deepseek, &config, None);

    assert_eq!(picker.stage, Stage::CustomForm);
    assert_eq!(picker.custom_provider_field, CustomProviderField::Model);
    assert_eq!(picker.custom_provider_id, "lm_studio");
    assert_eq!(picker.custom_provider_base_url, "http://127.0.0.1:1234/v1");
    assert!(picker.custom_provider_model.is_empty());
    assert!(picker.custom_provider_api_key_env.is_empty());

    for ch in "local-code-model".chars() {
        picker.handle_key(key(KeyCode::Char(ch)));
    }
    picker.handle_key(key(KeyCode::Enter));
    let action = picker.handle_key(key(KeyCode::Enter));
    match action {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerCustomProviderSubmitted {
            provider_id,
            base_url,
            model,
            api_key_env,
        }) => {
            assert_eq!(provider_id, "lm_studio");
            assert_eq!(base_url, "http://127.0.0.1:1234/v1");
            assert_eq!(model.as_deref(), Some("local-code-model"));
            assert_eq!(api_key_env, None);
        }
        other => panic!("expected LM Studio custom-provider submit event, got {other:?}"),
    }
}

#[test]
fn ds4_preset_is_keyless_and_ready_to_save() {
    let mut picker =
        ProviderPickerView::new_for_ds4_setup(ApiProvider::Deepseek, &Config::default(), None);

    assert_eq!(picker.stage, Stage::CustomForm);
    assert_eq!(picker.custom_provider_id, "ds4");
    assert_eq!(picker.custom_provider_base_url, "http://127.0.0.1:8000/v1");
    assert_eq!(picker.custom_provider_model, "deepseek-v4-flash");
    assert!(picker.custom_provider_api_key_env.is_empty());

    match picker.handle_key(key(KeyCode::Enter)) {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerCustomProviderSubmitted {
            provider_id,
            base_url,
            model,
            api_key_env,
        }) => {
            assert_eq!(provider_id, "ds4");
            assert_eq!(base_url, "http://127.0.0.1:8000/v1");
            assert_eq!(model.as_deref(), Some("deepseek-v4-flash"));
            assert_eq!(api_key_env, None);
        }
        other => panic!("expected DS4 custom-provider submit event, got {other:?}"),
    }
}

#[test]
fn named_custom_provider_selection_preserves_provider_id() {
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "local_acme".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://localhost:9000/v1".to_string()),
            model: Some("acme/code-1".to_string()),
            ..Default::default()
        },
    );
    let config = Config {
        provider: Some("local_acme".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Config::default()
    };
    let mut picker = ProviderPickerView::new(ApiProvider::Custom, &config);

    let action = picker.handle_key(key(KeyCode::Enter));

    match action {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
            provider,
            provider_id,
        }) => {
            assert_eq!(provider, ApiProvider::Custom);
            assert_eq!(provider_id.as_deref(), Some("local_acme"));
        }
        other => panic!("expected named custom provider apply, got {other:?}"),
    }
}

#[test]
fn named_custom_provider_model_shortcut_preserves_provider_id() {
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "local_acme".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://localhost:9000/v1".to_string()),
            model: Some("acme/code-1".to_string()),
            ..Default::default()
        },
    );
    let config = Config {
        provider: Some("local_acme".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Config::default()
    };
    let mut picker = ProviderPickerView::new(ApiProvider::Custom, &config);

    let action = picker.handle_key(key(KeyCode::Char('m')));

    match action {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerOpenModels {
            provider,
            provider_id,
        }) => {
            assert_eq!(provider, ApiProvider::Custom);
            assert_eq!(provider_id.as_deref(), Some("local_acme"));
        }
        other => panic!("expected named custom provider model shortcut, got {other:?}"),
    }
}

#[test]
fn provider_dashboard_row_surfaces_anthropic_wire_protocol() {
    let config = Config::default();
    let row =
        ProviderDashboardRow::from_config(ApiProvider::Anthropic, ApiProvider::Deepseek, &config);

    assert_eq!(row.provider_id, "anthropic");
    assert_eq!(row.supported_protocols, vec!["anthropic".to_string()]);
    assert_eq!(row.catalog_status, ProviderCatalogStatus::Bundled);
    assert!(row.available_model_count >= 3);
}

#[test]
fn provider_dashboard_row_surfaces_openmodel_messages_route() {
    let _lock = crate::test_support::lock_test_env();
    let _openmodel_key = EnvVarGuard::remove("OPENMODEL_API_KEY");
    let config = Config::default();
    let row =
        ProviderDashboardRow::from_config(ApiProvider::Openmodel, ApiProvider::Deepseek, &config);

    assert_eq!(row.provider_id, "openmodel");
    assert_eq!(row.display_name, "OpenModel");
    assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
    assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
    assert_eq!(row.supported_protocols, vec!["anthropic".to_string()]);
    assert_eq!(row.base_url, crate::config::DEFAULT_OPENMODEL_BASE_URL);
    assert_eq!(row.default_route.logical_model, "deepseek-v4-flash");
    assert_eq!(row.default_route.wire_model, "deepseek-v4-flash");
    assert!(
        row.messages
            .iter()
            .any(|message| message.contains("missing OPENMODEL_API_KEY"))
    );
}

#[test]
fn provider_dashboard_row_marks_missing_api_key_as_needs_key() {
    let _lock = crate::test_support::lock_test_env();
    let _openrouter_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
    let config = Config::default();
    let row =
        ProviderDashboardRow::from_config(ApiProvider::Openrouter, ApiProvider::Deepseek, &config);

    assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
    assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
    assert_eq!(row.readiness.label(), "missing key");
    let hint = row.compact_hint();
    assert!(hint.contains("key:not-set"));
    assert!(!hint.contains("needs-auth"));
    assert!(!hint.contains("auth:missing"));
    assert!(
        row.messages
            .iter()
            .any(|message| message.contains("missing OPENROUTER_API_KEY"))
    );
}

/// The visible payoff of the sourced resolver. Before this change the row
/// said "missing OPENROUTER_API_KEY" and nothing else, so a user could not
/// tell whether the durable slot had been read and found empty, skipped, or
/// never consulted. The note must now name the places that were probed and
/// the command that fixes the first of them.
#[test]
fn missing_key_note_names_the_places_that_were_checked() {
    let _lock = crate::test_support::lock_test_env();
    let _openrouter_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
    let config = Config::default();
    let row =
        ProviderDashboardRow::from_config(ApiProvider::Openrouter, ApiProvider::Deepseek, &config);

    let note = row
        .messages
        .iter()
        .find(|message| message.contains("missing OPENROUTER_API_KEY"))
        .expect("missing-key note");
    assert!(
        note.contains("checked "),
        "the note must say where it looked: {note}"
    );
    assert!(
        note.contains("secret store \"openrouter\""),
        "the durable slot must be named: {note}"
    );
    assert!(
        note.contains("fix: "),
        "the note must offer an action: {note}"
    );
    assert!(
        !note.contains("sk-"),
        "a credential note must never carry key material: {note}"
    );
}

/// Every row states which place its credential came from, so a
/// "key:configured" row can be reconciled with a request that used a
/// different source.
#[test]
fn every_row_states_its_credential_source() {
    let _lock = crate::test_support::lock_test_env();
    let _openrouter_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
    let config = Config::default();
    let missing =
        ProviderDashboardRow::from_config(ApiProvider::Openrouter, ApiProvider::Deepseek, &config);
    assert_eq!(missing.credential_source, "not found");

    let _key = EnvVarGuard::set("OPENROUTER_API_KEY", "test-value");
    let configured =
        ProviderDashboardRow::from_config(ApiProvider::Openrouter, ApiProvider::Deepseek, &config);
    assert_eq!(configured.credential_source, "OPENROUTER_API_KEY");
    assert!(
        !configured.credential_source.contains("test-value"),
        "the source is a label, never the value"
    );
}

#[test]
fn modelstudio_family_key_marks_all_variants_configured() {
    let _guard = crate::test_support::lock_test_env();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
    let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
    let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", tmp.path());
    let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _ms_key = crate::test_support::EnvVarGuard::remove("MODELSTUDIO_API_KEY");
    let _dashscope_key = crate::test_support::EnvVarGuard::remove("DASHSCOPE_API_KEY");
    let _cli_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
    let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");

    // One saved key on the Token Plan variant, marked by the save path.
    codewhale_secrets::Secrets::auto_detect()
        .set("modelstudio-token-plan", "ms-family-key")
        .expect("seed family slot");
    let config = Config {
        provider: Some("deepseek".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            modelstudio_token_plan: crate::config::ProviderConfig {
                auth_mode: Some("api_key".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };

    for variant in [
        ApiProvider::ModelstudioTokenPlan,
        ApiProvider::ModelstudioTokenPlanAnthropic,
        ApiProvider::ModelstudioCodingPlan,
        ApiProvider::ModelstudioCodingPlanAnthropic,
    ] {
        let row = ProviderDashboardRow::from_config(variant, ApiProvider::Deepseek, &config);
        assert_eq!(
            row.auth_status,
            ProviderAuthStatus::Configured,
            "{variant:?} must resolve the family's one saved key"
        );
    }
}

#[test]
fn provider_dashboard_row_marks_route_resolver_errors_as_invalid() {
    let config = Config {
        api_key: Some("deepseek-key".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            deepseek: crate::config::ProviderConfig {
                model: Some("anthropic/claude-foreign".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let row =
        ProviderDashboardRow::from_config(ApiProvider::Deepseek, ApiProvider::Deepseek, &config);

    assert_eq!(row.auth_status, ProviderAuthStatus::Configured);
    assert_eq!(row.readiness, ResolvedProviderReadiness::InvalidRoute);
    assert_eq!(row.default_route.wire_model, "unresolved");
    assert!(
        row.messages
            .iter()
            .any(|message| message.contains("route validation failed"))
    );
}

#[test]
fn provider_dashboard_render_includes_route_protocol_usage_and_base_url() {
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            openai: crate::config::ProviderConfig {
                api_key: Some("openai-key".to_string()),
                base_url: Some("http://localhost:9000/v1".to_string()),
                model: Some("custom-model".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let picker = ProviderPickerView::new(ApiProvider::Openai, &config);

    let rendered = render_text(&picker, 124, 18);

    assert!(rendered.contains("key:configured"));
    assert!(!rendered.contains("auth:configured"));
    assert!(rendered.contains("Route: custom-model"));
    assert!(rendered.contains("chat"));
    assert!(rendered.contains("cost: unknown"));
    assert!(rendered.contains("Endpoint: http://localhost:9000/v1"));
}

#[test]
fn ollama_is_selectable_without_key() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Ollama);
    assert_eq!(picker.selected_provider(), ApiProvider::Ollama);
    assert!(picker.selected_has_key());
    let action = picker.handle_key(key(KeyCode::Enter));
    match action {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
            provider,
            provider_id,
        }) => {
            assert_eq!(provider, ApiProvider::Ollama);
            assert_eq!(provider_id, None);
        }
        other => panic!("expected ProviderPickerApplied, got {other:?}"),
    }
}

#[test]
fn pressing_m_opens_models_for_selected_provider() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Openrouter);

    let action = picker.handle_key(key(KeyCode::Char('m')));

    // #3083: `m` jumps to the model picker scoped to the highlighted
    // provider rather than acting as a type-ahead seek.
    match action {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerOpenModels {
            provider,
            provider_id,
        }) => {
            assert_eq!(provider, ApiProvider::Openrouter);
            assert_eq!(provider_id, None);
        }
        other => panic!("expected ProviderPickerOpenModels, got {other:?}"),
    }
}

#[test]
fn pressing_uppercase_m_also_opens_models() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

    // Case-insensitive like the `R` edit-key affordance: a bare `M` works.
    let action = picker.handle_key(key(KeyCode::Char('M')));

    match action {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerOpenModels {
            provider,
            provider_id,
        }) => {
            assert_eq!(provider, ApiProvider::Deepseek);
            assert_eq!(provider_id, None);
        }
        other => panic!("expected ProviderPickerOpenModels, got {other:?}"),
    }
}

#[test]
fn picker_marks_active_provider_as_initial_selection() {
    let config = Config::default();
    let picker = ProviderPickerView::new(ApiProvider::Openrouter, &config);
    assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
    assert!(picker.rows[picker.selected_idx].is_active);
}

#[test]
fn list_navigation_wraps_between_first_and_last_provider() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    // Wrap across the full catalog (#3830), not just the configured
    // subset, which would only contain the active provider here.
    picker.toggle_view();
    let first = picker.rows.first().expect("non-empty list").provider;
    let last = picker.rows.last().expect("non-empty list").provider;

    // Order-independent: jump to the first entry, wrap up to the last, back down.
    picker.selected_idx = 0;
    picker.handle_key(key(KeyCode::Up));
    assert_eq!(picker.selected_provider(), last);

    picker.handle_key(key(KeyCode::Down));
    assert_eq!(picker.selected_provider(), first);
}

#[test]
fn enter_with_no_key_transitions_to_key_entry_stage() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    // Move to OpenRouter, which has no key in default config.
    move_to_provider(&mut picker, ApiProvider::Openrouter);
    assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
    let action = picker.handle_key(key(KeyCode::Enter));
    assert!(matches!(action, ViewAction::None));
    assert_eq!(picker.stage, Stage::KeyEntry);
}

#[test]
fn enter_with_existing_key_emits_apply_and_closes() {
    let config = Config {
        api_key: Some("existing-deepseek-key".to_string()),
        ..Config::default()
    };
    let mut picker = ProviderPickerView::new(ApiProvider::NvidiaNim, &config);
    // Navigate to DeepSeek, which has a key from the top-level config.
    move_to_provider(&mut picker, ApiProvider::Deepseek);
    let action = picker.handle_key(key(KeyCode::Enter));
    match action {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
            provider,
            provider_id,
        }) => {
            assert_eq!(provider, ApiProvider::Deepseek);
            assert_eq!(provider_id, None);
        }
        other => panic!("expected ProviderPickerApplied, got {other:?}"),
    }
}

#[test]
fn new_for_missing_auth_opens_key_entry_focused_on_target() {
    // #3830: the missing-auth handoff drops the user onto the target
    // provider's key prompt, not a dead-end error.
    let config = Config::default();
    let picker = ProviderPickerView::new_for_missing_auth(
        ApiProvider::Deepseek,
        ApiProvider::Anthropic,
        &config,
        None,
    )
    .expect("Anthropic has a picker row");
    assert_eq!(picker.stage, Stage::KeyEntry);
    assert_eq!(picker.selected_provider(), ApiProvider::Anthropic);
}

#[test]
fn setup_catalog_shows_all_providers_from_configured_view() {
    let config = Config::default();
    let picker = ProviderPickerView::new_for_setup(ApiProvider::Deepseek, None, &config, None);

    assert_eq!(picker.stage, Stage::List);
    assert_eq!(picker.view, ProviderListView::Catalog);
    assert_eq!(picker.visible_row_count(), picker.rows.len());
    let mut listed = picker
        .rows
        .iter()
        .map(|row| row.provider)
        .collect::<Vec<_>>();
    // With no configured custom providers, the catalog keeps the Custom
    // entry so a custom endpoint can still be created from setup. The
    // canonical universe is the user-facing catalog (one identity per
    // vendor): dual-wire dialects are `wire` config and plan variants are
    // `mode`/base_url, not picker rows.
    let mut expected = ApiProvider::catalog().to_vec();
    listed.sort_by_key(|provider| provider.as_str());
    expected.sort_by_key(|provider| provider.as_str());
    assert_eq!(
        listed, expected,
        "setup must use the canonical provider universe"
    );
}

#[test]
fn setup_catalog_focuses_missing_provider_key_entry() {
    let _lock = crate::test_support::lock_test_env();
    let _anthropic_key = crate::test_support::EnvVarGuard::remove("ANTHROPIC_API_KEY");
    let config = Config::default();
    let picker = ProviderPickerView::new_for_setup(
        ApiProvider::Deepseek,
        Some(ApiProvider::Anthropic),
        &config,
        None,
    );

    assert_eq!(picker.view, ProviderListView::Catalog);
    assert_eq!(picker.stage, Stage::KeyEntry);
    assert_eq!(picker.selected_provider(), ApiProvider::Anthropic);
    assert!(picker.api_key_input.is_empty());
}

/// #4763: onboarding focuses the persisted route but must still open on
/// the navigable list. Jumping straight into key/OAuth entry hid the
/// provider catalog from returning users with a missing key.
#[test]
fn onboarding_catalog_focuses_missing_provider_without_leaving_the_list() {
    let _lock = crate::test_support::lock_test_env();
    let _anthropic_key = crate::test_support::EnvVarGuard::remove("ANTHROPIC_API_KEY");
    let config = Config::default();
    let picker = ProviderPickerView::new_for_onboarding(
        ApiProvider::Deepseek,
        Some(ApiProvider::Anthropic),
        &config,
        None,
    );

    assert_eq!(picker.stage, Stage::List);
    assert_eq!(picker.view, ProviderListView::Catalog);
    assert_eq!(picker.selected_provider(), ApiProvider::Anthropic);
    assert_eq!(
        picker.visible_row_count(),
        picker.rows.len(),
        "onboarding must show the whole provider catalog"
    );
}

#[test]
fn first_run_onboarding_shows_all_providers_including_hosted() {
    let _lock = crate::test_support::lock_test_env();
    let config = Config::default();
    let picker = ProviderPickerView::new_for_onboarding(ApiProvider::Deepseek, None, &config, None);

    // First-run setup opens on the full catalog with the active provider
    // (DeepSeek) selected — hosted APIs are visible immediately, not
    // hidden behind a keypress (#n3onr1ft feedback, 2026-08-23).
    assert_eq!(picker.stage, Stage::List);
    assert_eq!(picker.view, ProviderListView::Catalog);
    assert_eq!(picker.selected_provider(), ApiProvider::Deepseek);

    let visible = picker
        .filtered_rows()
        .into_iter()
        .map(|(_, row)| row.provider)
        .collect::<Vec<_>>();
    assert!(!visible.is_empty());
    // Hosted providers AND local runtimes are both present up front.
    assert!(visible.contains(&ApiProvider::Deepseek));
    assert!(visible.contains(&ApiProvider::Ollama));
    assert!(
        visible.iter().any(|provider| !provider.is_self_hosted()),
        "hosted providers must be visible on first run: {visible:?}"
    );

    let rendered = render_text(&picker, 40, 12);
    assert!(
        rendered.contains(crate::tui::glyphs::SELECTION),
        "{rendered}"
    );
    for (idx, line) in rendered.lines().enumerate() {
        assert!(
            crate::tui::ui_text::text_display_width(line) <= 40,
            "40x12 line {idx} clips: {line:?}\n{rendered}"
        );
    }
}

#[test]
fn local_shortcut_filters_cloud_rows_from_the_catalog() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new_for_onboarding(
        ApiProvider::Deepseek,
        Some(ApiProvider::Deepseek),
        &config,
        None,
    );
    assert_eq!(picker.view, ProviderListView::Catalog);

    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('l'))),
        ViewAction::None
    ));
    assert_eq!(picker.view, ProviderListView::Local);
    assert_eq!(picker.selected_provider(), ApiProvider::Ollama);
    assert!(
        picker
            .filtered_rows()
            .into_iter()
            .all(|(_, row)| row.provider.is_self_hosted())
    );
}

#[test]
fn onboarding_catalog_honors_typed_credentials_for_every_builtin_provider() {
    use codewhale_config::provider::CredentialAcquisition;

    let _global_env = crate::test_support::lock_test_env();
    let home = tempfile::tempdir().expect("isolated provider catalog home");
    let _home = EnvVarGuard::set("HOME", home.path().to_string_lossy().as_ref());
    let _codewhale_home =
        EnvVarGuard::set("CODEWHALE_HOME", home.path().to_string_lossy().as_ref());
    let _codex_home = EnvVarGuard::set("CODEX_HOME", home.path().to_string_lossy().as_ref());
    let _secret_backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let mut key_envs = ApiProvider::all()
        .iter()
        .flat_map(|provider| provider.env_vars().iter().copied())
        .collect::<Vec<_>>();
    key_envs.sort_unstable();
    key_envs.dedup();
    let _missing_keys = key_envs
        .into_iter()
        .map(EnvVarGuard::remove)
        .collect::<Vec<_>>();
    let config = Config::default();

    // Every provider the catalog actually lists. Hidden dual-wire/plan
    // variants share their vendor primary's row and credential metadata,
    // so `ApiProvider::all()` cannot be driven through the visible list.
    for provider in ApiProvider::catalog().iter().copied() {
        let mut picker = ProviderPickerView::new_for_onboarding(
            ApiProvider::Deepseek,
            Some(provider),
            &config,
            None,
        );
        assert_eq!(picker.selected_provider(), provider, "{provider:?}");
        assert_eq!(picker.stage, Stage::List, "{provider:?}");

        let action = picker.handle_key(key(KeyCode::Enter));
        match provider.credential_help().acquisition {
            CredentialAcquisition::ApiKey => {
                assert!(matches!(action, ViewAction::None), "{provider:?}");
                assert!(
                    matches!(picker.stage, Stage::KeyEntry | Stage::StepfunBillingRoute),
                    "{provider:?} entered {:?}",
                    picker.stage
                );
            }
            CredentialAcquisition::ApiKeyOrOAuth => {
                assert_eq!(provider, ApiProvider::Xai, "{provider:?}");
                assert!(matches!(action, ViewAction::None), "{provider:?}");
                let choices = render_text(&picker, 80, 24);
                assert!(choices.contains("API key"), "{choices}");
                assert!(choices.contains("device OAuth"), "{choices}");

                // Choice 1 is an ordinary API-key path. Text remains a key;
                // it is never reinterpreted as an OAuth bearer token.
                assert!(matches!(
                    picker.handle_key(key(KeyCode::Char('1'))),
                    ViewAction::None
                ));
                assert!(matches!(
                    picker.handle_key(key(KeyCode::Enter)),
                    ViewAction::None
                ));
                assert_eq!(picker.stage, Stage::KeyEntry);
                for ch in "violet-".chars() {
                    picker.handle_key(key(KeyCode::Char(ch)));
                }
                assert!(picker.handle_paste("otter-key"));
                let key_text = "violet-otter-key";
                assert_eq!(picker.api_key_input, key_text);
                for (width, height) in [(80, 24), (120, 32)] {
                    let rendered = render_text(&picker, width, height);
                    assert!(!rendered.contains(key_text), "{width}x{height}: {rendered}");
                    assert!(rendered.contains('*'), "{width}x{height}: {rendered}");
                }
                assert!(matches!(
                    picker.handle_key(key(KeyCode::Enter)),
                    ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
                        provider: ApiProvider::Xai,
                        provider_id: None,
                        api_key,
                        base_url: None,
                    }) if api_key == key_text
                ));

                // Choice 2 is the provider-native device flow and emits only
                // the request event; the picker never manufactures a token.
                let mut oauth = ProviderPickerView::new_for_onboarding(
                    ApiProvider::Deepseek,
                    Some(ApiProvider::Xai),
                    &config,
                    None,
                );
                assert!(matches!(
                    oauth.handle_key(key(KeyCode::Enter)),
                    ViewAction::None
                ));
                assert!(matches!(
                    oauth.handle_key(key(KeyCode::Char('2'))),
                    ViewAction::None
                ));
                assert!(matches!(
                    oauth.handle_key(key(KeyCode::Enter)),
                    ViewAction::EmitAndClose(ViewEvent::ProviderPickerXaiOAuthRequested)
                ));
            }
            CredentialAcquisition::LocalOptional => assert!(matches!(
                action,
                ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
                    provider: applied,
                    provider_id: None,
                }) if applied == provider
            )),
            CredentialAcquisition::OAuth => {
                assert!(matches!(action, ViewAction::None), "{provider:?}");
                assert_eq!(picker.stage, Stage::KeyEntry, "{provider:?}");
                assert!(picker.handle_paste("fixture-oauth-paste"));
                assert!(
                    picker.api_key_input.is_empty(),
                    "{provider:?} must reject key paste"
                );
            }
            CredentialAcquisition::Configuration => {
                assert_eq!(provider, ApiProvider::Custom);
                assert!(matches!(action, ViewAction::None));
                assert_eq!(picker.stage, Stage::CustomForm);
                assert!(picker.api_key_input.is_empty());
            }
        }
    }
}

#[test]
fn credential_draft_is_masked_and_escape_drops_it_without_persistence() {
    let _global_env = crate::test_support::lock_test_env();
    let home = tempfile::tempdir().expect("isolated credential draft home");
    let _home = EnvVarGuard::set("HOME", home.path().to_string_lossy().as_ref());
    let _codewhale_home =
        EnvVarGuard::set("CODEWHALE_HOME", home.path().to_string_lossy().as_ref());
    let _secret_backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _openrouter_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
    let config = Config::default();
    let draft = ["violet", "otter", "draft", "7361"].join("-");
    let mut picker = ProviderPickerView::new_for_missing_auth(
        ApiProvider::Deepseek,
        ApiProvider::Openrouter,
        &config,
        None,
    )
    .expect("OpenRouter key editor");

    let ctrl_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
    assert!(matches!(picker.handle_key(ctrl_v), ViewAction::None));
    assert!(
        picker.api_key_input.is_empty(),
        "shortcut must not type `v`"
    );
    let shifted_v = KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT);
    assert!(matches!(picker.handle_key(shifted_v), ViewAction::None));
    assert_eq!(
        picker.api_key_input, "V",
        "shifted credential text is valid"
    );
    assert!(matches!(
        picker.handle_key(key(KeyCode::Backspace)),
        ViewAction::None
    ));
    assert!(picker.handle_paste(&draft));
    assert_eq!(picker.api_key_input, draft);
    for (width, height) in [(80, 24), (120, 32)] {
        let rendered = render_text(&picker, width, height);
        assert!(!rendered.contains(&draft), "{width}x{height}: {rendered}");
        assert!(rendered.contains('*'), "{width}x{height}: {rendered}");
    }

    assert!(matches!(
        picker.handle_key(key(KeyCode::Esc)),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::List);
    assert!(picker.api_key_input.is_empty());
    assert_eq!(
        std::fs::read_dir(home.path())
            .expect("isolated home remains readable")
            .count(),
        0,
        "Esc must not create config or credential-backend files"
    );
}

/// #4763: Escape backs out one stage at a time — key entry returns to the
/// list, and only the list dismisses the picker.
#[test]
fn onboarding_escape_walks_key_entry_back_to_the_list_then_dismisses() {
    let _lock = crate::test_support::lock_test_env();
    let _anthropic_key = crate::test_support::EnvVarGuard::remove("ANTHROPIC_API_KEY");
    let config = Config::default();
    let mut picker = ProviderPickerView::new_for_onboarding(
        ApiProvider::Deepseek,
        Some(ApiProvider::Anthropic),
        &config,
        None,
    );
    assert_eq!(picker.stage, Stage::List);

    picker.enter_key_entry();
    assert_eq!(picker.stage, Stage::KeyEntry);

    assert!(matches!(
        picker.handle_key(key(KeyCode::Esc)),
        ViewAction::None
    ));
    assert_eq!(
        picker.stage,
        Stage::List,
        "Escape from key entry returns to the provider list"
    );

    assert!(
        matches!(
            picker.handle_key(key(KeyCode::Esc)),
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerDismissed { .. })
        ),
        "Escape from the list dismisses the picker"
    );
}

#[test]
fn setup_catalog_uses_setup_title() {
    let config = Config::default();
    let picker = ProviderPickerView::new_for_setup(ApiProvider::Deepseek, None, &config, None);

    let rendered = render_text(&picker, 96, 20);

    assert!(rendered.contains("Provider setup"));
}

#[test]
fn setup_catalog_key_entry_uses_setup_reopen_hint() {
    let config = Config::default();
    let picker = ProviderPickerView::new_for_setup(
        ApiProvider::Deepseek,
        Some(ApiProvider::Anthropic),
        &config,
        None,
    );

    let rendered = render_text(&picker, 96, 20);

    assert!(rendered.contains("API key"));
    assert!(rendered.contains("/setup provider"));
    assert!(!rendered.contains("re-open /provider."));
}

#[test]
fn default_provider_picker_keeps_provider_reopen_hint() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Anthropic);
    picker.handle_key(key(KeyCode::Enter));

    let rendered = render_text(&picker, 96, 20);

    assert!(rendered.contains("API key"));
    assert!(rendered.contains("re-open /provider."));
    assert!(!rendered.contains("/setup provider"));
}

#[test]
fn setup_catalog_focuses_configured_provider_without_rekeying() {
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            openai: crate::config::ProviderConfig {
                api_key: Some("openai-key".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let picker = ProviderPickerView::new_for_setup(
        ApiProvider::Deepseek,
        Some(ApiProvider::Openai),
        &config,
        None,
    );

    assert_eq!(picker.view, ProviderListView::Catalog);
    assert_eq!(picker.stage, Stage::List);
    assert_eq!(picker.selected_provider(), ApiProvider::Openai);
}

#[test]
fn new_for_key_entry_with_error_opens_prompt_and_renders_reason() {
    let config = Config::default();
    let picker = ProviderPickerView::new_for_key_entry_with_error(
        ApiProvider::Deepseek,
        ApiProvider::Openrouter,
        &config,
        None,
        "HTTP 401: unauthorized".to_string(),
    )
    .expect("OpenRouter has a picker row");

    assert_eq!(picker.stage, Stage::KeyEntry);
    assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
    let rendered = render_text(&picker, 90, 14);
    assert!(rendered.contains("Verification failed: HTTP 401: unauthorized"));
}

#[test]
fn new_for_model_pick_after_validation_opens_model_stage() {
    let config = Config::default();
    let picker = ProviderPickerView::new_for_model_pick_after_validation(
        ApiProvider::Deepseek,
        ApiProvider::Openrouter,
        &config,
        None,
        "sk-validated".to_string(),
        None,
    )
    .expect("OpenRouter has a picker row");

    assert_eq!(picker.stage, Stage::ModelPick);
    assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
    assert_eq!(picker.pending_api_key.as_deref(), Some("sk-validated"));
    assert!(!picker.model_options.is_empty());
    assert!(picker.selected_model.is_some());
}

#[test]
fn model_pick_enter_advances_to_confirm_and_confirm_emits_setup() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new_for_model_pick_after_validation(
        ApiProvider::Deepseek,
        ApiProvider::Openrouter,
        &config,
        None,
        "sk-validated".to_string(),
        None,
    )
    .expect("OpenRouter has a picker row");

    assert_eq!(picker.stage, Stage::ModelPick);
    let action = picker.handle_key(key(KeyCode::Enter));
    assert!(matches!(action, ViewAction::None));
    assert_eq!(picker.stage, Stage::Confirm);

    let selected_model = picker
        .selected_model
        .clone()
        .expect("model selected on confirm");
    let action = picker.handle_key(key(KeyCode::Enter));
    match action {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerSetupConfirmed {
            provider,
            provider_id,
            api_key,
            model,
            ..
        }) => {
            assert_eq!(provider, ApiProvider::Openrouter);
            assert_eq!(provider_id, None);
            assert_eq!(api_key, "sk-validated");
            assert_eq!(model, selected_model);
        }
        other => panic!("expected ProviderPickerSetupConfirmed, got {other:?}"),
    }
}

#[test]
fn exact_kimi_code_setup_asks_for_plan_and_emits_selected_context_window() {
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            moonshot: crate::config::ProviderConfig {
                base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut picker = ProviderPickerView::new_for_model_pick_after_validation(
        ApiProvider::Deepseek,
        ApiProvider::Moonshot,
        &config,
        None,
        "sk-kimi-plan".to_string(),
        None,
    )
    .expect("Moonshot has a picker row");

    assert_eq!(picker.stage, Stage::ModelPick);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Enter)),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::PlanTier);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('2'))),
        ViewAction::None
    ));
    assert!(matches!(
        picker.handle_key(key(KeyCode::Enter)),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::Confirm);
    match picker.handle_key(key(KeyCode::Enter)) {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerSetupConfirmed {
            context_window,
            model,
            ..
        }) => {
            assert_eq!(model, crate::config::KIMI_CODE_K3_MODEL);
            assert_eq!(context_window, Some(1_048_576));
        }
        other => panic!("expected Kimi Code setup confirmation, got {other:?}"),
    }
}

#[test]
fn model_pick_and_confirm_esc_backs_out_without_emitting() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new_for_model_pick_after_validation(
        ApiProvider::Deepseek,
        ApiProvider::Openrouter,
        &config,
        None,
        "sk-validated".to_string(),
        None,
    )
    .expect("OpenRouter has a picker row");

    picker.handle_key(key(KeyCode::Enter));
    assert_eq!(picker.stage, Stage::Confirm);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Esc)),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::ModelPick);

    assert!(matches!(
        picker.handle_key(key(KeyCode::Esc)),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::KeyEntry);
    assert_eq!(picker.api_key_input, "sk-validated");
    assert!(picker.pending_api_key.is_some());
}

fn stepfun_config(base_url: Option<&str>) -> Config {
    Config {
        providers: Some(crate::config::ProvidersConfig {
            stepfun: crate::config::ProviderConfig {
                base_url: base_url.map(str::to_string),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// #4526: StepFun's two billing tracks are two endpoints. Setup asks which
/// one the key belongs to, and the choice reaches key entry as a pending —
/// not yet persisted — endpoint.
#[test]
fn stepfun_setup_asks_for_billing_route_before_key_entry() {
    let config = stepfun_config(None);
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Stepfun);

    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('r'))),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::StepfunBillingRoute);
    assert_eq!(
        picker.stepfun_billing_route,
        StepfunBillingRoute::PayAsYouGo
    );

    // The endpoints are the whole difference between the two tracks, so
    // both have to be legible at the narrow terminal size too.
    for (w, h) in [(80u16, 24u16), (120u16, 32u16)] {
        let rendered = render_text(&picker, w, h);
        assert!(
            rendered.contains(crate::config::DEFAULT_STEPFUN_BASE_URL)
                && rendered.contains(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL),
            "{w}x{h} must show both StepFun endpoints:\n{rendered}"
        );
        for (idx, line) in rendered.lines().enumerate() {
            assert!(
                crate::tui::ui_text::text_display_width(line) <= w as usize,
                "{w}x{h} billing-route line {idx} overflows: {line:?}"
            );
        }
    }

    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('2'))),
        ViewAction::None
    ));
    assert!(matches!(
        picker.handle_key(key(KeyCode::Enter)),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::KeyEntry);
    assert_eq!(
        picker.pending_base_url.as_deref(),
        Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL)
    );
}

/// The chosen endpoint rides on the key-submit event so the live check in
/// `ui.rs` probes the Step Plan route, not the pay-as-you-go default.
#[test]
fn stepfun_plan_choice_travels_with_the_key_for_validation() {
    let config = stepfun_config(None);
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Stepfun);
    picker.handle_key(key(KeyCode::Char('r')));
    picker.handle_key(key(KeyCode::Char('2')));
    picker.handle_key(key(KeyCode::Enter));
    for c in "step-plan-key".chars() {
        picker.handle_key(key(KeyCode::Char(c)));
    }

    match picker.handle_key(key(KeyCode::Enter)) {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
            provider,
            api_key,
            base_url,
            ..
        }) => {
            assert_eq!(provider, ApiProvider::Stepfun);
            assert_eq!(api_key, "step-plan-key");
            assert_eq!(
                base_url.as_deref(),
                Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL)
            );
        }
        other => panic!("expected ProviderPickerApiKeySubmitted, got {other:?}"),
    }
}

/// Confirm carries exactly the validated endpoint, and nothing else about
/// the route, so the handler writes only `[providers.stepfun] base_url`.
#[test]
fn stepfun_confirm_emits_only_the_validated_endpoint() {
    let config = stepfun_config(None);
    let mut picker = ProviderPickerView::new_for_model_pick_after_validation(
        ApiProvider::Deepseek,
        ApiProvider::Stepfun,
        &config,
        None,
        "step-plan-key".to_string(),
        Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL.to_string()),
    )
    .expect("StepFun has a picker row");

    assert_eq!(picker.stage, Stage::ModelPick);
    picker.handle_key(key(KeyCode::Enter));
    assert_eq!(picker.stage, Stage::Confirm);
    match picker.handle_key(key(KeyCode::Enter)) {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerSetupConfirmed {
            provider,
            base_url,
            context_window,
            ..
        }) => {
            assert_eq!(provider, ApiProvider::Stepfun);
            assert_eq!(
                base_url.as_deref(),
                Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL)
            );
            assert_eq!(context_window, None);
        }
        other => panic!("expected ProviderPickerSetupConfirmed, got {other:?}"),
    }
}

/// A hand-configured StepFun endpoint is a deliberate choice. The wizard
/// skips the billing-route stage entirely and emits no endpoint, so the
/// custom value is never silently rewritten (#4526).
#[test]
fn stepfun_custom_base_url_survives_the_wizard_untouched() {
    let custom = "https://stepfun.internal.example/v1";
    let config = stepfun_config(Some(custom));
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Stepfun);
    assert_eq!(picker.rows[picker.selected_idx].base_url, custom);

    picker.handle_key(key(KeyCode::Char('r')));
    assert_eq!(picker.stage, Stage::KeyEntry);
    assert_eq!(picker.pending_base_url, None);
    assert_eq!(picker.rows[picker.selected_idx].base_url, custom);

    for c in "custom-key".chars() {
        picker.handle_key(key(KeyCode::Char(c)));
    }
    match picker.handle_key(key(KeyCode::Enter)) {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted { base_url, .. }) => {
            assert_eq!(base_url, None, "custom endpoint must not be rewritten")
        }
        other => panic!("expected ProviderPickerApiKeySubmitted, got {other:?}"),
    }
}

/// A StepFun route already on Step Plan re-opens preselected there rather
/// than defaulting the user back onto pay-as-you-go.
#[test]
fn stepfun_plan_route_reopens_preselected() {
    let config = stepfun_config(Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL));
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Stepfun);
    picker.handle_key(key(KeyCode::Char('r')));

    assert_eq!(picker.stage, Stage::StepfunBillingRoute);
    assert_eq!(picker.stepfun_billing_route, StepfunBillingRoute::StepPlan);
}

/// #4526: OpenCode Go (subscription allowance) and OpenCode Zen
/// (pay-as-you-go) are separate billing tracks and must not present as the
/// same generic meter.
#[test]
fn opencode_go_and_zen_read_as_distinct_billing_tracks() {
    let go = usage_meter_for(ApiProvider::OpencodeGo);
    let zen = usage_meter_for(ApiProvider::OpencodeZen);
    assert_ne!(go, zen);
    assert!(go.contains("subscription"), "Go label was {go:?}");
    assert!(zen.contains("pay-as-you-go"), "Zen label was {zen:?}");
    assert_ne!(go, usage_meter_for(ApiProvider::Openrouter));

    // Go never reports catalog token prices: its allowance is not spend.
    assert_eq!(
        pricing_label(
            ApiProvider::OpencodeGo,
            Some(&PricingSku::Token {
                input_per_mtok: Some(1.0),
                output_per_mtok: Some(2.0),
            }),
        ),
        go
    );
}

#[test]
fn guided_flow_stages_render_at_80x24_and_120x32() {
    let config = Config::default();
    let model_pick = ProviderPickerView::new_for_model_pick_after_validation(
        ApiProvider::Deepseek,
        ApiProvider::Openrouter,
        &config,
        None,
        "sk-validated-key".to_string(),
        None,
    )
    .expect("OpenRouter has a picker row");
    let mut confirm = ProviderPickerView::new_for_model_pick_after_validation(
        ApiProvider::Deepseek,
        ApiProvider::Openrouter,
        &config,
        None,
        "sk-validated-key".to_string(),
        None,
    )
    .expect("OpenRouter has a picker row");
    confirm.handle_key(key(KeyCode::Enter));
    assert_eq!(confirm.stage, Stage::Confirm);

    for (w, h) in [(80u16, 24u16), (120u16, 32u16)] {
        let model_text = render_text(&model_pick, w, h);
        assert!(
            model_text.contains("Default model") || model_text.contains("default model"),
            "{w}x{h} model pick missing title:\n{model_text}"
        );
        assert!(
            model_text.contains("continue") || model_text.contains("Enter"),
            "{w}x{h} model pick missing continue affordance:\n{model_text}"
        );
        for (idx, line) in model_text.lines().enumerate() {
            assert!(
                crate::tui::ui_text::text_display_width(line) <= w as usize,
                "{w}x{h} model pick line {idx} overflows: {line:?}"
            );
        }

        let confirm_text = render_text(&confirm, w, h);
        assert!(
            confirm_text.contains("Confirm"),
            "{w}x{h} confirm missing title:\n{confirm_text}"
        );
        assert!(
            confirm_text.contains("Provider:") || confirm_text.contains("OpenRouter"),
            "{w}x{h} confirm missing provider summary:\n{confirm_text}"
        );
        assert!(
            confirm_text.contains("Model:") || confirm_text.contains("model"),
            "{w}x{h} confirm missing model summary:\n{confirm_text}"
        );
        // Masked key only — never the raw secret.
        assert!(
            !confirm_text.contains("sk-validated-key"),
            "{w}x{h} confirm leaked raw key:\n{confirm_text}"
        );
        for (idx, line) in confirm_text.lines().enumerate() {
            assert!(
                crate::tui::ui_text::text_display_width(line) <= w as usize,
                "{w}x{h} confirm line {idx} overflows: {line:?}"
            );
        }
    }
}

#[test]
fn configured_provider_can_reenter_key_entry_with_r() {
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            xiaomi_mimo: crate::config::ProviderConfig {
                api_key: Some("mimo-key".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::XiaomiMimo);

    let action = picker.handle_key(key(KeyCode::Char('r')));

    assert!(matches!(action, ViewAction::None));
    assert_eq!(picker.stage, Stage::KeyEntry);
    assert!(picker.api_key_input.is_empty());
}

#[test]
fn configured_api_key_editors_acknowledge_saved_credentials_across_providers() {
    for (provider, config, secret) in [
        (
            ApiProvider::Zai,
            Config {
                providers: Some(crate::config::ProvidersConfig {
                    zai: crate::config::ProviderConfig {
                        api_key: Some("stored-zai-key".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                ..Config::default()
            },
            "stored-zai-key",
        ),
        (
            ApiProvider::Openrouter,
            Config {
                providers: Some(crate::config::ProvidersConfig {
                    openrouter: crate::config::ProviderConfig {
                        api_key: Some("stored-openrouter-key".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                ..Config::default()
            },
            "stored-openrouter-key",
        ),
    ] {
        let mut picker = ProviderPickerView::new(provider, &config);
        move_to_provider(&mut picker, provider);
        picker.handle_key(key(KeyCode::Char('r')));

        let rendered = render_text(&picker, 100, 20);

        assert!(
            rendered.contains("Saved credential configured"),
            "{provider:?}:\n{rendered}"
        );
        assert!(rendered.contains("stored credential"), "{rendered}");
        assert!(rendered.contains("replace saved key"), "{rendered}");
        assert!(rendered.contains("keep current key"), "{rendered}");
        assert!(!rendered.contains("paste key here"), "{rendered}");
        assert!(!rendered.contains(secret), "{rendered}");
    }
}

#[test]
fn ctrl_r_does_not_trigger_key_entry() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

    let action = picker.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));

    assert!(matches!(action, ViewAction::None));
    assert_eq!(picker.stage, Stage::List);
}

#[test]
fn configured_provider_footer_mentions_edit_key() {
    let config = Config {
        api_key: Some("existing-deepseek-key".to_string()),
        ..Config::default()
    };
    let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

    let rendered = render_text(&picker, 80, 14);

    assert!(rendered.contains("Enter"), "rendered: {rendered}");
    assert!(rendered.contains("apply"));
    assert!(rendered.contains("edit key"));
}

#[test]
fn key_entry_enter_submits_after_typing() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    // Navigate to Novita and trigger key entry.
    move_to_provider(&mut picker, ApiProvider::Novita);
    picker.handle_key(key(KeyCode::Enter));
    assert_eq!(picker.stage, Stage::KeyEntry);
    for c in "novita-key".chars() {
        picker.handle_key(key(KeyCode::Char(c)));
    }
    let action = picker.handle_key(key(KeyCode::Enter));
    match action {
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
            provider,
            provider_id,
            api_key,
            base_url,
        }) => {
            assert_eq!(provider, ApiProvider::Novita);
            assert_eq!(provider_id, None);
            assert_eq!(api_key, "novita-key");
            assert_eq!(base_url, None);
        }
        other => panic!("expected ProviderPickerApiKeySubmitted, got {other:?}"),
    }
}

#[test]
fn openai_codex_key_entry_is_oauth_only() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new_for_missing_auth(
        ApiProvider::Deepseek,
        ApiProvider::OpenaiCodex,
        &config,
        None,
    )
    .expect("OpenAI Codex has a picker row");
    assert_eq!(picker.stage, Stage::KeyEntry);

    let rendered = render_text(&picker, 96, 20);
    assert!(rendered.contains("OAuth login"), "{rendered}");
    assert!(rendered.contains("no token is stored here"), "{rendered}");
    assert!(!rendered.contains("save & switch"));
    assert!(!rendered.contains("(paste key here)"));
    assert!(!rendered.contains("Credentials:"));

    assert!(picker.handle_paste("codex-token"));
    for c in "codex-token".chars() {
        picker.handle_key(key(KeyCode::Char(c)));
    }
    assert!(picker.api_key_input.is_empty());
    assert!(matches!(
        picker.handle_key(key(KeyCode::Enter)),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::ExternalConsentChoice);
    let choices = render_text(&picker, 100, 20);
    assert!(choices.contains("Disabled (default)"), "{choices}");
    assert!(choices.contains("Read-only"), "{choices}");
    assert!(choices.contains("Managed (unavailable)"), "{choices}");

    picker.handle_key(key(KeyCode::Char('2')));
    picker.handle_key(key(KeyCode::Enter));
    assert_eq!(picker.stage, Stage::ExternalConsentConfirm);
    let confirm = render_text(&picker, 120, 22);
    assert!(confirm.contains("Owning CLI: Codex CLI"), "{confirm}");
    assert!(confirm.contains("Exact resolved path:"), "{confirm}");
    assert!(confirm.contains("no refresh, identity-provider or discovery requests"));
    assert!(confirm.contains("normal requests to the selected provider"));
    assert!(confirm.contains("external-revoke --provider openai-codex"));
    assert!(matches!(
        picker.handle_key(key(KeyCode::Enter)),
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerExternalConsentConfirmed {
            provider: ApiProvider::OpenaiCodex,
            consent_provider: codewhale_config::ProviderKind::OpenaiCodex,
            source: codewhale_config::ExternalCredentialSource::CodexCli,
            ..
        })
    ));
}

#[test]
fn external_consent_surface_uses_the_selected_locale() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new_for_missing_auth(
        ApiProvider::Deepseek,
        ApiProvider::OpenaiCodex,
        &config,
        None,
    )
    .expect("OpenAI Codex has a picker row")
    .with_locale(crate::localization::Locale::ZhHans);

    picker.handle_key(key(KeyCode::Enter));
    let choices = render_text(&picker, 100, 20);
    let compact = choices
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    assert!(compact.contains("外部凭据访问"), "{choices}");
    assert!(compact.contains("禁用（默认）"), "{choices}");
    assert!(compact.contains("托管（不可用）"), "{choices}");
}

#[test]
fn xai_auth_choice_keeps_api_key_device_oauth_and_external_reuse_distinct() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new_for_missing_auth(
        ApiProvider::Deepseek,
        ApiProvider::Xai,
        &config,
        None,
    )
    .expect("xAI has a picker row");
    assert_eq!(picker.stage, Stage::XaiAuthChoice);

    let rendered = render_text(&picker, 96, 20);
    assert!(rendered.contains("xAI API key"));
    assert!(rendered.contains("Native device OAuth"));
    assert!(rendered.contains("Codewhale-owned storage"));
    picker.handle_key(key(KeyCode::Char('2')));
    assert!(matches!(
        picker.handle_key(key(KeyCode::Enter)),
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerXaiOAuthRequested)
    ));

    let mut external = ProviderPickerView::new_for_missing_auth(
        ApiProvider::Deepseek,
        ApiProvider::Xai,
        &config,
        None,
    )
    .expect("xAI has a picker row");
    assert!(matches!(
        external.handle_key(key(KeyCode::Char('e'))),
        ViewAction::None
    ));
    assert_eq!(external.stage, Stage::ExternalConsentChoice);
    let rendered = render_text(&external, 100, 20);
    assert!(rendered.contains("Managed (unavailable)"), "{rendered}");
}

#[test]
fn xai_auth_choice_uses_the_selected_locale() {
    let config = Config::default();
    let picker = ProviderPickerView::new_for_missing_auth(
        ApiProvider::Deepseek,
        ApiProvider::Xai,
        &config,
        None,
    )
    .expect("xAI has a picker row")
    .with_locale(crate::localization::Locale::ZhHans);

    let rendered = render_text(&picker, 100, 24);
    let compact = rendered
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    for translated in [
        "xAI身份验证",
        "请选择一个明确的凭据来源",
        "xAIAPI密钥",
        "原生设备OAuth",
    ] {
        assert!(compact.contains(translated), "{translated}: {rendered}");
    }
    assert!(!rendered.contains("Choose one explicit credential source"));
    assert!(!rendered.contains("Native device OAuth"));
}

#[test]
fn xai_auth_status_distinguishes_oauth_from_api_key_auth() {
    let oauth_config = crate::config::ProviderConfig {
        auth_mode: Some("oauth".to_string()),
        ..Default::default()
    };
    assert_eq!(
        xai_oauth_status(Some(&oauth_config), false),
        Some(ProviderAuthStatus::OAuthMissing)
    );
    assert_eq!(
        xai_oauth_status(Some(&oauth_config), true),
        Some(ProviderAuthStatus::OAuthReady)
    );
    assert_eq!(xai_oauth_status(None, true), None);
    assert_eq!(xai_oauth_status(None, false), None);

    let fallback_key = crate::config::ProviderConfig {
        auth_mode: Some("oauth".to_string()),
        api_key: Some("xai-api-key".to_string()),
        ..Default::default()
    };
    assert_eq!(
        xai_oauth_status(Some(&fallback_key), false),
        Some(ProviderAuthStatus::Configured)
    );
    for sentinel in [crate::config::API_KEYRING_SENTINEL, "  __KEYRING__  "] {
        let placeholder = crate::config::ProviderConfig {
            auth_mode: Some("oauth".to_string()),
            api_key: Some(sentinel.to_string()),
            ..Default::default()
        };
        assert_eq!(
            xai_oauth_status(Some(&placeholder), false),
            Some(ProviderAuthStatus::OAuthMissing)
        );
    }
}

#[test]
fn inactive_external_consents_are_visible_without_io_and_never_enter_routing_inventory() {
    let _env = crate::test_support::lock_test_env();
    let temp = tempfile::tempdir().expect("external consent fixtures");
    let codex_path = temp.path().join("codex-auth.json");
    let grok_path = temp.path().join("grok-auth.json");
    let codex_raw = "codex-external-file-must-not-be-read";
    let grok_raw = "grok-external-file-must-not-be-read";
    std::fs::write(&codex_path, codex_raw).expect("write Codex trap");
    std::fs::write(&grok_path, grok_raw).expect("write Grok trap");
    let owned_home = temp.path().join("codewhale-owned");

    let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &owned_home);
    let _codex_path = crate::test_support::EnvVarGuard::set("OPENAI_CODEX_AUTH_FILE", &codex_path);
    let _grok_path = crate::test_support::EnvVarGuard::set("GROK_AUTH_PATH", &grok_path);
    let _codex_access = crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
    let _legacy_codex_access = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
    let _xai_key = crate::test_support::EnvVarGuard::remove("XAI_API_KEY");
    let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");
    let _cli_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");

    let config = Config {
        provider: Some(ApiProvider::Deepseek.as_str().to_string()),
        providers: Some(crate::config::ProvidersConfig {
            openai_codex: crate::config::ProviderConfig {
                auth_mode: Some("oauth".to_string()),
                external_credentials: Some(
                    codewhale_config::ExternalCredentialConsentToml::read_only(
                        codewhale_config::ProviderKind::OpenaiCodex,
                        codewhale_config::ExternalCredentialSource::CodexCli,
                        codex_path.clone(),
                    ),
                ),
                ..Default::default()
            },
            xai: crate::config::ProviderConfig {
                auth_mode: Some("oauth".to_string()),
                external_credentials: Some(
                    codewhale_config::ExternalCredentialConsentToml::read_only(
                        codewhale_config::ProviderKind::Xai,
                        codewhale_config::ExternalCredentialSource::GrokCli,
                        grok_path.clone(),
                    ),
                ),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    crate::external_credentials::reset_side_effect_trap();
    assert!(!has_api_key_for(&config, ApiProvider::OpenaiCodex));
    assert!(!has_api_key_for(&config, ApiProvider::Xai));

    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    for provider in [ApiProvider::OpenaiCodex, ApiProvider::Xai] {
        let index = picker
            .rows
            .iter()
            .position(|row| row.provider == provider)
            .expect("consented provider row");
        let row = &picker.rows[index];
        assert_eq!(row.credential_state, CredentialState::ExternalConsent);
        assert_eq!(row.auth_status, ProviderAuthStatus::OAuthConsented);
        let structural = row
            .external_credential_status
            .as_ref()
            .expect("external status");
        assert_eq!(structural.access.as_str(), "read_only");
        assert_eq!(structural.route_state, "dormant");
        assert!(structural.revoke_command.contains(provider.as_str()));
        assert_eq!(
            row.readiness,
            ResolvedProviderReadiness::ExternalConsentPendingSelection
        );
        assert!(!row.readiness.can_attempt());
        picker.selected_idx = index;
        let visible = render_text(&picker, 140, 32);
        assert!(visible.contains("External: access=read_only"), "{visible}");
        assert!(visible.contains("Owner/path:"), "{visible}");
        assert!(
            visible.contains("revoke: codewhale auth external-revoke"),
            "{visible}"
        );
        assert!(
            picker.selected_has_key(),
            "selecting {provider:?} should activate the consented route before checking it"
        );
        assert!(matches!(
            picker.handle_key(key(KeyCode::Enter)),
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied {
                provider: selected,
                ..
            }) if selected == provider
        ));
    }
    assert!(matches!(
        picker.handle_key(key(KeyCode::Char('x'))),
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerExternalConsentRevoked {
            provider: ApiProvider::Xai
        })
    ));

    let inventory = crate::model_inventory::ModelInventory::from_config(&config);
    assert!(
        inventory.candidates.iter().all(|candidate| !matches!(
            candidate.provider,
            ApiProvider::OpenaiCodex | ApiProvider::Xai
        )),
        "dormant external-only routes must not reach auto-routing inventory"
    );
    assert_eq!(
        crate::route_billing::for_route(&config, ApiProvider::Xai),
        crate::route_billing::BillingPresentation::Metered
    );
    assert_eq!(
        crate::external_credentials::side_effect_trap_counts(),
        (0, 0),
        "picker, readiness, billing, and model inventory must not inspect inactive external files"
    );
    assert_eq!(
        std::fs::read_to_string(&codex_path).expect("Codex trap unchanged"),
        codex_raw
    );
    assert_eq!(
        std::fs::read_to_string(&grok_path).expect("Grok trap unchanged"),
        grok_raw
    );
    assert!(!owned_home.join("credentials/xai-auth.json").exists());
}

#[test]
fn kimi_cli_token_is_never_auto_enabled_without_explicit_legacy_auth_mode() {
    let _env = crate::test_support::lock_test_env();
    let temp = tempfile::tempdir().expect("Kimi import fixture root");
    let kimi_home = temp.path().join("kimi-code");
    std::fs::create_dir_all(kimi_home.join("credentials"))
        .expect("Kimi import credential directory");
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs_f64()
        + 3600.0;
    std::fs::write(
        kimi_home.join("credentials/kimi-code.json"),
        serde_json::json!({
            "access_token": "unexpired-user-owned-token",
            "refresh_token": "must-not-be-used",
            "expires_at": expires_at,
        })
        .to_string(),
    )
    .expect("write Kimi import fixture");
    let _kimi_home = crate::test_support::EnvVarGuard::set(
        "KIMI_CODE_HOME",
        kimi_home.to_str().expect("utf8 path"),
    );
    let _moonshot_key = crate::test_support::EnvVarGuard::remove("MOONSHOT_API_KEY");
    let _kimi_key = crate::test_support::EnvVarGuard::remove("KIMI_API_KEY");

    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &Config::default());
    move_to_provider(&mut picker, ApiProvider::Moonshot);
    let row = &picker.rows[picker.selected_idx];
    assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
    assert_eq!(row.credential_state, CredentialState::MissingKey);

    assert!(matches!(
        picker.handle_key(key(KeyCode::Enter)),
        ViewAction::None
    ));
    assert_eq!(
        picker.stage,
        Stage::KeyEntry,
        "a stray Kimi CLI credential must lead to API-key setup, not import activation"
    );
}

#[test]
fn explicit_legacy_kimi_import_is_unavailable_and_routes_to_api_key_setup() {
    let _env = crate::test_support::lock_test_env();
    let temp = tempfile::tempdir().expect("Kimi import fixture root");
    let kimi_home = temp.path().join("kimi-code");
    std::fs::create_dir_all(kimi_home.join("credentials"))
        .expect("Kimi import credential directory");
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs_f64()
        + 3600.0;
    std::fs::write(
        kimi_home.join("credentials/kimi-code.json"),
        serde_json::json!({
            "access_token": "unexpired-user-owned-token",
            "refresh_token": "must-not-be-used",
            "expires_at": expires_at,
        })
        .to_string(),
    )
    .expect("write Kimi import fixture");
    let _kimi_home = crate::test_support::EnvVarGuard::set(
        "KIMI_CODE_HOME",
        kimi_home.to_str().expect("utf8 path"),
    );
    let config = Config {
        providers: Some(crate::config::ProvidersConfig {
            moonshot: crate::config::ProviderConfig {
                auth_mode: Some("kimi_oauth".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Moonshot);
    let row = &picker.rows[picker.selected_idx];
    assert_eq!(
        row.auth_status,
        ProviderAuthStatus::ImportedTokenUnavailable
    );
    assert_eq!(row.credential_state, CredentialState::MissingKey);
    assert_eq!(row.base_url, crate::config::DEFAULT_KIMI_CODE_BASE_URL);
    assert_eq!(
        row.default_route.logical_model,
        crate::config::DEFAULT_KIMI_CODE_MODEL
    );
    assert_eq!(row.usage_meter, "usage: Kimi API key required");
    assert_eq!(row.readiness, ResolvedProviderReadiness::MissingKey);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Enter)),
        ViewAction::None
    ));
    assert_eq!(picker.stage, Stage::KeyEntry);
}

#[test]
fn key_entry_esc_returns_to_list_without_emitting() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Openrouter);
    picker.handle_key(key(KeyCode::Enter));
    assert_eq!(picker.stage, Stage::KeyEntry);
    picker.handle_key(key(KeyCode::Char('a')));
    let action = picker.handle_key(key(KeyCode::Esc));
    assert!(matches!(action, ViewAction::None));
    assert_eq!(picker.stage, Stage::List);
    assert!(picker.api_key_input.is_empty());
}

#[test]
fn list_esc_emits_dismiss_memory() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    let action = picker.handle_key(key(KeyCode::Esc));
    assert!(matches!(
        action,
        ViewAction::EmitAndClose(ViewEvent::ProviderPickerDismissed { .. })
    ));
}

#[test]
fn key_entry_strips_whitespace_chars() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Openrouter);
    picker.handle_key(key(KeyCode::Enter));
    assert_eq!(picker.stage, Stage::KeyEntry);
    for c in "abc def".chars() {
        picker.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(picker.api_key_input, "abcdef");
}

#[test]
fn small_list_render_keeps_selected_provider_visible_after_down_navigation() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    move_to_provider(&mut picker, ApiProvider::Ollama);

    let rendered = render_text(&picker, 80, 12);

    assert!(rendered.contains("Ollama"));
    assert!(!rendered.contains("DeepSeek *"));
}

#[test]
fn small_list_render_keeps_initial_active_provider_visible() {
    let config = Config::default();
    let picker = ProviderPickerView::new(ApiProvider::Ollama, &config);

    let rendered = render_text(&picker, 80, 12);

    assert!(rendered.contains("Ollama *"));
}

#[test]
fn tall_catalog_render_shows_selected_provider_details() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    // "All providers" means the full catalog (#3830), not just configured.
    picker.toggle_view();

    let rendered = render_text(&picker, 80, 23);

    assert!(rendered.contains("DeepSeek *"));
    assert!(rendered.contains("Details"));
    assert!(rendered.contains("Route:"));
}

/// The four terminal sizes the v0.8.66 modal blocker (#3732) requires every
/// overlay to remain readable and fully operable at.
const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

#[test]
fn provider_picker_is_usable_and_opaque_at_blocker_sizes() {
    use crate::tui::views::ViewStack;
    // Provider display names contain capital X/Q (Xiaomi MiMo, Qianfan), so
    // use a glyph that can never appear in the modal content as the
    // bleed-through sentinel.
    const SENTINEL: &str = "\u{2592}"; // ▒
    let config = Config::default();
    // Make the first provider in the sorted list active so its highlighted
    // row sits at the top of the list, never on the vertical center cell
    // that must read as the opaque modal ink.
    let active = ProviderPickerView::new(ApiProvider::Deepseek, &config).rows[0].provider;

    for (w, h) in BLOCKER_SIZES {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        for y in 0..h {
            for x in 0..w {
                buf[(x, y)].set_symbol(SENTINEL);
            }
        }
        // Render through the ViewStack so the shared opaque backdrop is
        // painted exactly as it is in production.
        let mut stack = ViewStack::new();
        stack.push(ProviderPickerView::new(active, &config));
        stack.render(area, &mut buf);

        let rows: Vec<String> = (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect();
        let text = rows.join("\n");

        // Footer keeps every action (it wraps instead of clipping).
        for label in ["move", "jump", "edit key", "models", "cancel"] {
            assert!(text.contains(label), "{w}x{h}: missing '{label}' hint");
        }
        // The Enter action label is dynamic (apply vs set key); one shows.
        assert!(
            text.contains("apply") || text.contains("set key"),
            "{w}x{h}: missing Enter action label"
        );
        // Composited frame is fully opaque: no sentinel survives and the
        // center cell carries the modal ink background.
        assert!(
            !text.contains(SENTINEL),
            "{w}x{h}: background bleed-through into modal surface"
        );
        assert_eq!(
            buf[(w / 2, h / 2)].bg,
            palette::WHALE_BG,
            "{w}x{h}: modal interior must be opaque"
        );
        // No row exceeds the frame width (no horizontal overflow).
        for (y, row) in rows.iter().enumerate() {
            assert!(
                unicode_width::UnicodeWidthStr::width(row.trim_end()) <= w as usize,
                "{w}x{h}: row {y} overflows width: {row:?}"
            );
        }
    }
}

#[test]
fn selected_provider_row_uses_strong_highlight() {
    let config = Config::default();
    let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    let area = Rect::new(0, 0, 80, 20);
    let mut buf = Buffer::empty(area);

    picker.render(area, &mut buf);

    let highlighted_cells = area
        .positions()
        .filter(|position| {
            let cell = &buf[*position];
            cell.bg == palette::SELECTION_BG
        })
        .count();
    assert!(
        highlighted_cells >= 32,
        "selected provider row should use a visible continuous highlight"
    );
}

#[test]
fn search_footer_shows_two_stage_esc_as_a_single_hint() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    picker.query = "deep".to_string();
    let area = Rect::new(0, 0, 100, 24);
    let mut buf = Buffer::empty(area);

    picker.render(area, &mut buf);

    let text = area
        .positions()
        .map(|position| buf[position].symbol())
        .collect::<String>();
    // The key appears once, with both stages spelled out in its label.
    assert_eq!(
        text.matches(" Esc ").count(),
        1,
        "search footer must not duplicate the Esc key: {text}"
    );
    assert!(text.contains("clear / cancel"), "{text}");
}

#[test]
fn esc_reports_browsing_context_and_reopen_restores_it() {
    let config = Config::default();
    let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
    // Browse full catalog and move highlight.
    picker.handle_key(key(KeyCode::Char('a')));
    picker.handle_key(key(KeyCode::Down));
    let remembered_id = picker.rows[picker.selected_idx].provider_id.clone();
    let action = picker.handle_key(key(KeyCode::Esc));
    let ViewAction::EmitAndClose(ViewEvent::ProviderPickerDismissed {
        catalog_view,
        selected_provider_id,
    }) = action
    else {
        panic!("expected ProviderPickerDismissed");
    };
    assert!(catalog_view);
    assert_eq!(
        selected_provider_id.as_deref(),
        Some(remembered_id.as_str())
    );

    let memory = crate::tui::app::ProviderPickerMemory {
        catalog_view,
        selected_provider_id,
    };
    let reopened = ProviderPickerView::new_with_runtime_status_and_memory(
        ApiProvider::Deepseek,
        &config,
        None,
        Some(&memory),
    );
    assert_eq!(reopened.view, ProviderListView::Catalog);
    assert_eq!(
        reopened.rows[reopened.selected_idx].provider_id,
        remembered_id
    );
}
