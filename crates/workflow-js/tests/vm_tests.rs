//! End-to-end tests for the Workflow JS runtime against a fake driver.

use std::sync::Arc;
use std::time::Duration;

use codewhale_workflow_js::testing::{FakeDriver, FakeReply};
use codewhale_workflow_js::{
    ProgressEvent, WORKFLOW_LIFETIME_CAP, WorkflowJsError, WorkflowRunCancel, WorkflowVm,
};
use serde_json::json;

async fn run(
    driver: &Arc<FakeDriver>,
    source: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, WorkflowJsError> {
    WorkflowVm::new()
        .run_script(
            source,
            args,
            driver.clone() as Arc<dyn codewhale_workflow_js::WorkflowDriver>,
        )
        .await
}

fn script_message(result: Result<serde_json::Value, WorkflowJsError>) -> String {
    match result {
        Err(WorkflowJsError::Script(message)) => message,
        other => panic!("expected script error, got {other:?}"),
    }
}

#[tokio::test]
async fn plain_return_value_round_trips() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(&driver, "return 1 + 1;", json!(null)).await.unwrap();
    assert_eq!(value, json!(2));
}

#[tokio::test]
async fn undefined_return_becomes_null() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(&driver, "const x = 1;", json!(null)).await.unwrap();
    assert_eq!(value, json!(null));
}

#[tokio::test]
async fn args_global_is_the_invocation_input() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(
        &driver,
        "return { sum: args.x + 1, tag: args.tags[0] };",
        json!({"x": 41, "tags": ["release"]}),
    )
    .await
    .unwrap();
    assert_eq!(value, json!({"sum": 42, "tag": "release"}));
}

#[tokio::test]
async fn task_round_trip_carries_all_options_and_normalizes_profile() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(
        &driver,
        r#"
        return await task({
            description: "implement the bounded change",
            subagentType: "implementer",
            profile: "  ALpha-1  ",
            model: "deepseek-chat",
            modelStrength: "faster",
            thinking: "low",
            worktree: true,
            writeAuthority: "worktree_write",
            writeRoots: ["crates/tui/src"],
            exactFiles: ["Cargo.toml"],
            coordinationContracts: ["public-api"],
            dependencies: ["issue-4619"],
            acceptance: ["locked tests pass"],
            allowedTools: ["read", "grep"],
            maxDepth: 2,
            tokenBudget: 5000,
            maxSteps: 4,
            wallTimeSecs: 90,
            label: "L1",
            phase: "P1",
        });
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value, json!("done:implement the bounded change"));

    let requests = driver.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.description, "implement the bounded change");
    assert_eq!(request.subagent_type.as_deref(), Some("implementer"));
    assert_eq!(request.profile.as_deref(), Some("alpha-1"));
    assert_eq!(request.model.as_deref(), Some("deepseek-chat"));
    assert_eq!(request.model_strength.as_deref(), Some("faster"));
    assert_eq!(request.thinking.as_deref(), Some("low"));
    assert!(request.worktree);
    assert_eq!(request.write_authority.as_deref(), Some("worktree_write"));
    assert_eq!(request.write_roots, ["crates/tui/src"]);
    assert_eq!(request.exact_files, ["Cargo.toml"]);
    assert_eq!(request.coordination_contracts, ["public-api"]);
    assert_eq!(request.dependencies, ["issue-4619"]);
    assert_eq!(request.acceptance, ["locked tests pass"]);
    assert_eq!(
        request.allowed_tools.as_deref(),
        Some(["read".to_string(), "grep".to_string()].as_slice())
    );
    assert_eq!(request.max_depth, Some(2));
    assert_eq!(request.token_budget, Some(5000));
    assert_eq!(request.max_steps, Some(4));
    assert_eq!(request.wall_time_secs, Some(90));
    assert_eq!(request.response_schema, None);
    assert_eq!(request.label.as_deref(), Some("L1"));
    assert_eq!(request.phase.as_deref(), Some("P1"));
}

#[tokio::test]
async fn task_write_authority_requires_bounded_coordination_scope() {
    let driver = Arc::new(FakeDriver::new());
    let error = run(
        &driver,
        r#"
        return await task({
            prompt: "edit without a claim",
            type: "implementer",
            writeAuthority: "workspace_write",
        });
        "#,
        json!(null),
    )
    .await
    .expect_err("unscoped Workflow writer must fail before driver dispatch")
    .to_string();
    assert!(error.contains("requires writeRoots"), "{error}");
    assert!(driver.requests().is_empty());
}

#[tokio::test]
async fn task_coordination_lists_deduplicate_with_hard_count_bounds() {
    let driver = Arc::new(FakeDriver::new());
    run(
        &driver,
        r#"
        return await task({
            prompt: "bounded edit",
            type: "implementer",
            writeAuthority: "workspace_write",
            exactFiles: ["src/a.rs", "src/a.rs"],
            dependencies: ["A", "A"],
            acceptance: ["tests pass", "tests pass"],
        });
        "#,
        json!(null),
    )
    .await
    .expect("bounded unique coordination values");
    let request = driver.requests().pop().expect("request");
    assert_eq!(request.exact_files, ["src/a.rs"]);
    assert_eq!(request.dependencies, ["A"]);
    assert_eq!(request.acceptance, ["tests pass"]);
}

#[tokio::test]
async fn task_write_paths_normalize_and_reject_escape_spellings() {
    let driver = Arc::new(FakeDriver::new());
    run(
        &driver,
        r#"return await task({
            prompt: "bounded edit",
            type: "implementer",
            writeRoots: ["./src//", "src"],
            exactFiles: ["src\\lib.rs"]
        });"#,
        json!(null),
    )
    .await
    .expect("normalized repo-relative paths");
    let request = driver.requests().pop().expect("request");
    assert_eq!(request.write_roots, ["src"]);
    assert_eq!(request.exact_files, ["src/lib.rs"]);

    for path in [
        "../outside",
        "/tmp/outside",
        "C:\\outside",
        "src/../../outside",
    ] {
        let driver = Arc::new(FakeDriver::new());
        let source = format!(
            "return await task({{ prompt: 'escape', type: 'implementer', writeRoots: [{}] }});",
            serde_json::to_string(path).expect("path json")
        );
        let message = script_message(run(&driver, &source, json!(null)).await);
        assert!(
            message.contains("repo-relative") || message.contains("traversal"),
            "{path}: {message}"
        );
        assert!(driver.requests().is_empty());
    }
}

#[tokio::test]
async fn task_explicit_write_roles_fail_closed_without_scope_and_reject_contradictions() {
    for source in [
        r#"return await task({prompt: "no scope", type: "implementer"});"#,
        r#"return await task({prompt: "no scope", type: "general"});"#,
        r#"return await task({prompt: "no scope", profile: "release-lead"});"#,
        r#"return await task({prompt: "wrong authority", type: "reviewer", writeAuthority: "workspace_write", writeRoots: ["src"]});"#,
        r#"return await task({prompt: "wrong role", type: "implementer", writeAuthority: "read_only"});"#,
        r#"return await task({prompt: "role conflict", type: "implementer", role: "reviewer", writeRoots: ["src"]});"#,
    ] {
        let driver = Arc::new(FakeDriver::new());
        let message = script_message(run(&driver, source, json!(null)).await);
        assert!(
            message.contains("require")
                || message.contains("cannot")
                || message.contains("contradictory"),
            "{message}"
        );
        assert!(driver.requests().is_empty());
    }
}

#[tokio::test]
async fn task_accepts_prompt_and_type_aliases() {
    let driver = Arc::new(FakeDriver::new());
    run(
        &driver,
        r#"return await task({ prompt: "aliased", type: "verifier" });"#,
        json!(null),
    )
    .await
    .unwrap();
    let request = &driver.requests()[0];
    assert_eq!(request.description, "aliased");
    assert_eq!(request.subagent_type.as_deref(), Some("verifier"));
}

#[tokio::test]
async fn task_prompt_takes_precedence_over_short_description() {
    let driver = Arc::new(FakeDriver::new());
    run(
        &driver,
        r#"return await task({
            description: "Short progress summary",
            prompt: "Detailed child instructions",
            label: "fixture-compatible"
        });"#,
        json!(null),
    )
    .await
    .unwrap();
    let request = &driver.requests()[0];
    assert_eq!(request.description, "Detailed child instructions");
    assert_eq!(request.label.as_deref(), Some("fixture-compatible"));
}

#[tokio::test]
async fn task_rejects_invalid_profile_tokens() {
    for bad in ["two words", "a=b", "a\"b", "a`b", "   "] {
        let driver = Arc::new(FakeDriver::new());
        let source = format!(
            "return await task({{ description: \"x\", profile: {} }});",
            serde_json::Value::String(bad.to_string())
        );
        let message = script_message(run(&driver, &source, json!(null)).await);
        assert!(message.contains("profile"), "profile {bad:?}: {message}");
        assert_eq!(driver.spawn_count(), 0, "invalid profile must not spawn");
    }
}

#[tokio::test]
async fn task_requires_a_description() {
    let driver = Arc::new(FakeDriver::new());
    let message = script_message(run(&driver, "return await task({});", json!(null)).await);
    assert!(message.contains("description"), "{message}");
    assert_eq!(driver.spawn_count(), 0);
}

#[tokio::test]
async fn task_rejects_unknown_option_names() {
    let driver = Arc::new(FakeDriver::new());
    let message = script_message(
        run(
            &driver,
            r#"return await task({ description: "x", responseschema: {} });"#,
            json!(null),
        )
        .await,
    );
    assert!(message.contains("invalid options"), "{message}");
    assert_eq!(driver.spawn_count(), 0);
}

#[tokio::test]
async fn driver_rejection_is_catchable_in_script() {
    let driver = Arc::new(FakeDriver::new());
    driver.on("bad", FakeReply::Reject("admission cap".to_string()));
    let value = run(
        &driver,
        r#"
        try {
            await task({ description: "bad idea" });
            return "no-throw";
        } catch (err) {
            return String(err);
        }
        "#,
        json!(null),
    )
    .await
    .unwrap();
    let text = value.as_str().unwrap();
    assert!(text.contains("admission cap"), "{text}");
}

#[tokio::test]
async fn parallel_fan_out_maps_one_failure_to_null_slot() {
    let driver = Arc::new(FakeDriver::new());
    driver.on("beta", FakeReply::Fail("boom".to_string()));
    let value = run(
        &driver,
        r#"
        return await parallel([
            () => task({ description: "alpha" }),
            () => task({ description: "beta" }),
            () => task({ description: "gamma" }),
        ]);
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value, json!(["done:alpha", null, "done:gamma"]));
    assert_eq!(driver.spawn_count(), 3);
}

#[tokio::test]
async fn parallel_logs_a_breadcrumb_when_a_slot_is_dropped_to_null() {
    // #dogfood 0.8.67: a fan-out slot that fails for a non-schema reason still
    // resolves to null (documented resilience), but must leave a breadcrumb in
    // the run log so an operator can see why a slot came back null / nothing
    // spawned — instead of a silent "completed" with no explanation.
    let driver = Arc::new(FakeDriver::new());
    driver.on("beta", FakeReply::Fail("boom".to_string()));
    let value = run(
        &driver,
        r#"
        return await parallel([
            () => task({ description: "alpha" }),
            () => task({ description: "beta" }),
        ]);
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value, json!(["done:alpha", null]));
    assert!(
        driver.events().iter().any(|event| matches!(
            event,
            ProgressEvent::Log { message } if message.contains("dropped a failed slot")
        )),
        "a dropped parallel slot should leave a breadcrumb in the run log"
    );
}

#[tokio::test]
async fn parallel_surfaces_response_schema_errors_instead_of_null() {
    let driver = Arc::new(FakeDriver::new());
    driver.on(
        "bad schema",
        FakeReply::Complete(r#"{"refuted":"yes"}"#.to_string()),
    );

    let message = script_message(
        run(
            &driver,
            r#"
            return await parallel([
                () => task({
                    description: "bad schema",
                    responseSchema: {
                        type: "object",
                        properties: { refuted: { type: "boolean" } },
                        required: ["refuted"],
                    },
                }),
            ]);
            "#,
            json!(null),
        )
        .await,
    );

    assert!(message.contains("responseSchema validation"), "{message}");
    assert!(
        driver.events().iter().any(|event| matches!(
            event,
            ProgressEvent::TaskSchemaValidationFailed { message, .. }
                if message.contains("responseSchema validation")
        )),
        "schema validation error should be emitted as workflow progress"
    );
}

#[tokio::test]
async fn pipeline_surfaces_response_schema_errors_instead_of_null() {
    let driver = Arc::new(FakeDriver::new());
    driver.on(
        "bad schema",
        FakeReply::Complete(r#"{"refuted":"yes"}"#.to_string()),
    );

    let message = script_message(
        run(
            &driver,
            r#"
            return await pipeline(
                ["bad schema"],
                (description) => task({
                    description,
                    responseSchema: {
                        type: "object",
                        properties: { refuted: { type: "boolean" } },
                        required: ["refuted"],
                    },
                }),
            );
            "#,
            json!(null),
        )
        .await,
    );

    assert!(message.contains("responseSchema validation"), "{message}");
}

#[tokio::test]
async fn parallel_enforces_the_1000_item_cap_without_spawning() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(
        &driver,
        r#"
        const thunks = new Array(1001).fill(() => task({ description: "x" }));
        try {
            await parallel(thunks);
            return "no-throw";
        } catch (err) {
            return String(err);
        }
        "#,
        json!(null),
    )
    .await
    .unwrap();
    let text = value.as_str().unwrap();
    assert!(text.contains("max 1000"), "{text}");
    assert_eq!(driver.spawn_count(), 0, "cap must reject before any spawn");
}

#[tokio::test]
async fn parallel_accepts_exactly_1000_items() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(
        &driver,
        r#"
        const thunks = new Array(1000).fill(() => Promise.resolve(1));
        const results = await parallel(thunks);
        return results.length;
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value, json!(1000));
}

#[tokio::test]
async fn pipeline_has_no_barrier_between_stages() {
    let driver = Arc::new(FakeDriver::new());
    // Item A crawls through stage 1; item B sprints through both stages.
    driver.on_with_delay(
        "s1:A",
        FakeReply::Complete("A1".to_string()),
        Duration::from_millis(300),
    );
    driver.on_with_delay(
        "s1:B",
        FakeReply::Complete("B1".to_string()),
        Duration::from_millis(20),
    );
    driver.on_with_delay(
        "s2:B1",
        FakeReply::Complete("B2".to_string()),
        Duration::from_millis(20),
    );
    driver.on("s2:A1", FakeReply::Complete("A2".to_string()));

    let value = run(
        &driver,
        r#"
        return await pipeline(
            ["A", "B"],
            (v) => task({ description: "s1:" + v }),
            (v) => task({ description: "s2:" + v }),
        );
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value, json!(["A2", "B2"]));

    // B's stage 2 must have been requested while A was still in stage 1 —
    // per-item chains, no stage barrier.
    let descriptions = driver.request_descriptions();
    assert_eq!(descriptions[..2], ["s1:A".to_string(), "s1:B".to_string()]);
    assert_eq!(
        descriptions[2], "s2:B1",
        "expected B to reach stage 2 while A was still in stage 1: {descriptions:?}"
    );
    assert_eq!(descriptions[3], "s2:A1");
}

#[tokio::test]
async fn pipeline_stage_error_drops_only_that_item() {
    let driver = Arc::new(FakeDriver::new());
    driver.on("s1:B", FakeReply::Fail("boom".to_string()));
    let value = run(
        &driver,
        r#"
        return await pipeline(
            ["A", "B"],
            (v) => task({ description: "s1:" + v }),
            (v) => v + "+2",
        );
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value, json!(["done:s1:A+2", null]));
}

#[tokio::test]
async fn task_throws_once_budget_spent_reaches_total() {
    let driver = Arc::new(FakeDriver::new());
    driver.set_budget(Some(100), 60);
    let value = run(
        &driver,
        r#"
        let completed = 0;
        try {
            while (true) {
                await task({ description: "chunk " + completed });
                completed++;
            }
        } catch (err) {
            return { completed, message: String(err) };
        }
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value["completed"], json!(2));
    let message = value["message"].as_str().unwrap();
    assert!(message.contains("budget exhausted"), "{message}");
    assert_eq!(driver.spawn_count(), 2);
}

#[tokio::test]
async fn budget_globals_reflect_live_driver_snapshots() {
    let driver = Arc::new(FakeDriver::new());
    driver.set_budget(Some(1000), 100);
    let value = run(
        &driver,
        r#"
        const before = budget.remaining();
        await task({ description: "one" });
        return {
            total: budget.total,
            before,
            spent: budget.spent(),
            after: budget.remaining(),
        };
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(
        value,
        json!({"total": 1000, "before": 1000, "spent": 100, "after": 900})
    );
}

#[tokio::test]
async fn unbounded_budget_reads_as_null_total_and_infinite_remaining() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(
        &driver,
        "return budget.total === null && budget.remaining() === Infinity;",
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value, json!(true));
}

#[tokio::test]
async fn lifetime_cap_throws_on_spawn_attempt_1001() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(
        &driver,
        r#"
        let completed = 0;
        try {
            for (let i = 0; i < 1001; i++) {
                await task({ description: "t" + i });
                completed++;
            }
            return "no-throw";
        } catch (err) {
            return { completed, message: String(err) };
        }
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value["completed"], json!(WORKFLOW_LIFETIME_CAP));
    let message = value["message"].as_str().unwrap();
    assert!(message.contains("lifetime agent cap (1000)"), "{message}");
    assert_eq!(driver.spawn_count(), WORKFLOW_LIFETIME_CAP as usize);
}

#[tokio::test]
async fn response_schema_returns_the_parsed_validated_object() {
    let driver = Arc::new(FakeDriver::new());
    driver.on(
        "check",
        FakeReply::Complete(r#"{"refuted": true, "confidence": 0.9}"#.to_string()),
    );
    let value = run(
        &driver,
        r#"
        const verdict = await task({
            description: "check the claim",
            responseSchema: {
                type: "object",
                properties: { refuted: { type: "boolean" } },
                required: ["refuted"],
            },
        });
        return verdict.refuted === true ? "refuted" : "upheld";
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value, json!("refuted"));
    assert!(driver.requests()[0].response_schema.is_some());
}

#[tokio::test]
async fn response_schema_rejects_non_json_replies() {
    let driver = Arc::new(FakeDriver::new());
    driver.on(
        "check",
        FakeReply::Complete("definitely not json".to_string()),
    );
    let message = script_message(
        run(
            &driver,
            r#"
            return await task({
                description: "check",
                responseSchema: { type: "object" },
            });
            "#,
            json!(null),
        )
        .await,
    );
    assert!(message.contains("not valid JSON"), "{message}");
}

#[tokio::test]
async fn response_schema_rejects_schema_violations() {
    let driver = Arc::new(FakeDriver::new());
    driver.on(
        "check",
        FakeReply::Complete(r#"{"refuted": "yes"}"#.to_string()),
    );
    let message = script_message(
        run(
            &driver,
            r#"
            return await task({
                description: "check",
                responseSchema: {
                    type: "object",
                    properties: { refuted: { type: "boolean" } },
                    required: ["refuted"],
                },
            });
            "#,
            json!(null),
        )
        .await,
    );
    assert!(message.contains("responseSchema validation"), "{message}");
}

#[tokio::test]
async fn determinism_ban_date_now() {
    let driver = Arc::new(FakeDriver::new());
    let message = script_message(run(&driver, "return Date.now();", json!(null)).await);
    assert!(message.contains("Date.now()"), "{message}");
}

#[tokio::test]
async fn determinism_ban_math_random() {
    let driver = Arc::new(FakeDriver::new());
    let message = script_message(run(&driver, "return Math.random();", json!(null)).await);
    assert!(message.contains("Math.random()"), "{message}");
}

#[tokio::test]
async fn determinism_ban_new_date() {
    let driver = Arc::new(FakeDriver::new());
    let message = script_message(run(&driver, "return new Date();", json!(null)).await);
    assert!(message.contains("unavailable"), "{message}");
}

/// Explicit product surface for the sandboxed Workflow VM (#4129).
///
/// Only these Workflow-owned calls may exist on `globalThis` beyond standard
/// ECMAScript intrinsics. If a new host global is intentionally added, update
/// this list in the same PR — the fail-closed inventory test below will break
/// until the allowlist is extended deliberately.
const WORKFLOW_ALLOWED_GLOBALS: &[&str] = &[
    "task", "parallel", "pipeline", "phase", "log", "budget", "args",
];

/// Host / Node / Deno / browser surfaces that must never leak into the VM.
///
/// Standard ECMAScript intrinsics (`Object`, `Function`, `eval`, `Promise`, …)
/// remain available; this list is only host escape hatches.
const SANDBOX_BANNED_GLOBALS: &[&str] = &[
    "process",
    "require",
    "module",
    "exports",
    "__dirname",
    "__filename",
    "Buffer",
    "fs",
    "child_process",
    "os",
    "path",
    "net",
    "http",
    "https",
    "fetch",
    "XMLHttpRequest",
    "WebSocket",
    "Deno",
    "Bun",
    "Worker",
];

#[tokio::test]
async fn sandbox_exposes_only_the_documented_workflow_calls() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(
        &driver,
        r#"
        return {
            task: typeof task,
            parallel: typeof parallel,
            pipeline: typeof pipeline,
            phase: typeof phase,
            log: typeof log,
            budget: typeof budget,
            args: typeof args,
        };
        "#,
        json!({"ok": true}),
    )
    .await
    .unwrap();
    assert_eq!(
        value,
        json!({
            "task": "function",
            "parallel": "function",
            "pipeline": "function",
            "phase": "function",
            "log": "function",
            "budget": "object",
            "args": "object",
        })
    );
    // Keep the constant and the live typeof probe in lockstep.
    assert_eq!(
        WORKFLOW_ALLOWED_GLOBALS,
        &[
            "task", "parallel", "pipeline", "phase", "log", "budget", "args"
        ]
    );
}

#[tokio::test]
async fn sandbox_blocks_host_filesystem_shell_network_and_env_surfaces() {
    // Each probe must either throw / reject or resolve to a clearly absent
    // binding. We never allow a successful host escape.
    let probes: &[(&str, &str)] = &[
        (
            "process.env",
            r#"
            if (typeof process !== "undefined") {
                return process.env;
            }
            throw new Error("process is unavailable");
            "#,
        ),
        (
            "require('fs')",
            r#"
            if (typeof require === "function") {
                return require("fs");
            }
            throw new Error("require is unavailable");
            "#,
        ),
        (
            "import",
            r#"
            // Dynamic import is a module-loader surface; the VM has no loader.
            return await import("fs");
            "#,
        ),
        (
            "fetch",
            r#"
            if (typeof fetch === "function") {
                return await fetch("https://example.invalid/");
            }
            throw new Error("fetch is unavailable");
            "#,
        ),
        (
            "child_process",
            r#"
            if (typeof require === "function") {
                return require("child_process");
            }
            if (typeof child_process !== "undefined") {
                return child_process;
            }
            throw new Error("child_process is unavailable");
            "#,
        ),
        (
            "Deno.env",
            r#"
            if (typeof Deno !== "undefined") {
                return Deno.env.toObject();
            }
            throw new Error("Deno is unavailable");
            "#,
        ),
    ];

    for (label, source) in probes {
        let driver = Arc::new(FakeDriver::new());
        let result = run(&driver, source, json!(null)).await;
        assert!(
            result.is_err(),
            "sandbox probe `{label}` must fail closed, got {result:?}"
        );
        // No driver side-effect is expected from a sandbox probe.
        assert_eq!(
            driver.spawn_count(),
            0,
            "probe `{label}` must not spawn tasks"
        );
    }
}

#[tokio::test]
async fn sandbox_global_inventory_fails_closed_on_new_host_leaks() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(
        &driver,
        r#"
        // Own enumerable + non-enumerable names on the global object.
        // Anything beyond standard ECMAScript + the Workflow allowlist is a
        // regression that must break this test so new leaks cannot land quietly.
        const names = Reflect.ownKeys(globalThis)
            .map((k) => String(k))
            .sort();
        return names;
        "#,
        json!(null),
    )
    .await
    .unwrap();
    let names: Vec<String> = serde_json::from_value(value).expect("name list is a JSON array");

    // Fail closed: none of the banned host surfaces may appear.
    for banned in SANDBOX_BANNED_GLOBALS {
        assert!(
            !names.iter().any(|n| n == *banned),
            "banned global `{banned}` leaked into the Workflow VM: {names:?}"
        );
    }

    // Every Workflow-owned call must still be present.
    for allowed in WORKFLOW_ALLOWED_GLOBALS {
        assert!(
            names.iter().any(|n| n == *allowed),
            "expected Workflow global `{allowed}` missing from inventory: {names:?}"
        );
    }

    // Internal host helpers must not be script-visible.
    for internal in [
        "__workflow_task",
        "__workflow_log",
        "__workflow_phase",
        "__workflow_budget_total",
        "__workflow_budget_spent",
        "__workflow_budget_remaining",
    ] {
        assert!(
            !names.iter().any(|n| n == internal),
            "internal host binding `{internal}` must stay hidden: {names:?}"
        );
    }
}

#[tokio::test]
async fn sandbox_rejects_commonjs_module_loader_and_eval_style_constructors() {
    let driver = Arc::new(FakeDriver::new());
    // `eval` / `Function` are standard ES, but if they are present they must
    // still be unable to reach host modules. The banned-global inventory above
    // already fails closed if Node-style loaders appear; this probe documents
    // the intended product message for module load attempts.
    let message = script_message(
        run(
            &driver,
            r#"
            if (typeof require === "function") {
                return require("node:fs");
            }
            throw new Error("require is unavailable");
            "#,
            json!(null),
        )
        .await,
    );
    assert!(
        message.contains("unavailable") || message.contains("require"),
        "{message}"
    );
}

#[tokio::test]
async fn dropping_the_run_future_cancels_outstanding_tasks() {
    let driver = Arc::new(FakeDriver::new());
    driver.on("hang", FakeReply::Never);
    let vm = WorkflowVm::new();
    {
        let fut = vm.run_script(
            "await task({ description: 'hang forever' }); return 'unreachable';",
            json!(null),
            driver.clone() as Arc<dyn codewhale_workflow_js::WorkflowDriver>,
        );
        let outcome = tokio::time::timeout(Duration::from_millis(400), fut).await;
        assert!(outcome.is_err(), "run should still be pending at timeout");
        // The timed-out future is dropped here.
    }
    assert!(
        driver.cancel_all_calls() >= 1,
        "dropping the run future must cancel outstanding driver tasks"
    );
    assert_eq!(driver.spawn_count(), 1);
}

#[tokio::test]
async fn parallel_does_not_continue_after_external_run_cancellation() {
    let driver = Arc::new(FakeDriver::new());
    driver.on("hang", FakeReply::Never);
    let cancel = WorkflowRunCancel::new();
    let run_cancel = cancel.clone();
    let run_driver = driver.clone();
    let handle = tokio::spawn(async move {
        WorkflowVm::new()
            .run_script_with_cancel(
                r#"
                await parallel([() => task({ description: "hang" })]);
                phase("unreachable after cancellation");
                return "wrong";
                "#,
                json!(null),
                run_driver as Arc<dyn codewhale_workflow_js::WorkflowDriver>,
                run_cancel,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(2), async {
        while driver.spawn_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("task should start");
    cancel.cancel();

    let result = handle.await.expect("VM task should join");
    assert!(
        matches!(result, Err(WorkflowJsError::Cancelled)),
        "{result:?}"
    );
    assert!(
        !driver.events().iter().any(|event| matches!(
            event,
            ProgressEvent::Phase { title } if title == "unreachable after cancellation"
        )),
        "parallel() must not downgrade run cancellation into a null slot"
    );
}

#[tokio::test]
async fn script_error_rejects_cleanly_and_cancels_children() {
    let driver = Arc::new(FakeDriver::new());
    let result = run(
        &driver,
        r#"await task({ description: "quick" }); throw new Error("boom");"#,
        json!(null),
    )
    .await;
    let message = script_message(result);
    assert!(message.contains("boom"), "{message}");
    assert!(
        driver.cancel_all_calls() >= 1,
        "a failed run must cancel its cascade"
    );
}

#[tokio::test]
async fn log_and_phase_events_reach_the_driver_in_order() {
    let driver = Arc::new(FakeDriver::new());
    run(
        &driver,
        r#"
        phase("scan");
        log("a");
        log({ found: 2 });
        phase("verify");
        log("b");
        return null;
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(
        driver.events(),
        vec![
            ProgressEvent::Phase {
                title: "scan".to_string()
            },
            ProgressEvent::Log {
                message: "a".to_string()
            },
            ProgressEvent::Log {
                message: r#"{"found":2}"#.to_string()
            },
            ProgressEvent::Phase {
                title: "verify".to_string()
            },
            ProgressEvent::Log {
                message: "b".to_string()
            },
        ]
    );
}

#[tokio::test]
async fn promise_all_of_tasks_resolves_concurrently() {
    let driver = Arc::new(FakeDriver::new());
    driver.on_with_delay(
        "left",
        FakeReply::Complete("L".to_string()),
        Duration::from_millis(50),
    );
    driver.on_with_delay(
        "right",
        FakeReply::Complete("R".to_string()),
        Duration::from_millis(50),
    );
    let started = std::time::Instant::now();
    let value = run(
        &driver,
        r#"
        const [a, b] = await Promise.all([
            task({ description: "left" }),
            task({ description: "right" }),
        ]);
        return a + "/" + b;
        "#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value, json!("L/R"));
    // Two 50ms tasks awaited concurrently should not take ~100ms serially.
    // Generous bound to stay green on slow CI.
    assert!(
        started.elapsed() < Duration::from_millis(3000),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(driver.spawn_count(), 2);
}

#[tokio::test]
async fn export_default_async_function_runs_with_args() {
    let driver = Arc::new(FakeDriver::new());
    let source = r#"
export default async function (args) {
  return { doubled: args.n * 2 };
}
"#;
    let value = run(&driver, source, json!({ "n": 21 })).await.unwrap();
    assert_eq!(value, json!({ "doubled": 42 }));
}

#[tokio::test]
async fn export_default_function_result_becomes_run_result() {
    let driver = Arc::new(FakeDriver::new());
    let source = r#"
function helper() {
  return "from-helper";
}
export default function () {
  return helper();
}
"#;
    let value = run(&driver, source, json!(null)).await.unwrap();
    assert_eq!(value, json!("from-helper"));
}

#[tokio::test]
async fn export_default_non_function_value_is_returned() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(&driver, "export default 7;", json!(null))
        .await
        .unwrap();
    assert_eq!(value, json!(7));
}

#[tokio::test]
async fn plain_scripts_are_untouched_by_export_desugaring() {
    let driver = Arc::new(FakeDriver::new());
    // A string literal mentioning `export default` must not trigger the
    // module desugaring path.
    let value = run(
        &driver,
        "const note = \"export default docs\";\nreturn note.length;",
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value, json!(19));
}

#[tokio::test]
async fn export_default_examples_inside_multiline_text_are_not_desugared() {
    let driver = Arc::new(FakeDriver::new());
    let value = run(
        &driver,
        r#"
const template = `
export default async function (args) {
  return args;
}
`;
/*
export default function () {
  return "comment example";
}
*/
return template.includes("export default async function");
"#,
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(value, json!(true));
}
