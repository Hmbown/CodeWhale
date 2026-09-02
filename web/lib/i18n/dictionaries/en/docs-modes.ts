import type { DocsModesDict } from "../types";

export const docsModes: DocsModesDict = {
  metaTitle: "Modes · Codewhale Docs",
  metaDescription: "Plan, Work, Operate modes and independent permission postures.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Modes",
  overviewLead:
    "A mode decides how Codewhale handles the work. A permission posture decides how it handles consequential tool calls. They are separate controls.",
  modes: [
    ["Plan", "Read-only investigation and planning. Codewhale can inspect the workspace, but it cannot run shell commands or edit files."],
    ["Act", "Normal interactive coding. Codewhale can inspect, edit, and use tools; shell availability and approval prompts follow the active configuration and permission posture."],
    ["Operate", "Multitask coordination from the same composer. The parent can inspect, edit, and use shell or MCP tools under the same permission posture, sandbox, and safety rules as Act. Fleet workers are preferred for independent, parallel, background, or long-running work, but delegation is not required for every executable step. Workflow is optional unless the work needs ordered phases, gates, or deterministic fan-in."],
  ],
  switchingTitle: "Switch modes",
  switchingLead:
    "When the composer is idle, press {tab} to cycle Plan → Act → Operate. When a completion menu is open, Tab accepts the completion; during an active turn, it can queue the current draft as the next follow-up.",
  switchingCommandLead: "Run /mode to open the picker, or switch directly:",
  permissionsTitle: "Permission postures",
  permissionsLead:
    "Plan is always Read Only. When the composer is idle in Act or Operate, press {shiftTab} to cycle Ask → Auto-Review → Full Access. Run {configCommand} to inspect or edit the current session permission; project or managed policy may lock or tighten it.",
  postures: [
    ["Ask", "Ask before tools that can make consequential changes."],
    ["Auto-Review", "Review tool risk automatically and ask when a decision needs you."],
    ["Full Access", "Run tools without approval prompts and enable trusted-workspace access. Repository rules and managed constraints still apply; use it only in a workspace you trust."],
  ],
  sourceNote: "Source document: docs/MODES.md · Update docs-map.ts when changing.",
};
