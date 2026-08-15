import { isClientNonce } from "./client-nonce";

export const MAX_PROJECT_NAME_BYTES = 100;
export const MAX_PROJECT_PREFIX_BYTES = 10;
export const MAX_PROJECT_PAGE_SIZE = 100;

export interface ProjectVM {
  id: string;
  ref: string;
  name: string;
  issue_prefix: string;
  default_issue_type_id: string;
  created_at: string;
}

export interface ProjectPage {
  items: ProjectVM[];
  page: { next_cursor: string | null; limit: number };
}

export interface ProjectListInput {
  cursor?: string;
  limit?: number;
}

export interface NewProjectInput {
  name: string;
  issuePrefix: string;
  clientNonce: string;
}

export interface ProjectCreationReceipt {
  project: ProjectVM;
  created: boolean;
}

type WireRecord = Record<string, unknown>;

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const PREFIX = /^[A-Z0-9]{2,10}$/;
const utf8 = new TextEncoder();

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const expected = new Set(keys);
  return Object.keys(value).length === expected.size &&
    Object.keys(value).every((key) => expected.has(key));
}

function bounded(value: unknown, maxBytes: number): value is string {
  return typeof value === "string" && value.length > 0 && utf8.encode(value).byteLength <= maxBytes;
}

function hasControl(value: string): boolean {
  return /[\p{Cc}]/u.test(value);
}

function canonicalProjectRef(value: string, id: string): boolean {
  const scheme = "myelin://";
  const suffix = `/identity/project/${id}`;
  if (!value.startsWith(scheme) || !value.endsWith(suffix)) return false;
  const tenant = value.slice(scheme.length, -suffix.length);
  return tenant.length > 0 && tenant.length <= 255 &&
    !tenant.includes("/") && !/[\s\p{Cc}]/u.test(tenant);
}

export function isProjectId(value: unknown): value is string {
  return typeof value === "string" && UUID.test(value);
}

export function projectNameError(value: string): string | null {
  const name = value.trim();
  if (!name) return "Enter a project name.";
  if (name !== value || utf8.encode(name).byteLength > MAX_PROJECT_NAME_BYTES || hasControl(name)) {
    return "Use 1–100 UTF-8 bytes without surrounding whitespace or control characters.";
  }
  return null;
}

export function projectPrefixError(value: string): string | null {
  return PREFIX.test(value)
    ? null
    : "Use 2–10 uppercase letters or numbers.";
}

export function parseProjectListInput(value: unknown): ProjectListInput | null {
  const input = record(value);
  if (!input || !Object.keys(input).every((key) => key === "cursor" || key === "limit")) return null;
  if (input.cursor !== undefined && !isProjectId(input.cursor)) return null;
  if (input.limit !== undefined &&
      (!Number.isSafeInteger(input.limit) || (input.limit as number) < 1 ||
        (input.limit as number) > MAX_PROJECT_PAGE_SIZE)) return null;
  return {
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
    ...(input.limit === undefined ? {} : { limit: input.limit as number }),
  };
}

export function projectListSearchParams(input: ProjectListInput): URLSearchParams {
  const query = new URLSearchParams();
  if (input.cursor) query.set("cursor", input.cursor);
  if (input.limit !== undefined) query.set("limit", String(input.limit));
  return query;
}

export function parseNewProjectInput(value: unknown): NewProjectInput | null {
  const input = record(value);
  if (!input || !exact(input, ["name", "issuePrefix", "clientNonce"]) ||
      typeof input.name !== "string" || typeof input.issuePrefix !== "string" ||
      projectNameError(input.name) || projectPrefixError(input.issuePrefix) ||
      !isClientNonce(input.clientNonce)) return null;
  return { name: input.name, issuePrefix: input.issuePrefix, clientNonce: input.clientNonce };
}

function parseProject(value: unknown): ProjectVM | null {
  const input = record(value);
  if (!input || !exact(input, [
    "id", "ref", "name", "issue_prefix", "default_issue_type_id", "created_at",
  ]) || !isProjectId(input.id) || !bounded(input.ref, 4_096) ||
      !canonicalProjectRef(input.ref, input.id) ||
      !bounded(input.name, MAX_PROJECT_NAME_BYTES) || input.name.trim() !== input.name ||
      hasControl(input.name) || typeof input.issue_prefix !== "string" ||
      projectPrefixError(input.issue_prefix) || !isProjectId(input.default_issue_type_id) ||
      !bounded(input.created_at, 64) || !Number.isFinite(Date.parse(input.created_at))) return null;
  return input as unknown as ProjectVM;
}

export function parseProjectPage(value: unknown): ProjectPage | null {
  const input = record(value);
  const page = record(input?.page);
  if (!input || !exact(input, ["items", "page"]) || !Array.isArray(input.items) ||
      input.items.length > MAX_PROJECT_PAGE_SIZE || !page ||
      !exact(page, ["next_cursor", "limit"]) ||
      !(page.next_cursor === null || isProjectId(page.next_cursor)) ||
      !Number.isSafeInteger(page.limit) || (page.limit as number) < 1 ||
      (page.limit as number) > MAX_PROJECT_PAGE_SIZE) return null;
  const items = input.items.map(parseProject);
  if (items.some((item) => item === null)) return null;
  return { items: items as ProjectVM[], page: {
    next_cursor: page.next_cursor,
    limit: page.limit as number,
  } };
}

export function parseProjectCreation(value: unknown): ProjectCreationReceipt | null {
  const input = record(value);
  const project = parseProject(input?.project);
  if (!input || !exact(input, ["project", "created", "durable"]) || !project ||
      typeof input.created !== "boolean" || input.durable !== true) return null;
  return { project, created: input.created };
}
