workflow(
    id = "issue-fix-tournament",
    goal = "Compare narrow fixes for one issue before promotion",
    nodes = [
        branch(
            id = "candidate-fixes",
            parallel = True,
            children = [
                agent(
                    id = "minimal-fix",
                    prompt = "Produce the smallest fix and list verification evidence.",
                    agent_type = "implementer",
                    mode = "read_write",
                    isolation = "worktree",
                    file_scope = ["crates/**"],
                ),
                agent(
                    id = "defensive-fix",
                    prompt = "Produce a more defensive fix and list regression risks.",
                    agent_type = "implementer",
                    mode = "read_write",
                    isolation = "worktree",
                    file_scope = ["crates/**"],
                ),
            ],
        ),
        sequence(
            id = "review",
            children = [
                tournament(
                    id = "select-fix",
                    candidates = ["minimal-fix", "defensive-fix"],
                ),
                test(
                    id = "verify-selected",
                    command = "cargo test --workspace --locked",
                    file_scope = ["crates/**"],
                ),
            ],
        ),
    ],
)
