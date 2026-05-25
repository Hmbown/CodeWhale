export interface GameConsoleModel {
  id: "genmicon.gameConsole";
  regions: readonly string[];
  fallback: "compact-text";
}

export interface GameViewSnapshot {
  revision: number;
  scene_title: string;
  scene: string;
  figure_title: string;
  figure: string;
  status: readonly string[];
  items: readonly string[];
  tasks: readonly string[];
  dialogue: readonly string[];
  choices: readonly string[];
  validation: string;
}

export interface GameConsoleLayout {
  width: number;
  mode: "compact" | "medium" | "wide";
  regions: readonly string[];
}

export interface ActionComposerState {
  value: string;
  canSubmit: boolean;
  placeholder: string;
}

export function createGameConsoleModel(): GameConsoleModel {
  return {
    id: "genmicon.gameConsole",
    regions: ["scene", "status", "items", "tasks", "dialogue", "choices", "composer"],
    fallback: "compact-text",
  };
}

export function layoutForWidth(width: number): GameConsoleLayout {
  if (width < 80) {
    return {
      width,
      mode: "compact",
      regions: ["scene", "status", "dialogue", "choices", "composer"],
    };
  }
  if (width < 120) {
    return {
      width,
      mode: "medium",
      regions: ["scene", "figure", "status", "tasks", "dialogue", "choices", "composer"],
    };
  }
  return {
    width,
    mode: "wide",
    regions: ["scene", "figure", "status", "items", "tasks", "dialogue", "choices", "composer"],
  };
}

export function renderCompactGameView(view: GameViewSnapshot): string {
  return [
    `${view.scene_title} (rev ${view.revision})`,
    view.scene,
    section("Status", view.status),
    section("Dialogue", view.dialogue),
    section("Choices", view.choices),
  ]
    .filter((line) => line.trim().length > 0)
    .join("\n\n");
}

export function createActionComposerState(value = ""): ActionComposerState {
  return {
    value,
    canSubmit: value.trim().length > 0,
    placeholder: "What do you do?",
  };
}

export function updateActionComposer(value: string): ActionComposerState {
  return createActionComposerState(value);
}

function section(title: string, values: readonly string[]): string {
  if (values.length === 0) {
    return "";
  }
  return `${title}: ${values.join(" | ")}`;
}
