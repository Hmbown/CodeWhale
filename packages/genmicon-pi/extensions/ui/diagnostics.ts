import type { GameSessionState, LoadedResourceInventory } from "../state.js";

export interface DiagnosticPanelModel {
  id: "genmicon.diagnostics";
  visible: boolean;
  sections: readonly string[];
  rows: readonly DiagnosticRow[];
}

export interface DiagnosticRow {
  label: string;
  value: string;
}

export interface RuntimeDiagnosticSummary {
  saveRevision?: number;
  driverId?: string;
  driverVersion?: string;
  renderSnapshot?: unknown;
  warnings?: readonly string[];
  lastRuntimeCommand?: string;
}

export function createDiagnosticPanelModel(
  visible = false,
  state?: GameSessionState,
  runtime: RuntimeDiagnosticSummary = {},
): DiagnosticPanelModel {
  const snapshot = state ? buildDiagnosticRows(state, runtime) : [];
  return {
    id: "genmicon.diagnostics",
    visible,
    sections: ["package", "resources", "activeTools", "save", "driver", "render", "warnings"],
    rows: snapshot,
  };
}

export function buildDiagnosticRows(
  state: GameSessionState,
  runtime: RuntimeDiagnosticSummary = {},
): DiagnosticRow[] {
  const saveRevision = runtime.saveRevision ?? state.saveRevision;
  const driverId = runtime.driverId ?? state.driverId;
  const driverVersion = runtime.driverVersion ?? state.driverVersion;
  const renderSnapshot = runtime.renderSnapshot ?? state.renderSnapshot;
  const warnings = runtime.warnings ?? state.warnings ?? [];
  const lastRuntimeCommand = runtime.lastRuntimeCommand ?? state.lastRuntimeCommand ?? "none";

  return [
    { label: "Package", value: `${state.packageSource} (${state.reviewStatus})` },
    { label: "Resources", value: formatResources(state.resources) },
    { label: "Active tools", value: state.activeToolProfile.activeTools.join(", ") },
    { label: "Developer-only", value: state.activeToolProfile.developerOnlyTools.join(", ") || "none" },
    { label: "Save", value: state.saveId ? `${state.saveId} @ ${saveRevision ?? "unknown"}` : "unloaded" },
    { label: "Driver", value: driverId ? `${driverId}@${driverVersion ?? "unknown"}` : "unresolved" },
    { label: "Render", value: renderSnapshot ? "available" : "none" },
    { label: "Last runtime", value: lastRuntimeCommand },
    { label: "Warnings", value: warnings.length === 0 ? "none" : warnings.join(" | ") },
  ];
}

export function formatDiagnostics(state: GameSessionState, runtime: RuntimeDiagnosticSummary = {}): string {
  return buildDiagnosticRows(state, runtime)
    .map((row) => `${row.label}: ${row.value}`)
    .join("\n");
}

function formatResources(resources: LoadedResourceInventory): string {
  return [
    `extensions=${resources.extensions.length}`,
    `skills=${resources.skills.length}`,
    `prompts=${resources.prompts.length}`,
    `themes=${resources.themes.length}`,
  ].join(", ");
}
