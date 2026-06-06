workflow(
    id = "rlm-cache-change",
    goal = "Evaluate an RLM/cache routing change with safe mock WhaleFlow IR",
    nodes = [
        branch(
            id = "candidate-branches",
            parallel = True,
            children = [
                search(
                    id = "find-cache-surfaces",
                    query = "Find RLM and cache routing surfaces",
                    file_scope = ["crates/tui/src/rlm/**", "crates/tui/src/core/**"],
                ),
                agent(
                    id = "minimal-patch",
                    prompt = "Draft the smallest safe cache-routing patch using shared ARMH context.",
                    agent_type = "implementer",
                    mode = "read_write",
                    isolation = "worktree",
                    file_scope = ["crates/tui/src/rlm/**", "crates/tui/src/core/**"],
                ),
                agent(
                    id = "architecture-review",
                    prompt = "Review cache routing boundaries and identify replay or provider risks.",
                    agent_type = "explore",
                    file_scope = [
                        "crates/tui/src/providers/**",
                        "crates/tui/src/rlm/**",
                    ],
                ),
            ],
        ),
        sequence(
            id = "verify-select-and-summarize",
            children = [
                loop_until(
                    id = "implement-until-tests-pass",
                    condition = "regression tests pass",
                    max_iterations = 2,
                    children = [
                        test(
                            id = "regression-tests",
                            command = "cargo test -p codewhale-tui rlm --locked",
                            file_scope = ["crates/tui/src/rlm/**"],
                        ),
                    ],
                ),
                tournament(
                    id = "select-maintainer-slice",
                    candidates = [
                        "minimal-patch",
                        "regression-tests",
                        "architecture-review",
                    ],
                ),
                teacher_review(
                    id = "teacher-review",
                    candidates = ["select-maintainer-slice"],
                ),
                reduce(
                    id = "summarize-cache-change",
                    inputs = [
                        "find-cache-surfaces",
                        "minimal-patch",
                        "architecture-review",
                        "regression-tests",
                        "teacher-review",
                    ],
                    prompt = "Summarize the smallest safe cache-routing patch.",
                ),
            ],
        ),
    ],
)
