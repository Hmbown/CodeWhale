RLM sessions that produce large stdout/stderr (e.g. reading a local log file, dumping a large JSON table, or printing diagnostic output) currently inline the full preview into the parent tool result. On long-running RLM sessions this bloat accumulates and pressures the parent context window.

Fix: when `rlm_eval` stdout or stderr exceeds 1000 characters, the full body is stored as a `var_handle` in the handle store. The tool result returns a short inline note (`"N chars; retrieve via handle_read"`) plus `stdout_handle` / `stderr_handle` fields containing the handle reference. The model calls `handle_read` for bounded projections.

Changes:
- `rlm.rs`: Added `STDOUT_HANDLE_THRESHOLD_CHARS` constant
- `rlm.rs`: Added `route_output()` helper that stores large text as a var_handle
- `rlm.rs`: Modified `rlm_eval` execute to route stdout/stderr >= 1k chars into handles
- `rlm.rs`: Updated description to document the new handle-routing behavior
