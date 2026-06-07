//! Command registry metadata and lookup helpers.

use crate::localization::{Locale, MessageId, tr};

/// Command metadata for help and autocomplete.
///
/// The English description lives in `localization::english` (private), keyed
/// by `description_id`. Callers resolve a localized description through
/// [`CommandInfo::description_for`] which delegates to
/// [`crate::localization::tr`].
#[derive(Debug, Clone, Copy)]
pub struct CommandInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub description_id: MessageId,
}

impl CommandInfo {
    pub fn requires_argument(&self) -> bool {
        self.usage.contains('<') || self.usage.contains('[')
    }

    pub fn palette_command(&self) -> String {
        if self.requires_argument() {
            format!("/{} ", self.name)
        } else {
            format!("/{}", self.name)
        }
    }

    pub fn description_for(&self, locale: Locale) -> &'static str {
        tr(locale, self.description_id)
    }

    pub fn palette_description_for(&self, locale: Locale) -> String {
        let desc = self.description_for(locale);
        if self.aliases.is_empty() {
            desc.to_string()
        } else {
            format!("{}  aliases: {}", desc, self.aliases.join(", "))
        }
    }
}

/// All registered commands
pub const COMMANDS: &[CommandInfo] = &[
    // Core commands
    CommandInfo {
        name: "anchor",
        aliases: &["maodian"],
        usage: "/anchor <text> | /anchor list | /anchor remove <n>",
        description_id: MessageId::CmdAnchorDescription,
    },
    CommandInfo {
        name: "help",
        aliases: &["?", "bangzhu", "帮助"],
        usage: "/help [command]",
        description_id: MessageId::CmdHelpDescription,
    },
    CommandInfo {
        name: "clear",
        aliases: &["qingping"],
        usage: "/clear",
        description_id: MessageId::CmdClearDescription,
    },
    CommandInfo {
        name: "exit",
        aliases: &["quit", "q", "tuichu"],
        usage: "/exit",
        description_id: MessageId::CmdExitDescription,
    },
    CommandInfo {
        name: "model",
        aliases: &["moxing"],
        usage: "/model [name]",
        description_id: MessageId::CmdModelDescription,
    },
    CommandInfo {
        name: "models",
        aliases: &["moxingliebiao"],
        usage: "/models",
        description_id: MessageId::CmdModelsDescription,
    },
    CommandInfo {
        name: "provider",
        aliases: &[],
        usage: "/provider [name] [model]",
        description_id: MessageId::CmdProviderDescription,
    },
    CommandInfo {
        name: "queue",
        aliases: &["queued"],
        usage: "/queue [list|edit <n>|drop <n>|clear]",
        description_id: MessageId::CmdQueueDescription,
    },
    CommandInfo {
        name: "stash",
        aliases: &["park"],
        usage: "/stash [list|pop|clear]",
        description_id: MessageId::CmdStashDescription,
    },
    CommandInfo {
        name: "hooks",
        aliases: &["hook", "gouzi"],
        usage: "/hooks [list|events]",
        description_id: MessageId::CmdHooksDescription,
    },
    CommandInfo {
        name: "subagents",
        aliases: &["agents", "zhinengti"],
        usage: "/subagents",
        description_id: MessageId::CmdSubagentsDescription,
    },
    CommandInfo {
        name: "agent",
        aliases: &["daili"],
        usage: "/agent [N] <task>",
        description_id: MessageId::CmdAgentDescription,
    },
    CommandInfo {
        name: "links",
        aliases: &["dashboard", "api", "lianjie"],
        usage: "/links",
        description_id: MessageId::CmdLinksDescription,
    },
    CommandInfo {
        name: "feedback",
        aliases: &[],
        usage: "/feedback [bug|feature|security]",
        description_id: MessageId::CmdFeedbackDescription,
    },
    CommandInfo {
        name: "hf",
        aliases: &["huggingface"],
        usage: "/hf [mcp <status|setup>|concepts]",
        description_id: MessageId::CmdHfDescription,
    },
    CommandInfo {
        name: "home",
        aliases: &["stats", "overview", "zhuye", "shouye"],
        usage: "/home",
        description_id: MessageId::CmdHomeDescription,
    },
    CommandInfo {
        name: "workspace",
        aliases: &["cwd"],
        usage: "/workspace [path]",
        description_id: MessageId::CmdWorkspaceDescription,
    },
    CommandInfo {
        name: "note",
        aliases: &[],
        usage: "/note [add|list|show|edit|remove|clear|path]",
        description_id: MessageId::CmdNoteDescription,
    },
    CommandInfo {
        name: "memory",
        aliases: &[],
        usage: "/memory [show|path|clear|edit|help]",
        description_id: MessageId::CmdMemoryDescription,
    },
    CommandInfo {
        name: "attach",
        aliases: &["image", "media", "fujian"],
        usage: "/attach <path>",
        description_id: MessageId::CmdAttachDescription,
    },
    CommandInfo {
        name: "task",
        aliases: &["tasks"],
        usage: "/task [add <prompt>|list|show <id>|cancel <id>]",
        description_id: MessageId::CmdTaskDescription,
    },
    CommandInfo {
        name: "jobs",
        aliases: &["job", "zuoye"],
        usage: "/jobs [list|show <id>|poll <id>|wait <id>|stdin <id> <input>|cancel <id>]",
        description_id: MessageId::CmdJobsDescription,
    },
    CommandInfo {
        name: "mcp",
        aliases: &[],
        usage: "/mcp [init|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>|validate|reload]",
        description_id: MessageId::CmdMcpDescription,
    },
    CommandInfo {
        name: "network",
        aliases: &[],
        usage: "/network [list|allow <host>|deny <host>|remove <host>|default <allow|deny|prompt>]",
        description_id: MessageId::CmdNetworkDescription,
    },
    // Session commands
    CommandInfo {
        name: "rename",
        aliases: &["gaiming", "chongmingming"],
        usage: "/rename <new title>",
        description_id: MessageId::CmdRenameDescription,
    },
    CommandInfo {
        name: "save",
        aliases: &[],
        usage: "/save [path]",
        description_id: MessageId::CmdSaveDescription,
    },
    CommandInfo {
        name: "fork",
        aliases: &["branch"],
        usage: "/fork",
        description_id: MessageId::CmdForkDescription,
    },
    CommandInfo {
        name: "new",
        aliases: &[],
        usage: "/new [--force]",
        description_id: MessageId::CmdNewDescription,
    },
    CommandInfo {
        name: "sessions",
        aliases: &["resume"],
        usage: "/sessions [show|prune <days>]",
        description_id: MessageId::CmdSessionsDescription,
    },
    CommandInfo {
        name: "load",
        aliases: &["jiazai"],
        usage: "/load [path]",
        description_id: MessageId::CmdLoadDescription,
    },
    CommandInfo {
        name: "compact",
        aliases: &["yasuo"],
        usage: "/compact",
        description_id: MessageId::CmdCompactDescription,
    },
    CommandInfo {
        name: "purge",
        aliases: &["qingchu"],
        usage: "/purge",
        description_id: MessageId::CmdPurgeDescription,
    },
    CommandInfo {
        name: "relay",
        aliases: &["batonpass", "接力"],
        usage: "/relay [focus]",
        description_id: MessageId::CmdRelayDescription,
    },
    CommandInfo {
        name: "context",
        aliases: &["ctx"],
        usage: "/context",
        description_id: MessageId::CmdContextDescription,
    },
    CommandInfo {
        name: "export",
        aliases: &["daochu"],
        usage: "/export [path]",
        description_id: MessageId::CmdExportDescription,
    },
    // Config commands
    CommandInfo {
        name: "config",
        aliases: &[],
        usage: "/config",
        description_id: MessageId::CmdConfigDescription,
    },
    CommandInfo {
        name: "sidebar",
        aliases: &[],
        usage: "/sidebar [on|off|auto|work|tasks|agents|context] [--save]",
        description_id: MessageId::CmdSidebarDescription,
    },
    CommandInfo {
        name: "mode",
        aliases: &["jihua", "zidong"],
        usage: "/mode [agent|plan|yolo|1|2|3]",
        description_id: MessageId::CmdModeDescription,
    },
    CommandInfo {
        name: "theme",
        aliases: &[],
        usage: "/theme [name]",
        description_id: MessageId::CmdThemeDescription,
    },
    CommandInfo {
        name: "verbose",
        aliases: &[],
        usage: "/verbose [on|off]",
        description_id: MessageId::CmdVerboseDescription,
    },
    CommandInfo {
        name: "trust",
        aliases: &["xinren"],
        usage: "/trust [on|off|add <path>|remove <path>|list]",
        description_id: MessageId::CmdTrustDescription,
    },
    CommandInfo {
        name: "logout",
        aliases: &[],
        usage: "/logout",
        description_id: MessageId::CmdLogoutDescription,
    },
    // Debug commands
    CommandInfo {
        name: "tokens",
        aliases: &[],
        usage: "/tokens",
        description_id: MessageId::CmdTokensDescription,
    },
    CommandInfo {
        name: "translate",
        aliases: &["translation", "transale"],
        usage: "/translate",
        description_id: MessageId::CmdTranslateDescription,
    },
    CommandInfo {
        name: "system",
        aliases: &["xitong"],
        usage: "/system",
        description_id: MessageId::CmdSystemDescription,
    },
    CommandInfo {
        name: "edit",
        aliases: &[],
        usage: "/edit",
        description_id: MessageId::CmdEditDescription,
    },
    CommandInfo {
        name: "diff",
        aliases: &[],
        usage: "/diff",
        description_id: MessageId::CmdDiffDescription,
    },
    CommandInfo {
        name: "change",
        aliases: &[],
        usage: "/change [version]",
        description_id: MessageId::CmdChangeDescription,
    },
    CommandInfo {
        name: "undo",
        aliases: &[],
        usage: "/undo",
        description_id: MessageId::CmdUndoDescription,
    },
    CommandInfo {
        name: "retry",
        aliases: &["chongshi"],
        usage: "/retry",
        description_id: MessageId::CmdRetryDescription,
    },
    CommandInfo {
        name: "init",
        aliases: &[],
        usage: "/init",
        description_id: MessageId::CmdInitDescription,
    },
    CommandInfo {
        name: "lsp",
        aliases: &[],
        usage: "/lsp [on|off|status]",
        description_id: MessageId::CmdLspDescription,
    },
    CommandInfo {
        name: "share",
        aliases: &[],
        usage: "/share",
        description_id: MessageId::CmdShareDescription,
    },
    CommandInfo {
        name: "hunt",
        aliases: &["goal", "mubiao", "狩猎"],
        usage: "/hunt [quarry] [budget: N]",
        description_id: MessageId::CmdGoalDescription,
    },
    CommandInfo {
        name: "settings",
        aliases: &[],
        usage: "/settings",
        description_id: MessageId::CmdSettingsDescription,
    },
    CommandInfo {
        name: "status",
        aliases: &[],
        usage: "/status",
        description_id: MessageId::CmdStatusDescription,
    },
    CommandInfo {
        name: "statusline",
        aliases: &[],
        usage: "/statusline",
        description_id: MessageId::CmdStatuslineDescription,
    },
    // Skills commands
    CommandInfo {
        name: "skills",
        aliases: &["jinengliebiao"],
        usage: "/skills [--remote|sync|<prefix>]",
        description_id: MessageId::CmdSkillsDescription,
    },
    CommandInfo {
        name: "skill",
        aliases: &["jineng"],
        usage: "/skill <name|install <spec>|update <name>|uninstall <name>|trust <name>>",
        description_id: MessageId::CmdSkillDescription,
    },
    CommandInfo {
        name: "review",
        aliases: &["shencha"],
        usage: "/review <target>",
        description_id: MessageId::CmdReviewDescription,
    },
    CommandInfo {
        name: "restore",
        aliases: &[],
        usage: "/restore [N|list [N]]",
        description_id: MessageId::CmdRestoreDescription,
    },
    // RLM command
    CommandInfo {
        name: "rlm",
        aliases: &["recursive", "digui"],
        usage: "/rlm [N] <file_or_text>",
        description_id: MessageId::CmdRlmDescription,
    },
    // Debug/cost command
    CommandInfo {
        name: "cost",
        aliases: &[],
        usage: "/cost",
        description_id: MessageId::CmdCostDescription,
    },
    // Balance query (#2019)
    CommandInfo {
        name: "balance",
        aliases: &[],
        usage: "/balance",
        description_id: MessageId::CmdBalanceDescription,
    },
    // Profile switching (#390)
    CommandInfo {
        name: "profile",
        aliases: &["dangan"],
        usage: "/profile <name>",
        description_id: MessageId::CmdHelpDescription, // reuse for now
    },
    // Cache telemetry (#263)
    CommandInfo {
        name: "cache",
        aliases: &[],
        usage: "/cache [count|inspect|stats|zones|warmup]",
        description_id: MessageId::CmdCacheDescription,
    },
    // Slop Ledger (#2127)
    CommandInfo {
        name: "slop",
        aliases: &["canzha"],
        usage: "/slop [query|export]",
        description_id: MessageId::CmdSlopDescription,
    },
];

/// Get command info by name or alias
pub fn get_command_info(name: &str) -> Option<&'static CommandInfo> {
    let name = name.strip_prefix('/').unwrap_or(name);
    COMMANDS
        .iter()
        .find(|cmd| cmd.name == name || cmd.aliases.contains(&name))
}

fn edit_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.chars().count();
    }
    if b.is_empty() {
        return a.chars().count();
    }

    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0usize; b_chars.len() + 1];

    for (i, a_ch) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b_chars.iter().enumerate() {
            let cost = if a_ch == *b_ch { 0 } else { 1 };
            let delete = prev[j + 1] + 1;
            let insert = curr[j] + 1;
            let substitute = prev[j] + cost;
            curr[j + 1] = delete.min(insert).min(substitute);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_chars.len()]
}

pub(super) fn suggest_command_names(input: &str, limit: usize) -> Vec<String> {
    let query = input.trim().to_ascii_lowercase();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut scored: Vec<(u8, usize, String)> = Vec::new();
    for command in COMMANDS {
        let mut best: Option<(u8, usize)> = None;
        for candidate in std::iter::once(command.name).chain(command.aliases.iter().copied()) {
            let candidate = candidate.to_ascii_lowercase();
            let prefix_match = candidate.starts_with(&query) || query.starts_with(&candidate);
            let contains_match = candidate.contains(&query) || query.contains(&candidate);
            let distance = edit_distance(&candidate, &query);
            let close_typo = distance <= 2;
            if !(prefix_match || contains_match || close_typo) {
                continue;
            }

            let rank = if prefix_match {
                0
            } else if contains_match {
                1
            } else {
                2
            };

            match best {
                Some((best_rank, best_distance))
                    if rank > best_rank || (rank == best_rank && distance >= best_distance) => {}
                _ => best = Some((rank, distance)),
            }
        }

        if let Some((rank, distance)) = best {
            scored.push((rank, distance, command.name.to_string()));
        }
    }

    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, name)| name)
        .collect()
}
