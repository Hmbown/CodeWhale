import { createCommandRegistry, type ExtensionCommandContext } from "./commands.js";
import { createGameToolDefinitions, gameToolNames } from "./tools.js";
import { playerToolProfile } from "./active-tools.js";
import { createGameConsoleModel } from "./ui/game-console.js";
export { buildResumeContext, injectResumeContext } from "./session-context.js";

export interface PiLikeExtensionAPI {
  registerCommand?: (
    name: string,
    options: {
      description: string;
      handler: (args: string, ctx: unknown) => Promise<void> | void;
    },
  ) => void;
  registerTool?: (tool: unknown) => void;
  registerRenderer?: (renderer: unknown) => void;
  registerView?: (view: unknown) => void;
}

export default function activate(pi: PiLikeExtensionAPI): void {
  const commands = createCommandRegistry();
  for (const command of commands) {
    pi.registerCommand?.(command.id, {
      description: command.description,
      handler: (args, ctx) => command.handler(args, ctx as ExtensionCommandContext),
    });
  }

  for (const tool of createGameToolDefinitions()) {
    pi.registerTool?.(tool);
  }

  pi.registerView?.({
    id: "genmicon.gameConsole",
    title: "GENmicon",
    model: createGameConsoleModel(),
  });
}

export const genmiconPackage = {
  id: "genmicon-pi",
  activeTools: playerToolProfile.activeTools,
  tools: gameToolNames,
};
