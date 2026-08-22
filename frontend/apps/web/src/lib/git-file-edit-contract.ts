import { isClientNonce } from "./client-nonce";
import { isGitRepositorySlug } from "./git-coordinate";
import { isGitPath, isGitRefName } from "./git-read-input";
import { isBranchRef } from "./git-ref";

const MAX_FILE_CONTENT_BYTES = 512 * 1024;
const MAX_COMMIT_MESSAGE_BYTES = 8 * 1024;
const utf8 = new TextEncoder();

type WireRecord = Record<string, unknown>;

export interface GitFileEditDraft {
  repo: string;
  ref: string;
  path: string;
  baseOid: string;
  contents: string;
  message: string;
  clientNonce: string;
}

export interface GitFileEditReceipt {
  newOid: string;
}

export type GitFileEditError =
  | "bad-input"
  | "not-found"
  | "conflict"
  | "forbidden"
  | "too-large"
  | "unavailable"
  | "error";

export type GitFileEditResult =
  | { ok: true; receipt: GitFileEditReceipt }
  | { ok: false; error: GitFileEditError };

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(value).every((key) => allowed.has(key));
}

function bounded(value: unknown, maximum: number, allowEmpty = false): value is string {
  return typeof value === "string" && (allowEmpty || value.length > 0) &&
    utf8.encode(value).byteLength <= maximum;
}

function oid(value: unknown, allowEmpty: boolean): value is string {
  return (allowEmpty && value === "") ||
    (typeof value === "string" && /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(value));
}

export function isEditableBranch(value: unknown): value is string {
  if (!isGitRefName(value)) return false;
  if (!value.startsWith("refs/heads/") && /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(value)) {
    return false;
  }
  return isBranchRef(value.startsWith("refs/heads/") ? value : `refs/heads/${value}`);
}

export function parseGitFileEditDraft(value: unknown): GitFileEditDraft | null {
  const draft = record(value);
  if (!draft || !exact(draft, [
    "repo", "ref", "path", "baseOid", "contents", "message", "clientNonce",
  ]) || !isGitRepositorySlug(draft.repo) || !isEditableBranch(draft.ref) ||
      !isGitPath(draft.path) || !oid(draft.baseOid, true) ||
      !bounded(draft.contents, MAX_FILE_CONTENT_BYTES, true) || draft.contents.includes("\0") ||
      !bounded(draft.message, MAX_COMMIT_MESSAGE_BYTES) || draft.message.trim() !== draft.message ||
      draft.message.includes("\0") || !isClientNonce(draft.clientNonce)) {
    return null;
  }
  return {
    repo: draft.repo,
    ref: draft.ref,
    path: draft.path,
    baseOid: draft.baseOid,
    contents: draft.contents,
    message: draft.message,
    clientNonce: draft.clientNonce,
  };
}

export function parseGitFileEditReceipt(value: unknown): GitFileEditReceipt | null {
  const envelope = record(value);
  const applied = record(envelope?.applied);
  return envelope && exact(envelope, ["applied", "durable"]) && envelope.durable === true &&
    applied && exact(applied, ["outcome", "new_oid"]) && applied.outcome === "committed" &&
    oid(applied.new_oid, false)
    ? { newOid: applied.new_oid }
    : null;
}
