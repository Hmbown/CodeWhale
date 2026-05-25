import { gameToolNames, type GameToolName } from "./tools.js";

export interface ActiveToolProfile {
  mode: "player" | "developer";
  activeTools: readonly GameToolName[];
  developerOnlyTools: readonly string[];
}

export const playerToolProfile: ActiveToolProfile = {
  mode: "player",
  activeTools: gameToolNames,
  developerOnlyTools: [],
};

export const developerToolProfile: ActiveToolProfile = {
  mode: "developer",
  activeTools: gameToolNames,
  developerOnlyTools: ["genmicon_diagnostics"],
};

export interface ActiveToolTarget {
  setActiveTools?: (tools: readonly GameToolName[]) => void;
}

export function isPlayerToolAllowed(toolName: string): toolName is GameToolName {
  return gameToolNames.includes(toolName as GameToolName);
}

export function installPlayerActiveTools(target: ActiveToolTarget = {}): ActiveToolProfile {
  target.setActiveTools?.(playerToolProfile.activeTools);
  return playerToolProfile;
}

export function preservesPlayerToolProfile(before: ActiveToolProfile, after: ActiveToolProfile): boolean {
  return before.activeTools.length === after.activeTools.length
    && before.activeTools.every((toolName, index) => toolName === after.activeTools[index]);
}
