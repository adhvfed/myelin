const utf8 = new TextEncoder();
const MAX_ARTIFACT_REF_BYTES = 4 * 1024;
const SUBSYSTEMS = new Set([
  "git", "ci", "issue", "knowledge", "chat", "notif", "signal", "identity", "agent", "refs",
]);
const TYPES = new Set([
  "pr", "ref", "review", "comment", "repo", "commit", "blob", "run", "check", "log", "artifact",
  "deployment", "pipeline", "runner", "issue", "initiative", "relation", "page", "doc", "row",
  "channel", "message", "thread", "read_state", "permission", "member", "project", "edge",
]);

export interface ArtifactRefParts {
  tenant: string;
  subsystem: string;
  type: string;
  id: string;
  sub: string | null;
  root: string;
}

export function parseArtifactRef(value: unknown): ArtifactRefParts | null {
  if (typeof value !== "string" || utf8.encode(value).byteLength > MAX_ARTIFACT_REF_BYTES ||
      [...value].some((character) => character.charCodeAt(0) <= 0x20 || character.charCodeAt(0) === 0x7f)) {
    return null;
  }
  const match = /^myelin:\/\/([^/]+)\/([^/]+)\/([^/]+)\/([^/#]+)(?:#(.+))?$/.exec(value);
  if (!match?.[1] || !match[2] || !match[3] || !match[4] ||
      !SUBSYSTEMS.has(match[2]) || !TYPES.has(match[3]) || !canonicalSub(match[5])) return null;
  return {
    tenant: match[1],
    subsystem: match[2],
    type: match[3],
    id: match[4],
    sub: match[5] ?? null,
    root: `myelin://${match[1]}/${match[2]}/${match[3]}/${match[4]}`,
  };
}

const MAX_U64 = 18_446_744_073_709_551_615n;

function canonicalUnsigned(value: string): bigint | null {
  if (!/^(?:0|[1-9][0-9]*)$/.test(value)) return null;
  const parsed = BigInt(value);
  return parsed <= MAX_U64 ? parsed : null;
}

function canonicalSub(value: string | undefined): boolean {
  if (value === undefined) return true;
  for (const prefix of ["comment-", "thread-", "message-", "row-", "field-", "check-"]) {
    if (value.startsWith(prefix)) return value.length > prefix.length;
  }
  if (value.startsWith("step-")) return canonicalUnsigned(value.slice("step-".length)) !== null;
  const line = /^L([^/]+)-L([^/]+)$/.exec(value);
  const start = line?.[1] ? canonicalUnsigned(line[1]) : null;
  const end = line?.[2] ? canonicalUnsigned(line[2]) : null;
  if (start !== null && end !== null) {
    return end >= start;
  }
  if (/^[bh].+/.test(value)) return true;
  const commit = /^commit-([^/]+)\/(?:check-([^/]+)|ci-result)$/.exec(value);
  return Boolean(commit?.[1] && (commit[2] !== ""));
}

export function isArtifactRef(value: unknown): value is string {
  return parseArtifactRef(value) !== null;
}

export function isStorableArtifactRef(value: unknown): value is string {
  return isArtifactRef(value) && utf8.encode(value).byteLength <= 1024;
}

export function artifactRefLabel(reference: string): string {
  const parsed = parseArtifactRef(reference);
  if (!parsed) return "Linked work";
  if (parsed.subsystem === "issue" && parsed.type === "issue") return parsed.id;
  if (parsed.subsystem === "git" && parsed.type === "pr") {
    const separator = parsed.id.lastIndexOf(":");
    if (separator > 0 && /^[1-9][0-9]*$/.test(parsed.id.slice(separator + 1))) {
      return `${parsed.id.slice(0, separator)} #${parsed.id.slice(separator + 1)}`;
    }
  }
  if (parsed.subsystem === "knowledge" && parsed.type === "page") {
    return `Knowledge · ${parsed.id.slice(-6)}`;
  }
  if (parsed.subsystem === "ci" && parsed.type === "run") return `CI run · ${parsed.id.slice(-8)}`;
  return `${parsed.type} · ${parsed.id.length > 16 ? `…${parsed.id.slice(-12)}` : parsed.id}`;
}

export function artifactRefHref(reference: string): string | undefined {
  const parsed = parseArtifactRef(reference);
  if (!parsed) return undefined;
  if (parsed.subsystem === "issue" && parsed.type === "issue") {
    return `/issues?state=all&key=${encodeURIComponent(parsed.id)}`;
  }
  if (parsed.subsystem === "knowledge" && parsed.type === "page") {
    return `/knowledge?page=${encodeURIComponent(parsed.id)}`;
  }
  if (parsed.subsystem === "git" && parsed.type === "repo") {
    return `/git/repos/${encodeURIComponent(parsed.id)}`;
  }
  if (parsed.subsystem === "git" && parsed.type === "pr") {
    const separator = parsed.id.lastIndexOf(":");
    const number = parsed.id.slice(separator + 1);
    if (separator > 0 && /^[1-9][0-9]*$/.test(number)) {
      return `/git/repos/${encodeURIComponent(parsed.id.slice(0, separator))}/prs/${number}`;
    }
  }
  if (parsed.subsystem === "ci" && parsed.type === "run") {
    return `/ci/runs/${encodeURIComponent(parsed.id)}`;
  }
  return undefined;
}
