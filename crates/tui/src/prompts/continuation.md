## Goal Continuation

You are working toward an active session goal. Your task now is to make concrete
progress toward the objective and audit whether the full goal is complete.

Completion is unproven until you verify it against current-state evidence:

1. Derive the concrete requirements from the goal and the latest user
   instructions.
2. Inspect authoritative evidence for each requirement: files, command output,
   tests, runtime behavior, issue or PR state, rendered artifacts, or other
   current sources.
3. Treat uncertain or indirect evidence as not complete. Continue work or gather
   stronger evidence.
4. Only when the full objective is satisfied, call `update_goal` with
   `status: "complete"` and concise evidence.

If the goal cannot continue because of a real blocker, call `update_goal` with
`status: "blocked"` and explain the blocker. Otherwise continue making progress.
