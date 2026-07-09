# Work Units Glossary

CodeWhale shows several kinds of work at once. These names are intentionally
separate so the UI can stay calm under load.

| Term | What it is | Visible surface | Use it when |
|------|------------|-----------------|-------------|
| **Tasks** | Live runtime work in the current session: running tools, background shell jobs, waits, and recent active operations. | The `Tasks` sidebar rail and live status rows. | You need to inspect or cancel what is currently running. |
| **To-do** | The durable checklist for the active objective or turn. It is progress narration, not a running process. | The `To-do` sidebar rail and Work context. | The model needs to expose planned steps, current progress, or remaining checklist items. |
| **Fleet worker** | A durable headless `codewhale exec` worker launched and tracked by Agent Fleet, with a role, route, ledger, logs, artifacts, and receipts. | `/fleet`, the Fleet roster, worker rows, and the Agents sidebar. | Work should be delegated, restarted, inspected, stopped, or audited as a worker. |
| **Workflow run** | A repeatable orchestration plan that sequences phases and may dispatch Fleet workers. Workflow owns ordering; Fleet owns worker execution. | Workflow cards, run status, receipts, and progress overlays. | A task needs a visible multi-phase plan, fan-out/fan-in, or rerunnable coordination. |

## How to Choose

- Put model-visible plan/progress in **To-do**.
- Watch or cancel live commands and runtime activity in **Tasks**.
- Open/stop delegated agent processes through **Fleet worker** rows.
- Use a **Workflow run** when the work needs phases, fan-out/fan-in, or a
  durable orchestration receipt.

## Related Axes

Modes are not work units. **Act**, **Plan**, and **Operate** describe the TUI's
interaction posture; **Ask**, **Auto-Review**, and **Full Access** describe
permission posture. See [MODES.md](MODES.md).

Fleet and Workflow are also different layers. Fleet is the durable worker
substrate; Workflow is the orchestration plan that may use those workers. See
[FLEET.md](FLEET.md) and [WORKFLOW_AUTHORING.md](WORKFLOW_AUTHORING.md).

