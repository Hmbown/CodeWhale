import { Type, type TSchema } from "typebox";

import type { RuntimeCommand, RuntimeRequest, RuntimeResponse } from "./runtime-client.js";
import { callRuntime } from "./runtime-client.js";

export const gameToolNames = [
  "game_status",
  "game_render",
  "game_playbook",
  "game_lookup",
  "game_fact_check",
  "game_run_driver",
  "game_commit_turn",
] as const;

export type GameToolName = (typeof gameToolNames)[number];

export interface GameToolInput {
  game_root: string;
  save_id?: string;
  [key: string]: unknown;
}

export interface GameToolDefinition {
  name: GameToolName;
  description: string;
  runtimeCommand: RuntimeCommand;
  inputSchema: TSchema;
  mutatesSave: boolean;
  playerSafe: boolean;
  sequential: boolean;
}

export type RuntimeCaller = (request: RuntimeRequest) => Promise<RuntimeResponse<unknown>>;

export function createGameToolDefinitions(): GameToolDefinition[] {
  return [
    tool("game_status", "Read active cartridge, driver, save, warning, and readiness status.", "status", statusSchema()),
    tool("game_render", "Read the current player view and render panels.", "render", statusSchema()),
    tool("game_playbook", "Read allowed player actions, choices, and turn guidance.", "playbook", statusSchema()),
    tool("game_lookup", "Read bounded cartridge content by handle or query.", "lookup", lookupSchema()),
    tool("game_fact_check", "Check proposed action or resolution against protected continuity facts.", "fact_check", factCheckSchema()),
    tool("game_run_driver", "Run a declared deterministic driver function.", "run_driver", runDriverSchema()),
    tool(
      "game_commit_turn",
      "Sequentially commit one authoritative turn after an expected-revision check.",
      "commit_turn",
      commitTurnSchema(),
      true,
    ),
  ];
}

export function gameToolDefinition(name: GameToolName): GameToolDefinition {
  const definition = createGameToolDefinitions().find((toolDefinition) => toolDefinition.name === name);
  if (!definition) {
    throw new Error(`unknown game tool: ${name}`);
  }
  return definition;
}

export async function executeGameTool<T = unknown>(
  name: GameToolName,
  input: GameToolInput,
  runtimeCaller: RuntimeCaller = callRuntime,
): Promise<RuntimeResponse<T>> {
  assertAuthoritativeToolInput(name, input);
  const definition = gameToolDefinition(name);
  const { game_root: gameRoot, save_id: saveId, ...payload } = input;
  const response = await runtimeCaller({
    command: definition.runtimeCommand,
    game_root: gameRoot,
    ...(saveId ? { save_id: saveId } : {}),
    developer: false,
    payload,
  });
  return response as RuntimeResponse<T>;
}

export function assertAuthoritativeToolInput(name: GameToolName, input: GameToolInput): void {
  if (name !== "game_commit_turn") {
    return;
  }
  for (const forbidden of ["transcript_state", "transcript_patch", "transcript_summary", "conversation_state"]) {
    if (forbidden in input) {
      throw new Error(`game_commit_turn cannot write transcript-derived field: ${forbidden}`);
    }
  }
}

export function extractRefreshedView(response: RuntimeResponse): unknown | undefined {
  if (!response.ok || !response.data || typeof response.data !== "object") {
    return undefined;
  }
  return (response.data as { view?: unknown }).view;
}

function tool(
  name: GameToolName,
  description: string,
  runtimeCommand: RuntimeCommand,
  inputSchema: TSchema,
  mutatesSave = false,
): GameToolDefinition {
  return {
    name,
    description,
    runtimeCommand,
    inputSchema,
    mutatesSave,
    playerSafe: true,
    sequential: mutatesSave,
  };
}

function baseSchema(extra: Record<string, TSchema> = {}): TSchema {
  return Type.Object({
    game_root: Type.String(),
    save_id: Type.Optional(Type.String()),
    ...extra,
  });
}

function statusSchema(): TSchema {
  return baseSchema();
}

function lookupSchema(): TSchema {
  return baseSchema({
    handle: Type.Optional(Type.String()),
    query: Type.Optional(Type.String()),
    max_bytes: Type.Optional(Type.Number()),
  });
}

function factCheckSchema(): TSchema {
  return baseSchema({
    player_action: Type.Optional(Type.String()),
    resolution: Type.Optional(Type.String()),
    text: Type.Optional(Type.String()),
  });
}

function runDriverSchema(): TSchema {
  return baseSchema({
    function: Type.String(),
    args: Type.Optional(Type.Record(Type.String(), Type.Unknown())),
  });
}

function commitTurnSchema(): TSchema {
  return baseSchema({
    expected_revision: Type.Number(),
    player_input: Type.String(),
    resolution: Type.String(),
    state_patch: Type.Record(Type.String(), Type.Unknown()),
    driver_results: Type.Optional(Type.Record(Type.String(), Type.Unknown())),
    metadata: Type.Optional(Type.Record(Type.String(), Type.Unknown())),
  });
}
