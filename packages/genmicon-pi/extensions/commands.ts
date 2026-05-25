import { resolve } from "node:path";

import { installPlayerActiveTools, type ActiveToolTarget } from "./active-tools.js";
import type { RuntimeResponse } from "./runtime-client.js";
import { listGameSaves, loadResumeSnapshot, validateGameReadiness } from "./runtime-client.js";
import { injectResumeContext, type SessionContextTarget } from "./session-context.js";
import {
  applyDiagnosticsMode,
  createInitialGameState,
  diagnosticsStatusMessage,
  parseDiagnosticsMode,
  type GameSessionState,
} from "./state.js";

export type GenmiconCommandId =
  | "genmicon:validate"
  | "genmicon:play"
  | "genmicon:dev"
  | "genmicon:saves";

export interface GenmiconCommand {
  id: GenmiconCommandId;
  title: string;
  description: string;
  handler: (args: string, ctx: ExtensionCommandContext) => Promise<void> | void;
}

export interface ExtensionCommandContext {
  cwd?: string;
  tools?: ActiveToolTarget;
  session?: SessionContextTarget;
  gameState?: GameSessionState;
  setGameState?: (state: GameSessionState) => void;
  ui?: {
    notify?: (message: string, level?: "info" | "warn" | "error" | "success") => void;
    openView?: (viewId: string, data: unknown) => void;
  };
}

export interface CommandDependencies {
  validateGame?: typeof validateGameReadiness;
  listSaves?: typeof listGameSaves;
  loadResume?: typeof loadResumeSnapshot;
  installPlayerTools?: typeof installPlayerActiveTools;
}

export function createCommandRegistry(dependencies: CommandDependencies = {}): GenmiconCommand[] {
  const validateGame = dependencies.validateGame ?? validateGameReadiness;
  const listSaves = dependencies.listSaves ?? listGameSaves;
  const loadResume = dependencies.loadResume ?? loadResumeSnapshot;
  const installPlayerTools = dependencies.installPlayerTools ?? installPlayerActiveTools;
  return [
    {
      id: "genmicon:validate",
      title: "/genmicon:validate",
      description: "Validate the local package, cartridge, driver, and save.",
      handler: async (args, ctx) => {
        const { gameRoot, saveId } = parseGamePathArgs(args, ctx.cwd);
        const response = await validateGame(gameRoot, saveId ? { saveId } : {});
        const level = response.ok && blockingWarnings(response).length === 0 ? "success" : "warn";
        ctx.ui?.notify?.(formatValidationResult(response), level);
      },
    },
    {
      id: "genmicon:play",
      title: "/genmicon:play",
      description: "Start player mode for a local cartridge.",
      handler: async (args, ctx) => {
        const { gameRoot, saveId } = parseGamePathArgs(args, ctx.cwd);
        const response = await validateGame(gameRoot, saveId ? { saveId } : {});
        const blockers = blockingWarnings(response);
        if (!response.ok || blockers.length > 0) {
          ctx.ui?.notify?.(formatValidationResult(response), "warn");
          return;
        }

        const resume = await loadResume(gameRoot, saveId ? { saveId } : {});
        if (!resume.ok) {
          ctx.ui?.notify?.(`GENmicon resume failed: ${resume.error?.message ?? "unknown error"}`, "warn");
          return;
        }

        const profile = installPlayerTools(ctx.tools);
        injectResumeContext(ctx.session, resume);
        ctx.ui?.openView?.("genmicon.gameConsole", {
          gameRoot,
          saveId,
          validation: response.data,
          resume: resume.data,
          activeTools: profile.activeTools,
        });
        ctx.ui?.notify?.(formatPlayerLaunchResult(response), "success");
      },
    },
    {
      id: "genmicon:dev",
      title: "/genmicon:dev",
      description: "Toggle or report developer diagnostics.",
      handler: (args, ctx) => {
        const mode = parseDiagnosticsMode(args);
        const current = ctx.gameState ?? createInitialGameState();
        const next = applyDiagnosticsMode(current, mode);
        if (next !== current) {
          ctx.setGameState?.(next);
        }
        ctx.ui?.notify?.(diagnosticsStatusMessage(next), "info");
      },
    },
    {
      id: "genmicon:saves",
      title: "/genmicon:saves",
      description: "List available saves for a local cartridge.",
      handler: async (args, ctx) => {
        const { gameRoot } = parseGamePathArgs(args, ctx.cwd);
        const response = await listSaves(gameRoot);
        ctx.ui?.notify?.(formatSaveListResult(response), response.ok ? "info" : "warn");
      },
    },
  ];
}

export function formatPlayerLaunchResult(response: RuntimeResponse): string {
  const game = readString(response.data, "/game/id") ?? "unknown-game";
  const revision = readNumber(response.data, "/save/revision");
  const revisionText = revision === undefined ? "unknown revision" : `revision ${revision}`;
  return `GENmicon player mode started for ${game} at ${revisionText}.`;
}

export function formatSaveListResult(response: RuntimeResponse): string {
  if (!response.ok) {
    return `GENmicon saves failed: ${response.error?.message ?? "unknown error"}`;
  }
  const saves = readPointer(response.data, "/saves");
  if (!Array.isArray(saves) || saves.length === 0) {
    return "GENmicon saves: none found.";
  }
  const formatted = saves.map((save) => {
    const id = readPointer(save, "/id") ?? "unknown";
    const revision = readPointer(save, "/revision") ?? "unknown";
    const driver = readPointer(save, "/driver/id") ?? "unknown-driver";
    return `${id} rev ${revision} (${driver})`;
  });
  return `GENmicon saves: ${formatted.join("; ")}`;
}

export function parseGamePathArgs(args: string, cwd = process.cwd()): { gameRoot: string; saveId?: string } {
  const parts = args.trim().split(/\s+/).filter(Boolean);
  const gamePath = parts[0] ?? ".";
  const saveIndex = parts.indexOf("--save");
  const result: { gameRoot: string; saveId?: string } = {
    gameRoot: resolve(cwd, gamePath),
  };
  const parsedSaveId = saveIndex >= 0 ? parts[saveIndex + 1] : undefined;
  if (parsedSaveId) {
    result.saveId = parsedSaveId;
  }
  return result;
}

export function formatValidationResult(response: RuntimeResponse): string {
  if (!response.ok) {
    return `GENmicon validation failed: ${response.error?.message ?? "unknown error"}`;
  }

  const game = readString(response.data, "/game/id") ?? "unknown-game";
  const driver = readString(response.data, "/driver/id") ?? "unknown-driver";
  const revision = readNumber(response.data, "/save/revision");
  const warningText = response.warnings.length > 0 ? ` (${response.warnings.length} warning(s))` : "";
  const revisionText = revision === undefined ? "unknown revision" : `revision ${revision}`;
  return `GENmicon validation passed for ${game} using ${driver}, ${revisionText}${warningText}.`;
}

export function blockingWarnings(response: RuntimeResponse): string[] {
  if (!response.ok) {
    return [response.error?.message ?? "validation failed"];
  }
  return response.warnings.filter((warning) =>
    /blocked|missing|not found|does not exist|unreviewed/i.test(warning),
  );
}

function readString(data: unknown, pointer: string): string | undefined {
  const value = readPointer(data, pointer);
  return typeof value === "string" ? value : undefined;
}

function readNumber(data: unknown, pointer: string): number | undefined {
  const value = readPointer(data, pointer);
  return typeof value === "number" ? value : undefined;
}

function readPointer(data: unknown, pointer: string): unknown {
  const parts = pointer.split("/").slice(1);
  let current = data;
  for (const part of parts) {
    if (!current || typeof current !== "object" || !(part in current)) {
      return undefined;
    }
    current = (current as Record<string, unknown>)[part];
  }
  return current;
}
