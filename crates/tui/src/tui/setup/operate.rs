use crate::config::Config;
use crate::tui::app::App;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SetupOperateFacts {
    pub(super) runtime_ready: bool,
    pub(super) runtime_result: String,
    pub(super) roster_ready: bool,
    pub(super) roster_result: String,
    pub(super) concurrency_result: String,
    pub(super) result: String,
}

impl Default for SetupOperateFacts {
    fn default() -> Self {
        Self {
            runtime_ready: false,
            runtime_result: "worker runtime not loaded".to_string(),
            roster_ready: false,
            roster_result: "Fleet roster not loaded".to_string(),
            concurrency_result: "concurrency not loaded".to_string(),
            result: "operate readiness not loaded".to_string(),
        }
    }
}

impl SetupOperateFacts {
    pub(super) fn from_app_config(app: &App, config: &Config, provider_ready: bool) -> Self {
        let subagents_enabled = config.subagents_enabled_for_provider(app.api_provider);
        let max_subagents = config.max_subagents_for_provider(app.api_provider);
        let launch_concurrency = config.launch_concurrency_for_provider(app.api_provider);
        let max_admitted = config.max_admitted_subagents_for_provider(app.api_provider);
        let runtime_disabled_reason = if subagents_enabled {
            None
        } else {
            Some(
                config
                    .subagents_disabled_reason()
                    .unwrap_or("disabled for active provider"),
            )
        };
        let runtime_ready = subagents_enabled && max_subagents > 0 && launch_concurrency > 0;
        let runtime_result = if let Some(reason) = runtime_disabled_reason {
            format!("worker runtime disabled ({reason})")
        } else {
            format!(
                "worker runtime enabled for {}; max_subagents={}, launch_concurrency={}, admission={}",
                app.api_provider.as_str(),
                max_subagents,
                launch_concurrency,
                max_admitted
            )
        };

        let roster =
            crate::fleet::roster::FleetRoster::load(&config.fleet_config(), &app.workspace);
        let roster_members = roster.members().len();
        let custom_roster_members = roster
            .members()
            .iter()
            .filter(|member| !matches!(member.origin, crate::fleet::roster::ProfileOrigin::BuiltIn))
            .count();
        let roster_ready = roster_members > 0;
        let roster_result = if custom_roster_members > 0 {
            format!("{roster_members} Fleet members ({custom_roster_members} config/workspace)")
        } else {
            format!("{roster_members} built-in Fleet members; starter roster available")
        };

        let concurrency_result = format!(
            "configured launch_concurrency={launch_concurrency}; max_subagents={max_subagents}; admission={max_admitted}; plan limit not probed"
        );
        let result = format!(
            "provider={}, runtime={}, roster={}, concurrency={}",
            if provider_ready {
                "ready"
            } else {
                "needs_action"
            },
            if runtime_ready {
                "ready"
            } else {
                "needs_action"
            },
            if roster_ready {
                "ready"
            } else {
                "needs_action"
            },
            concurrency_result
        );

        Self {
            runtime_ready,
            runtime_result,
            roster_ready,
            roster_result,
            concurrency_result,
            result,
        }
    }
}
