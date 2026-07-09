use crate::localization::{Locale, MessageId, tr};
use crate::tui::app::App;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SetupRemoteFacts {
    pub(super) clouds_result: String,
    pub(super) bridges_result: String,
    pub(super) providers_result: String,
    pub(super) mode_result: String,
    pub(super) command_provider: String,
    pub(super) result: String,
}

impl SetupRemoteFacts {
    pub(super) fn from_app(app: &App) -> Self {
        let cloud_slugs = crate::remote_setup::registry::CLOUD_TARGETS
            .iter()
            .map(|cloud| cloud.slug)
            .collect::<Vec<_>>();
        let bridge_slugs = crate::remote_setup::registry::BRIDGES
            .iter()
            .map(|bridge| bridge.slug)
            .collect::<Vec<_>>();
        let provider_count = codewhale_config::ProviderKind::all().len();
        let command_provider =
            crate::remote_setup::bundle::ProviderInfo::from_slug(app.api_provider.as_str())
                .map_or_else(|| "deepseek".to_string(), |provider| provider.slug);

        Self {
            clouds_result: format!(
                "{} cloud targets: {}",
                cloud_slugs.len(),
                cloud_slugs.join(", ")
            ),
            bridges_result: format!(
                "{} chat bridges: {}",
                bridge_slugs.len(),
                bridge_slugs.join(", ")
            ),
            providers_result: format!(
                "{provider_count} providers from the provider registry; active route {} / {}",
                app.api_provider.as_str(),
                app.model
            ),
            mode_result: format!(
                "generate-only bundle; --apply not implemented; default port {}, workers {}",
                crate::remote_setup::bundle::DEFAULT_PORT,
                crate::remote_setup::bundle::DEFAULT_WORKERS
            ),
            command_provider,
            result: format!(
                "clouds={}, bridges={}, providers={}, mode=generate_only, apply=not_implemented",
                cloud_slugs.len(),
                bridge_slugs.len(),
                provider_count
            ),
        }
    }
}

pub(super) fn on_ramp_text(
    locale: Locale,
    clouds_result: &str,
    bridges_result: &str,
    providers_result: &str,
    mode_result: &str,
    command_provider: &str,
) -> String {
    let command = format!(
        "codewhale remote-setup --generate-only --cloud lighthouse --bridge telegram --provider {command_provider} --out ./codewhale-deploy/lighthouse-telegram"
    );
    let base = tr(locale, MessageId::SetupRemoteOnRampText);
    base.replace("{clouds_result}", clouds_result)
        .replace("{bridges_result}", bridges_result)
        .replace("{providers_result}", providers_result)
        .replace("{mode_result}", mode_result)
        .replace("{command}", &command)
}
