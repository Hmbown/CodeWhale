import type { ActiveToolProfile } from "./active-tools.js";
import { playerToolProfile } from "./active-tools.js";

export type ReviewStatus = "reviewed" | "unreviewed" | "blocked";

export interface LoadedResourceInventory {
  extensions: readonly string[];
  skills: readonly string[];
  prompts: readonly string[];
  themes: readonly string[];
}

export interface GameSessionState {
  packageSource: string;
  reviewStatus: ReviewStatus;
  resources: LoadedResourceInventory;
  activeToolProfile: ActiveToolProfile;
  diagnosticsVisible: boolean;
  gameRoot?: string;
  saveId?: string;
  saveRevision?: number;
  driverId?: string;
  driverVersion?: string;
  lastRuntimeCommand?: string;
  warnings?: readonly string[];
  renderSnapshot?: unknown;
}

export function createInitialGameState(packageSource = "./packages/genmicon-pi"): GameSessionState {
  return {
    packageSource,
    reviewStatus: "reviewed",
    resources: {
      extensions: ["extensions/index.ts"],
      skills: ["skills/game-driver/SKILL.md", "skills/player-contract/SKILL.md"],
      prompts: ["prompts/game-console.md", "prompts/compact-game-context.md"],
      themes: ["themes/genmicon.json"],
    },
    activeToolProfile: playerToolProfile,
    diagnosticsVisible: false,
  };
}

export function setDiagnosticsVisible(state: GameSessionState, visible: boolean): GameSessionState {
  return {
    ...state,
    diagnosticsVisible: visible,
  };
}

export type DiagnosticsMode = "on" | "off" | "status";

export function parseDiagnosticsMode(value: string): DiagnosticsMode {
  const mode = value.trim().toLowerCase();
  if (mode === "on" || mode === "off" || mode === "status" || mode === "") {
    return mode === "" ? "status" : mode;
  }
  return "status";
}

export function applyDiagnosticsMode(state: GameSessionState, mode: DiagnosticsMode): GameSessionState {
  if (mode === "status") {
    return state;
  }
  return setDiagnosticsVisible(state, mode === "on");
}

export function diagnosticsStatusMessage(state: GameSessionState): string {
  return `GENmicon diagnostics ${state.diagnosticsVisible ? "on" : "off"}.`;
}
