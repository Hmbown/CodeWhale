export default workflow({
  "id": "v0868-catalog-lane",
  "goal": "Complete v0.8.68 model catalog consolidation (Section 1)",
  "description": "Parallel scouts for catalog gaps, then sequential implementation of live refresh, picker views, and legacy source migration. DEFERRED until stopship (#4090, #4093, #4094) is green — do not start unless explicitly unblocked.",
  "nodes": [
    {
      "branch": {
        "id": "scout-catalog",
        "parallel": true,
        "children": [
          {
            "agent": {
              "id": "scout-openrouter",
              "prompt": "Audit OpenRouter live catalog integration for v0.8.68 (#4109, #4114). Read `crates/tui/src/client.rs` (`fetch_catalog_delta`, `refresh_catalog_cache`, `parse_openrouter_models_response`) and `crates/config/src/catalog.rs`. Compare against issue bodies: `gh issue view 4114 -R Hmbown/CodeWhale`. Report: fields preserved vs dropped, sort order handling, TTL/backoff gaps. Output SUMMARY + EVIDENCE (path:line) + effort S/M/L.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/client.rs", "crates/config/src/catalog.rs", "crates/tui/src/provider_lake.rs"],
              "budget": { "max_steps": 12, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "scout-picker-views",
              "prompt": "Audit model picker UX for v0.8.68 (#4115, #4139, #4140, #4141). Read `crates/tui/src/tui/model_picker.rs` and `provider_picker.rs`. Check ModelListView modes, configured vs catalog toggle, cross-field search, metadata rows. Issues: `gh issue view 4115 4139 4140 4141 -R Hmbown/CodeWhale`. Report done/partial/missing per acceptance criterion.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/tui/model_picker.rs", "crates/tui/src/tui/provider_picker.rs"],
              "budget": { "max_steps": 12, "timeout_secs": 600 }
            }
          },
          {
            "agent": {
              "id": "scout-legacy-sources",
              "prompt": "Find remaining `model_completion_names_for_provider` consumers (#4116). Run `git grep -n model_completion_names_for_provider -- crates/`. For each call site (picker, hotbar, inventory, subagent, widgets), report whether it uses ConfiguredProviderLake or legacy table. Map to issue #4116 acceptance. Read-only.",
              "agent_type": "explore",
              "mode": "read_only",
              "file_scope": ["crates/tui/src/"],
              "budget": { "max_steps": 10, "timeout_secs": 600 }
            }
          }
        ]
      }
    },
    {
      "sequence": {
        "id": "implement-catalog",
        "children": [
          {
            "agent": {
              "id": "impl-refresh-policy",
              "prompt": "Implement catalog refresh policy (#4114) per scout-openrouter findings: manual `/model refresh` or `/provider refresh`, background TTL refresh, stale chip, fail-closed on auth errors, secret-free cache persistence. Touch `client.rs` and `catalog.rs`. Add tests. Run `cargo test -p codewhale-config catalog` and `cargo test -p codewhale-tui provider_lake`.",
              "agent_type": "implementer",
              "mode": "write",
              "file_scope": ["crates/tui/src/client.rs", "crates/config/src/catalog.rs"],
              "budget": { "max_steps": 24, "timeout_secs": 1800 }
            }
          },
          {
            "agent": {
              "id": "impl-picker-views",
              "prompt": "Implement model picker catalog views (#4115, #4139-4141) per scout-picker-views: Configured/Catalog/Recent/Coding/Cheap/Long-context views, metadata rows, cross-field search on provider+model. Minimal diff matching existing picker patterns. Tests in `cargo test -p codewhale-tui model_picker`.",
              "agent_type": "implementer",
              "mode": "write",
              "file_scope": ["crates/tui/src/tui/model_picker.rs", "crates/tui/src/tui/provider_picker.rs"],
              "budget": { "max_steps": 24, "timeout_secs": 1800 }
            }
          },
          {
            "agent": {
              "id": "impl-migrate-consumers",
              "prompt": "Migrate remaining legacy model source consumers to ProviderLake (#4116). Replace `model_completion_names_for_provider` at each scout-legacy-sources call site. Keep bundled fallback for unconfigured providers. Run `cargo test -p codewhale-tui model_inventory hotbar subagent`.",
              "agent_type": "implementer",
              "mode": "write",
              "file_scope": ["crates/tui/src/tui/hotbar/", "crates/tui/src/tools/subagent/mod.rs"],
              "budget": { "max_steps": 20, "timeout_secs": 1200 }
            }
          }
        ]
      }
    },
    {
      "reduce": {
        "id": "catalog-handoff",
        "inputs": ["scout-openrouter", "scout-picker-views", "scout-legacy-sources", "impl-refresh-policy", "impl-picker-views", "impl-migrate-consumers"],
        "prompt": "Synthesize catalog lane.\n\n## SECTION 1 STATUS (1.1-1.5)\n| Item | Issue | Status | Evidence |\n\n## TESTS\n\n## DEFERRED TO 0.9.0\n\n## READY FOR WORKFLOW UI LANE?\nyes/no + why"
      }
    }
  ]
});
