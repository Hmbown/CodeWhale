import { spawn } from "node:child_process";

export type RuntimeCommand =
  | "validate"
  | "status"
  | "render"
  | "playbook"
  | "lookup"
  | "fact_check"
  | "run_driver"
  | "commit_turn"
  | "list_saves";

export interface RuntimeRequest {
  command: RuntimeCommand;
  game_root: string;
  save_id?: string;
  developer?: boolean;
  payload?: unknown;
}

export interface RuntimeResponse<T = unknown> {
  ok: boolean;
  data: T | null;
  warnings: string[];
  error: RuntimeError | null;
}

export interface RuntimeError {
  code: string;
  message: string;
  recoverable: boolean;
}

export interface RuntimeClientOptions {
  binary?: string;
  args?: readonly string[];
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  timeoutMs?: number;
}

export interface ValidateGameOptions extends RuntimeClientOptions {
  saveId?: string;
  developer?: boolean;
}

export interface SaveListOptions extends RuntimeClientOptions {
  developer?: boolean;
}

export interface ResumeSnapshotOptions extends RuntimeClientOptions {
  saveId?: string;
  developer?: boolean;
}

export function encodeRuntimeRequest(request: RuntimeRequest): string {
  return `${JSON.stringify({ developer: false, payload: {}, ...request })}\n`;
}

export async function callRuntime<T = unknown>(
  request: RuntimeRequest,
  options: RuntimeClientOptions = {},
): Promise<RuntimeResponse<T>> {
  const binary = options.binary ?? "genmicon-game-runtime";
  const child = spawn(binary, [...(options.args ?? [])], {
    stdio: ["pipe", "pipe", "pipe"],
    ...(options.cwd ? { cwd: options.cwd } : {}),
    ...(options.env ? { env: options.env } : {}),
  });

  const stdout: Buffer[] = [];
  const stderr: Buffer[] = [];
  let settled = false;

  const timeout = setTimeout(() => {
    if (!settled) {
      child.kill("SIGTERM");
    }
  }, options.timeoutMs ?? 30_000);

  child.stdout?.on("data", (chunk: Buffer) => stdout.push(chunk));
  child.stderr?.on("data", (chunk: Buffer) => stderr.push(chunk));
  child.stdin?.end(encodeRuntimeRequest(request));

  return new Promise((resolve) => {
    child.on("error", (error) => {
      settled = true;
      clearTimeout(timeout);
      resolve(runtimeClientError(error.message));
    });

    child.on("close", (code, signal) => {
      settled = true;
      clearTimeout(timeout);
      const raw = Buffer.concat(stdout).toString("utf8").trim();
      const stderrText = Buffer.concat(stderr).toString("utf8").trim();
      if (raw.length === 0) {
        resolve(
          runtimeClientError(
            signal ? `runtime helper terminated by ${signal}` : stderrText || `runtime helper exited with ${code}`,
          ),
        );
        return;
      }

      try {
        resolve(JSON.parse(raw) as RuntimeResponse<T>);
      } catch (error) {
        resolve(runtimeClientError(`invalid runtime JSON: ${(error as Error).message}`));
      }
    });
  });
}

function runtimeClientError(message: string): RuntimeResponse<never> {
  return {
    ok: false,
    data: null,
    warnings: [],
    error: {
      code: "runtime_client_error",
      message,
      recoverable: true,
    },
  };
}

export async function validateGameReadiness(
  gameRoot: string,
  options: ValidateGameOptions = {},
): Promise<RuntimeResponse> {
  const { saveId, developer, ...clientOptions } = options;
  const request: RuntimeRequest = {
    command: "validate",
    game_root: gameRoot,
    developer: developer ?? false,
    payload: {},
  };
  if (saveId) {
    request.save_id = saveId;
  }
  return callRuntime(request, clientOptions);
}

export async function listGameSaves(
  gameRoot: string,
  options: SaveListOptions = {},
): Promise<RuntimeResponse> {
  const { developer, ...clientOptions } = options;
  return callRuntime(
    {
      command: "list_saves",
      game_root: gameRoot,
      developer: developer ?? false,
      payload: {},
    },
    clientOptions,
  );
}

export async function loadResumeSnapshot(
  gameRoot: string,
  options: ResumeSnapshotOptions = {},
): Promise<RuntimeResponse> {
  const { saveId, developer, ...clientOptions } = options;
  const base = {
    game_root: gameRoot,
    ...(saveId ? { save_id: saveId } : {}),
    developer: developer ?? false,
    payload: {},
  };
  const status = await callRuntime({ command: "status", ...base }, clientOptions);
  if (!status.ok) {
    return status;
  }
  const render = await callRuntime({ command: "render", ...base }, clientOptions);
  if (!render.ok) {
    return render;
  }
  return {
    ok: true,
    data: {
      status: status.data,
      render: render.data,
    },
    warnings: [...status.warnings, ...render.warnings],
    error: null,
  };
}
