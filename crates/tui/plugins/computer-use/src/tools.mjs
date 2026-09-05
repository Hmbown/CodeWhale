// codewhale-cu tool schemas — single source of truth for tools/list.
// Every action/observation tool accepts an optional `computer` id; supplying it
// switches the active computer first (switch-by-use is the default model).
const computerParam = {
  type: "string",
  description: "Computer id to act on. Defaults to the active computer. Providing a different registered id switches to it first (sticky).",
};

const targetSchema = {
  oneOf: [
    {
      type: "object",
      description: "Element target from the latest get_app_state on this computer.",
      required: ["type", "state_id", "index"],
      properties: {
        type: { const: "element" },
        state_id: { type: "string" },
        index: { type: "integer", minimum: 0 },
      },
      additionalProperties: false,
    },
    {
      type: "object",
      description: "Pixel coordinates in the latest returned raster (screenshot or zoom) for this computer.",
      required: ["type", "x", "y"],
      properties: {
        type: { const: "coordinate" },
        x: { type: "integer", minimum: 0 },
        y: { type: "integer", minimum: 0 },
      },
      additionalProperties: false,
    },
  ],
};

export const TOOLS = [
  // ---- computers (switching is a default) ----
  {
    name: "computer_list",
    description: "List registered computers (local, ssh, harmony/hdc) and which one is active. Every other tool acts on the active computer unless given `computer`.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
  },
  {
    name: "computer_switch",
    description: "Switch the active computer. Subsequent tools act on it by default.",
    inputSchema: { type: "object", required: ["computer"], properties: { computer: { type: "string", description: "Registered computer id (see computer_list)" } }, additionalProperties: false },
  },
  {
    name: "computer_register",
    description: "Register or update a computer. transport=local (this machine), ssh (runs the bundled remote agent over ssh; agent is pushed automatically), hdc (HarmonyOS device via hdc).",
    inputSchema: {
      type: "object",
      required: ["computer", "transport"],
      properties: {
        computer: { type: "string", description: "Short id for the computer (letters, digits, dot, dash)" },
        transport: { enum: ["local", "ssh", "hdc"] },
        label: { type: "string" },
        host: { type: "string", description: "ssh: hostname" },
        port: { type: "integer", description: "ssh: port (default 22)" },
        user: { type: "string", description: "ssh: user" },
        target: { type: "string", description: "hdc: target key (omit for the only connected device)" },
        installAgent: { type: "boolean", description: "ssh: push the remote agent before first use (default true)" },
      },
      additionalProperties: false,
    },
  },
  {
    name: "computer_remove",
    description: "Remove a registered computer. 'local' cannot be removed.",
    inputSchema: { type: "object", required: ["computer"], properties: { computer: { type: "string" } }, additionalProperties: false },
  },
  // ---- observe & resolve ----
  {
    name: "request_access",
    description: "Probe permissions and capabilities of a computer (accessibility, screen capture, recording, missing tools). Call once when readiness is unknown or a permission failure is explicitly named.",
    inputSchema: { type: "object", properties: { computer: computerParam }, additionalProperties: false },
  },
  {
    name: "list_displays",
    description: "List displays/panels with geometry and pixel scale.",
    inputSchema: { type: "object", properties: { computer: computerParam }, additionalProperties: false },
  },
  {
    name: "switch_display",
    description: "Set which display subsequent screenshots/recordings capture on this computer.",
    inputSchema: { type: "object", required: ["index"], properties: { index: { type: "integer", minimum: 1 }, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "list_apps",
    description: "List running applications (name, pid, bundle id). If the user names an app that is absent, use open_application once with the exact user-provided name.",
    inputSchema: { type: "object", properties: { computer: computerParam }, additionalProperties: false },
  },
  {
    name: "list_windows",
    description: "List windows of an application (or all windows when app_ref is omitted).",
    inputSchema: {
      type: "object",
      properties: {
        app_ref: {
          type: "object",
          properties: {
            pid: { type: "integer" }, name: { type: "string" }, bundle_id: { type: "string" },
          },
          additionalProperties: false,
        },
        computer: computerParam,
      },
      additionalProperties: false,
    },
  },
  {
    name: "get_app_state",
    description: "Observe an application once: returns a bounded accessibility/UIA/uitest element tree with stable indices. Elements are the primary action targets; request screenshots only when accessibility cannot express the target.",
    inputSchema: {
      type: "object",
      properties: {
        app_ref: { type: "object", properties: { pid: { type: "integer" }, name: { type: "string" }, bundle_id: { type: "string" } }, additionalProperties: false },
        window_id: { type: "integer", description: "Zero-based window index within the app" },
        detail: { enum: ["compact", "full"] },
        computer: computerParam,
      },
      additionalProperties: false,
    },
  },
  {
    name: "screenshot",
    description: "Capture the screen (all or one display, optional region) as PNG/JPEG. The receipt carries raster geometry; later coordinate targets refer to this raster.",
    inputSchema: {
      type: "object",
      properties: {
        display: { type: ["integer", "string"], description: "Display index or 'all'" },
        region: { type: "array", items: { type: "number" }, minItems: 4, maxItems: 4, description: "[x, y, w, h] in screen points" },
        path: { type: "string", description: "Optional output path (absolute). Defaults into the recordings directory." },
        computer: computerParam,
      },
      additionalProperties: false,
    },
  },
  {
    name: "zoom",
    description: "Close-up crop of the latest screenshot. Choose points from the returned child raster only.",
    inputSchema: {
      type: "object",
      required: ["region"],
      properties: {
        region: { type: "array", items: { type: "number" }, minItems: 4, maxItems: 4, description: "[x, y, w, h] in last-raster pixels" },
        path: { type: "string" },
        computer: computerParam,
      },
      additionalProperties: false,
    },
  },
  {
    name: "cursor_position",
    description: "Read the current pointer position in screen points.",
    inputSchema: { type: "object", properties: { computer: computerParam }, additionalProperties: false },
  },
  {
    name: "open_application",
    description: "Launch or activate an application. Copy user-provided names character-for-character; never translate, normalize, or strip suffixes. On macOS prefer bundle_id when known.",
    inputSchema: {
      type: "object",
      properties: {
        name: { type: "string" }, bundle_id: { type: "string" }, url: { type: "string" },
        activate: { type: "boolean", description: "Bring to foreground" },
        computer: computerParam,
      },
      additionalProperties: false,
    },
  },
  // ---- pointer ----
  {
    name: "left_click", description: "Left-click a coordinate (pixels in the latest raster) or perform the element's press action.",
    inputSchema: { type: "object", required: ["target"], properties: { target: targetSchema, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "double_click", description: "Double-click a target.",
    inputSchema: { type: "object", required: ["target"], properties: { target: targetSchema, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "triple_click", description: "Triple-click a target (e.g. select a paragraph).",
    inputSchema: { type: "object", required: ["target"], properties: { target: targetSchema, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "right_click", description: "Right-click a target (context menu).",
    inputSchema: { type: "object", required: ["target"], properties: { target: targetSchema, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "middle_click", description: "Middle-click a target.",
    inputSchema: { type: "object", required: ["target"], properties: { target: targetSchema, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "mouse_move", description: "Move the pointer without clicking (hover).",
    inputSchema: { type: "object", required: ["target"], properties: { target: targetSchema, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "left_click_drag", description: "Press at from_target, move in steps, release at `to`.",
    inputSchema: { type: "object", required: ["from_target", "to"], properties: { from_target: targetSchema, to: targetSchema, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "left_mouse_down", description: "Press and hold the left button at a target. Release with left_mouse_up.",
    inputSchema: { type: "object", properties: { target: targetSchema, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "left_mouse_up", description: "Release the left button pressed by left_mouse_down.",
    inputSchema: { type: "object", properties: { computer: computerParam }, additionalProperties: false },
  },
  {
    name: "scroll", description: "Scroll at a target: direction up/down/left/right, amount in lines/notches.",
    inputSchema: { type: "object", required: ["target"], properties: { target: targetSchema, direction: { enum: ["up", "down", "left", "right"] }, amount: { type: "integer", minimum: 1, maximum: 100 }, computer: computerParam }, additionalProperties: false },
  },
  // ---- text & keyboard ----
  {
    name: "type", description: "Type text into the focused control (unicode). Focus the field first (click/element action).",
    inputSchema: { type: "object", required: ["text"], properties: { text: { type: "string" }, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "key", description: "Press a key or chord, e.g. 'return', 'cmd+c' (macOS), 'ctrl+c' (Linux/Windows). Repeat with `repeat`.",
    inputSchema: { type: "object", required: ["text"], properties: { text: { type: "string" }, repeat: { type: "integer", minimum: 1, maximum: 100 }, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "hold_key", description: "Hold a key for `duration` seconds (0.05..30).",
    inputSchema: { type: "object", required: ["text", "duration"], properties: { text: { type: "string" }, duration: { type: "number", minimum: 0.05, maximum: 30 }, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "set_value", description: "Set an editable element's value through the accessibility layer (background-safe, no keystrokes). Element targets only.",
    inputSchema: { type: "object", required: ["target", "value"], properties: { target: targetSchema, value: { type: "string" }, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "select_text", description: "Select a text range [start, length] in an element, or place the caret when omitted.",
    inputSchema: { type: "object", properties: { target: targetSchema, text_range: { type: "array", items: { type: "integer" }, minItems: 2, maxItems: 2 }, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "perform_action", description: "Invoke a named accessibility action on an element (e.g. AXPress on macOS, Invoke on Windows/UIA, click on harmony). Only actions the element advertises.",
    inputSchema: { type: "object", required: ["target", "action"], properties: { target: targetSchema, action: { type: "string" }, computer: computerParam }, additionalProperties: false },
  },
  // ---- clipboard / runtime ----
  {
    name: "read_clipboard", description: "Read the system clipboard as UTF-8 text.",
    inputSchema: { type: "object", properties: { computer: computerParam }, additionalProperties: false },
  },
  {
    name: "write_clipboard", description: "Write UTF-8 text to the system clipboard.",
    inputSchema: { type: "object", required: ["text"], properties: { text: { type: "string" }, computer: computerParam }, additionalProperties: false },
  },
  // ---- recording ----
  {
    name: "recording_start",
    description: "Start screen recording on a computer (mp4/mov). Darwin: screencapture -v (timed or until recording_stop). Linux: x11grab/wf-recorder. Windows: ffmpeg gdigrab. HarmonyOS: snapshot-series muxed with ffmpeg.",
    inputSchema: {
      type: "object",
      properties: {
        display: { type: ["integer", "string"] },
        fps: { type: "integer", minimum: 1, maximum: 60, description: "Linux/Windows/harmony-series only" },
        region: { type: "array", items: { type: "number" }, minItems: 4, maxItems: 4, description: "Linux/Windows only" },
        durationSec: { type: "number", minimum: 1, maximum: 7200, description: "macOS only: auto-stop after N seconds" },
        intervalMs: { type: "integer", minimum: 150, maximum: 5000, description: "harmony snapshot-series frame interval" },
        computer: computerParam,
      },
      additionalProperties: false,
    },
  },
  {
    name: "recording_stop",
    description: "Stop a running recording and finalize the file.",
    inputSchema: { type: "object", required: ["id"], properties: { id: { type: "string" }, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "recording_status",
    description: "Status of one recording (running, bytes so far).",
    inputSchema: { type: "object", required: ["id"], properties: { id: { type: "string" }, computer: computerParam }, additionalProperties: false },
  },
  {
    name: "recording_list",
    description: "List recordings and screenshots saved on a computer.",
    inputSchema: { type: "object", properties: { computer: computerParam }, additionalProperties: false },
  },
  // ---- kill switch ----
  {
    name: "stop_computer_control",
    description: "Kill switch: refuse all further computer-use actions for the rest of the session. Read-only probes stay available.",
    inputSchema: { type: "object", properties: { reason: { type: "string" } }, additionalProperties: false },
  },
  {
    name: "wait",
    description: "Pause before the next observation (0..30s). Use after actions that animate or load.",
    inputSchema: { type: "object", properties: { seconds: { type: "number", minimum: 0, maximum: 30 } }, additionalProperties: false },
  },
];

export const TOOL_NAMES = new Set(TOOLS.map((t) => t.name));

/** Tools that never touch a computer (available even after kill switch). */
export const READ_ONLY_TOOLS = new Set([
  "computer_list", "stop_computer_control", "wait", "request_access", "recording_list", "recording_status",
]);

/** Tools dispatchable to a remote agent over ssh (allow-list must match agent.mjs). */
export const REMOTE_TOOLS = new Set([
  "probe", "list_displays", "switch_display", "list_apps", "list_windows",
  "open_application", "get_app_state", "screenshot", "zoom",
  "left_click", "double_click", "triple_click", "right_click", "middle_click",
  "mouse_move", "left_click_drag", "left_mouse_down", "left_mouse_up", "scroll",
  "type", "key", "hold_key", "set_value", "select_text", "perform_action",
  "read_clipboard", "write_clipboard", "cursor_position",
  "recordingStart", "recordingStop", "recordingStatus", "recordingList",
]);

/** Map public tool name -> backend method name. */
export const BACKEND_METHOD = Object.fromEntries(
  TOOLS.filter((t) => !["computer_list", "computer_switch", "computer_register", "computer_remove", "stop_computer_control", "wait"].includes(t.name))
    .map((t) => [t.name, {
      recording_start: "recordingStart",
      recording_stop: "recordingStop",
      recording_status: "recordingStatus",
      recording_list: "recordingList",
    }[t.name] ?? t.name]),
);
