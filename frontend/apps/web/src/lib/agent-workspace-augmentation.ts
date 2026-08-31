const utf8 = new TextEncoder();
const PREFIX = "myelin.workspace-augmentation.v1\0";
const MAX_BYTES = 12 * 1_024;

export interface AgentWorkspaceFile {
  path: string;
  contents: string;
  strategy: "create";
}

function validPath(path: unknown): path is string {
  if (typeof path !== "string" || !path || path.trim() !== path ||
      utf8.encode(path).byteLength > 4_096 || path.startsWith("/") ||
      path.includes("\\") || path.includes("\0")) return false;
  const segments = path.split("/");
  return !segments.some((segment) => !segment || segment === "." || segment === "..") &&
    path !== "AGENTS.md" && segments[0] !== ".git";
}

export function parseAgentWorkspaceFiles(value: unknown): AgentWorkspaceFile[] | null {
  if (!Array.isArray(value) || value.length > 8) return null;
  const files: AgentWorkspaceFile[] = [];
  const paths = new Set<string>();
  for (const candidate of value) {
    if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) return null;
    const file = candidate as Record<string, unknown>;
    if (Object.keys(file).length !== 3 ||
        !Object.keys(file).every((key) => ["path", "contents", "strategy"].includes(key)) ||
        !validPath(file.path) || paths.has(file.path) || typeof file.contents !== "string" ||
        file.contents.includes("\0") || file.strategy !== "create") return null;
    paths.add(file.path);
    files.push(file as unknown as AgentWorkspaceFile);
  }
  return files;
}

export function agentWorkspaceAugmentationFits(
  instructions: string | null,
  files: readonly AgentWorkspaceFile[],
): boolean {
  if (files.length === 0) {
    return instructions === null || utf8.encode(instructions).byteLength <= MAX_BYTES;
  }
  const payload = `${PREFIX}${JSON.stringify({
    version: 1,
    ...(instructions === null ? {} : { agents_md: instructions }),
    files,
  })}`;
  return utf8.encode(payload).byteLength <= MAX_BYTES;
}

export function validAgentWorkspaceAugmentation(
  instructions: string | null,
  files: unknown,
): files is AgentWorkspaceFile[] {
  const parsed = parseAgentWorkspaceFiles(files);
  return parsed !== null && agentWorkspaceAugmentationFits(instructions, parsed);
}
