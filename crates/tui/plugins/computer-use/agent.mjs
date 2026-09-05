// codewhale-cu remote agent — runs on an ssh-registered computer.
// One-shot: `node agent.mjs <base64(json)>` prints exactly one JSON receipt
// line. The request is {"tool": "...", "args": {...}}; only tools in this
// allow-list execute, so the transport can never become a generic shell.
import { exec } from "./src/remote-runtime.mjs";

const ALLOWED = new Set([
  "platform", "probe", "list_displays", "switch_display", "list_apps", "list_windows",
  "open_application", "get_app_state", "screenshot", "zoom",
  "left_click", "double_click", "triple_click", "right_click", "middle_click",
  "mouse_move", "left_click_drag", "left_mouse_down", "left_mouse_up", "scroll",
  "type", "key", "hold_key", "set_value", "select_text", "perform_action",
  "read_clipboard", "write_clipboard", "cursor_position",
  "recordingStart", "recordingStop", "recordingStatus", "recordingList",
]);

function reply(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
  process.exit(0);
}

const argv = process.argv.slice(2);
if (!argv[0]) reply({ ok: false, error: { code: "missing_payload", message: "usage: node agent.mjs <base64 payload>" } });

let req;
try {
  req = JSON.parse(Buffer.from(argv[0], "base64").toString("utf8"));
} catch {
  reply({ ok: false, error: { code: "bad_payload", message: "payload is not base64 JSON" } });
}

const tool = req?.tool;
if (!ALLOWED.has(tool)) {
  reply({ ok: false, error: { code: "tool_not_allowed", message: `tool "${tool}" is not in the remote allow-list` } });
}

try {
  if (tool === "platform") {
    reply({ ok: true, platform: process.platform });
  }
  const mod = await import(`./src/backends/${process.platform}.mjs`);
  const backend = mod.create({ exec, computer: { id: "remote", transport: "local", platform: process.platform } });
  const fn = backend[tool];
  if (typeof fn !== "function") {
    reply({ ok: false, error: { code: "unsupported_on_platform", message: `"${tool}" is not implemented on ${process.platform}` } });
  }
  const data = await fn(req.args ?? {});
  reply({ ok: true, platform: process.platform, tool, data });
} catch (err) {
  reply({ ok: false, platform: process.platform, tool, error: { code: err?.code ?? "tool_error", message: String(err?.message ?? err) } });
}
