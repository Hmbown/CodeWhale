import type { RuntimeResponse } from "./runtime-client.js";

export interface SessionContextTarget {
  injectContext?: (message: string) => void;
}

export function buildResumeContext(response: RuntimeResponse): string {
  const gameId = readPointer(response.data, "/status/game/id") ?? "unknown-game";
  const saveId = readPointer(response.data, "/status/save/id") ?? "default";
  const revision = readPointer(response.data, "/status/save/revision") ?? "unknown";
  const driverId = readPointer(response.data, "/status/driver/id") ?? "unknown-driver";
  const sceneTitle = readPointer(response.data, "/render/view/scene_title") ?? "Scene";
  return [
    "GENmicon resume context:",
    `game=${gameId}`,
    `save=${saveId}`,
    `revision=${revision}`,
    `driver=${driverId}`,
    `scene=${sceneTitle}`,
    "STATE.json and TURN_LOG.jsonl remain authoritative.",
  ].join("\n");
}

export function injectResumeContext(target: SessionContextTarget | undefined, response: RuntimeResponse): void {
  target?.injectContext?.(buildResumeContext(response));
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
