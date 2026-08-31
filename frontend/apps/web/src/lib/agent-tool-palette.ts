export interface PersonalAgentTool {
  name: string;
  label: string;
  description: string;
}

export interface AgentToolDefinition {
  name: string;
  subsystem: string;
  version: number;
  effect_kind: "read" | "compute" | "mutate" | "external";
  side_effecting: boolean;
  requires_approval: boolean;
  exposed_over_mcp: boolean;
}

export interface AgentToolDefinitionPage {
  items: AgentToolDefinition[];
  page: { next_cursor: string | null; limit: number };
}

export const PERSONAL_AGENT_TOOLS: readonly PersonalAgentTool[] = [
  {
    name: "chat.read_messages",
    label: "Read conversation history",
    description: "Read messages in conversations this agent is allowed to join.",
  },
  {
    name: "chat.post",
    label: "Post messages",
    description: "Post a new message in an allowed conversation.",
  },
  {
    name: "chat.reply",
    label: "Reply in threads",
    description: "Reply to an existing message in an allowed conversation.",
  },
  {
    name: "workspace.read_file",
    label: "Read workspace files",
    description: "Read files inside the generation-fenced private workspace.",
  },
  {
    name: "workspace.write_file",
    label: "Write workspace files",
    description: "Create and edit files inside the private workspace.",
  },
  {
    name: "workspace.exec",
    label: "Run bounded commands",
    description: "Run generation-fenced, network-denied commands with durable receipts.",
  },
] as const;

export const DEFAULT_PERSONAL_AGENT_TOOLS = PERSONAL_AGENT_TOOLS
  .filter((tool) => tool.name !== "workspace.exec")
  .map((tool) => tool.name);

const TOOL_NAME = /^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+$/;

export function validAgentToolPalette(value: unknown): value is string[] {
  return Array.isArray(value) && value.length > 0 && value.length <= 128 &&
    value.every((tool) => typeof tool === "string" && tool.length <= 255 && TOOL_NAME.test(tool)) &&
    new Set(value).size === value.length;
}

function record(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function exact(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(value).length === keys.length && Object.keys(value).every((key) => allowed.has(key));
}

export function parseAgentToolDefinitionPage(value: unknown): AgentToolDefinitionPage | null {
  const envelope = record(value);
  const paging = record(envelope?.page);
  if (!envelope || !exact(envelope, ["items", "page"]) || !Array.isArray(envelope.items) ||
      !paging || !exact(paging, ["next_cursor", "limit"]) ||
      !Number.isSafeInteger(paging.limit) || (paging.limit as number) < 1 ||
      (paging.limit as number) > 100 || envelope.items.length > (paging.limit as number) ||
      (paging.next_cursor !== null &&
        (typeof paging.next_cursor !== "string" || paging.next_cursor.length > 512))) return null;
  const items: AgentToolDefinition[] = [];
  for (const item of envelope.items) {
    const tool = record(item);
    if (!tool || !exact(tool, [
      "name", "ref", "subsystem", "version", "input_schema", "required_capabilities",
      "effect_kind", "side_effecting", "requires_approval", "exposed_over_mcp",
    ]) || typeof tool.name !== "string" || !TOOL_NAME.test(tool.name) ||
        typeof tool.ref !== "string" || tool.ref.length > 4_096 ||
        typeof tool.subsystem !== "string" || tool.subsystem.length > 64 ||
        !Number.isSafeInteger(tool.version) || (tool.version as number) < 1 ||
        !record(tool.input_schema) || !Array.isArray(tool.required_capabilities) ||
        tool.required_capabilities.length > 512 ||
        !tool.required_capabilities.every((grant) => typeof grant === "string" && grant.length > 0 && grant.length <= 4_096) ||
        !["read", "compute", "mutate", "external"].includes(tool.effect_kind as string) ||
        typeof tool.side_effecting !== "boolean" || typeof tool.requires_approval !== "boolean" ||
        typeof tool.exposed_over_mcp !== "boolean") return null;
    items.push({
      name: tool.name,
      subsystem: tool.subsystem,
      version: tool.version,
      effect_kind: tool.effect_kind,
      side_effecting: tool.side_effecting,
      requires_approval: tool.requires_approval,
      exposed_over_mcp: tool.exposed_over_mcp,
    } as AgentToolDefinition);
  }
  if (new Set(items.map((tool) => tool.name)).size !== items.length) return null;
  return {
    items,
    page: {
      next_cursor: paging.next_cursor as string | null,
      limit: paging.limit as number,
    },
  };
}
