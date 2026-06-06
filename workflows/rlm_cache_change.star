workflow(
    id = "rlm-cache-change",
    goal = "Evaluate an RLM/cache routing change with safe mock WhaleFlow IR",
    nodes = [
        branch(
            id = "discover",
            parallel = True,
            children = [
                search(
                    id = "find-cache-surfaces",
                    query = "Find RLM and cache routing surfaces",
                    file_scope = ["crates/tui/src/rlm/**", "crates/tui/src/core/**"],
                ),
                agent(
                    id = "inspect-provider-cache",
                    prompt = "Inspect provider cache behavior without editing files.",
                    agent_type = "explore",
                    file_scope = ["crates/tui/src/providers/**"],
                ),
            ],
        ),
        sequence(
            id = "verify-and-summarize",
            children = [
                test(
                    id = "run-rlm-tests",
                    command = "cargo test -p codewhale-tui rlm --locked",
                    file_scope = ["crates/tui/src/rlm/**"],
                ),
                reduce(
                    id = "summarize-cache-change",
                    inputs = [
                        "find-cache-surfaces",
                        "inspect-provider-cache",
                        "run-rlm-tests",
                    ],
                    prompt = "Summarize the smallest safe cache-routing patch.",
                ),
            ],
        ),
    ],
)
