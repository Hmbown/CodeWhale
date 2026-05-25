export interface RenderedToolResult {
  title: string;
  body: string;
  developerDetail?: string;
}

export function renderToolResult(toolName: string, result: unknown, developer = false): RenderedToolResult {
  if (developer) {
    return {
      title: toolName,
      body: summarizeResult(result),
      developerDetail: JSON.stringify(result, null, 2),
    };
  }

  return {
    title: toolName.replace(/^game_/, "").replaceAll("_", " "),
    body: summarizeResult(result),
  };
}

export function renderGameToolResult(toolName: string, result: unknown, developer = false): RenderedToolResult {
  const rendered = renderToolResult(toolName, result, developer);
  if (developer) {
    return rendered;
  }
  return {
    title: rendered.title,
    body: rendered.body.replace(/[{}[\]"]/g, ""),
  };
}

export function renderPlayerMessage(message: string): string {
  return message
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .join("\n");
}

export function renderDeveloperExpansion(toolName: string, result: unknown): RenderedToolResult {
  return {
    title: `${toolName} diagnostics`,
    body: summarizeResult(result),
    developerDetail: JSON.stringify(result, null, 2),
  };
}

function summarizeResult(result: unknown): string {
  if (result && typeof result === "object" && "summary" in result) {
    return String((result as { summary?: unknown }).summary ?? "");
  }
  if (result && typeof result === "object" && "revision" in result) {
    return `Revision ${(result as { revision?: unknown }).revision}`;
  }
  return "Game state updated.";
}
