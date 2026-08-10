const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

export const AUTOMATION_PAGE_LIMIT = 25;

export type AutomationState = "active" | "paused" | "disabled";
export type AutomationFiringState =
  | "queued"
  | "awaiting_approval"
  | "claimed"
  | "started"
  | "terminal";
export type AutomationRunOutcome =
  | "succeeded"
  | "failed"
  | "terminated"
  | "nondeterministic";

export interface AutomationVM {
  id: string;
  ref: string;
  owner_principal_id: string;
  run_as_agent_id: string;
  event_type: string;
  subject_type: string | null;
  condition: string | null;
  matcher: WireRecord;
  task: string;
  delegation_caveats: string[];
  budget_minor_units: number;
  max_firings: number;
  firings_used: number;
  max_causal_depth: number;
  require_no_personal_data: boolean;
  require_human_approval: boolean;
  state: AutomationState;
  created_at: string;
}

export interface AutomationApprovalVM {
  decision: "approved" | "rejected";
  decided_by: string;
  decided_at: string;
}

export interface AutomationFiringVM {
  event_id: string;
  event_type: string;
  trigger_ref: string;
  state: AutomationFiringState;
  run_id: string | null;
  run_ref: string | null;
  outcome: AutomationRunOutcome | null;
  result_state: "available" | "erased" | null;
  approval: AutomationApprovalVM | null;
  created_at: string;
}

export interface AutomationResultVM {
  run_id: string;
  run_ref: string;
  trace_ref: string;
  agent_principal: string;
  answer: string;
  charged_micro: number;
  recorded_at: string;
}

export interface AutomationErasureVM {
  run_id: string;
  run_ref: string;
  trace_ref: string;
  erased: true;
  already_erased: boolean;
  available_results: 0;
  recreation_blocked: true;
}

export interface AutomationLifecycleVM {
  action: "pause" | "resume" | "disable";
  changed: boolean;
  canceled_firings: number;
  durable: true;
  trigger: AutomationVM;
}

export interface AutomationPage {
  items: AutomationVM[];
  page: { next_cursor: string | null; limit: number };
}

export interface AutomationFiringPage {
  items: AutomationFiringVM[];
  page: { next_cursor: string | null; limit: number };
}

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  const actual = Object.keys(value);
  return actual.length === keys.length && actual.every((key) => allowed.has(key));
}

function text(value: unknown, maximum: number, multiline = false): value is string {
  if (typeof value !== "string" || value.length === 0 || utf8.encode(value).byteLength > maximum) {
    return false;
  }
  return [...value].every((character) => {
    const point = character.codePointAt(0)!;
    return point > 0x1f && point !== 0x7f || multiline && [0x09, 0x0a, 0x0d].includes(point);
  });
}

export function isAutomationId(value: unknown): value is string {
  return typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
}

function timestamp(value: unknown): value is string {
  return text(value, 64) && Number.isFinite(Date.parse(value));
}

function safeInteger(value: unknown, minimum: number, maximum: number): value is number {
  return Number.isSafeInteger(value) && (value as number) >= minimum && (value as number) <= maximum;
}

function automation(value: unknown): AutomationVM | null {
  const row = record(value);
  const matcher = record(row?.matcher);
  const caveats = row?.delegation_caveats;
  const referenceTenant = referencePart(row?.ref, "identity", "trigger", row?.id);
  if (!row || !exact(row, [
    "id", "ref", "owner_principal_id", "run_as_agent_id", "event_type", "subject_type",
    "condition", "matcher", "task", "delegation_caveats", "budget_minor_units",
    "max_firings", "firings_used", "max_causal_depth", "require_no_personal_data",
    "require_human_approval", "state", "created_at",
  ]) || !isAutomationId(row.id) || !isAutomationId(row.run_as_agent_id) || !referenceTenant ||
      !text(row.owner_principal_id, 255) || !text(row.event_type, 255) ||
      (row.subject_type !== null && !text(row.subject_type, 64)) ||
      (row.condition !== null && !text(row.condition, 4_096, true)) || !matcher ||
      !text(row.task, 16_384, true) || !Array.isArray(caveats) || caveats.length > 64 ||
      !caveats.every((item) => text(item, 1_024)) ||
      !safeInteger(row.budget_minor_units, 1, 1_000_000_000_000) ||
      !safeInteger(row.max_firings, 1, 1_000_000) ||
      !safeInteger(row.firings_used, 0, row.max_firings as number) ||
      !safeInteger(row.max_causal_depth, 0, 64) ||
      typeof row.require_no_personal_data !== "boolean" ||
      typeof row.require_human_approval !== "boolean" ||
      !["active", "paused", "disabled"].includes(row.state as string) ||
      !timestamp(row.created_at)) return null;
  return row as unknown as AutomationVM;
}

function referencePart(
  reference: unknown,
  subsystem: string,
  type: string,
  id: unknown,
): string | null {
  if (typeof reference !== "string" || typeof id !== "string") return null;
  const match = /^myelin:\/\/([^/]+)\/([^/]+)\/([^/]+)\/([^/]+)$/.exec(reference);
  return match && match[2] === subsystem && match[3] === type && match[4] === id
    ? match[1]!
    : null;
}

function approval(value: unknown): AutomationApprovalVM | null {
  const row = record(value);
  if (!row || !exact(row, ["decision", "decided_by", "decided_at"]) ||
      !["approved", "rejected"].includes(row.decision as string) ||
      !text(row.decided_by, 255) || !timestamp(row.decided_at)) return null;
  return row as unknown as AutomationApprovalVM;
}

function firing(value: unknown): AutomationFiringVM | null {
  const row = record(value);
  const triggerId = typeof row?.trigger_ref === "string" ? row.trigger_ref.split("/").at(-1) : null;
  const triggerTenant = referencePart(
    row?.trigger_ref,
    "identity",
    "trigger",
    triggerId,
  );
  const runTenant = row?.run_id === null ? null : referencePart(row?.run_ref, "agent", "run", row?.run_id);
  if (!row || !exact(row, [
    "event_id", "event_type", "trigger_ref", "state", "run_id", "run_ref", "outcome",
    "result_state", "approval", "created_at",
  ]) || !text(row.event_id, 255) || !text(row.event_type, 255) || !triggerTenant ||
      !isAutomationId(triggerId) ||
      !["queued", "awaiting_approval", "claimed", "started", "terminal"].includes(row.state as string) ||
      (row.run_id !== null && !isAutomationId(row.run_id)) ||
      (row.run_ref !== null && runTenant !== triggerTenant) ||
      (row.run_id === null && row.run_ref !== null) ||
      (row.outcome !== null && !["succeeded", "failed", "terminated", "nondeterministic"].includes(row.outcome as string)) ||
      (row.result_state !== null && !["available", "erased"].includes(row.result_state as string)) ||
      (row.result_state !== null && row.run_id === null) ||
      (row.approval !== null && approval(row.approval) === null) || !timestamp(row.created_at)) return null;
  return row as unknown as AutomationFiringVM;
}

function page(value: unknown): { next_cursor: string | null; limit: number } | null {
  const row = record(value);
  if (!row || !exact(row, ["next_cursor", "limit"]) ||
      !safeInteger(row.limit, 1, 100) ||
      (row.next_cursor !== null && !text(row.next_cursor, 1_024))) return null;
  return row as unknown as { next_cursor: string | null; limit: number };
}

export function parseAutomationPage(value: unknown): AutomationPage | null {
  const envelope = record(value);
  const pagination = page(envelope?.page);
  if (!envelope || !exact(envelope, ["items", "page"]) || !pagination ||
      !Array.isArray(envelope.items) || envelope.items.length > pagination.limit) return null;
  const items = envelope.items.map(automation);
  return items.every((item): item is AutomationVM => item !== null)
    ? { items, page: pagination }
    : null;
}

export function parseAutomation(value: unknown): AutomationVM | null {
  const envelope = record(value);
  return envelope && exact(envelope, ["trigger"])
    ? automation(envelope.trigger)
    : null;
}

export function parseAutomationFiringPage(value: unknown): AutomationFiringPage | null {
  const envelope = record(value);
  const pagination = page(envelope?.page);
  if (!envelope || !exact(envelope, ["items", "page"]) || !pagination ||
      !Array.isArray(envelope.items) || envelope.items.length > pagination.limit) return null;
  const items = envelope.items.map(firing);
  return items.every((item): item is AutomationFiringVM => item !== null)
    ? { items, page: pagination }
    : null;
}

export function parseAutomationResult(value: unknown): AutomationResultVM | null {
  const envelope = record(value);
  const row = record(envelope?.result);
  const runTenant = referencePart(row?.run_ref, "agent", "run", row?.run_id);
  const traceId = typeof row?.trace_ref === "string" ? row.trace_ref.split("/").at(-1) : null;
  const traceTenant = referencePart(row?.trace_ref, "knowledge", "doc", traceId);
  if (!envelope || !exact(envelope, ["result"]) || !row || !exact(row, [
    "run_id", "run_ref", "trace_ref", "agent_principal", "answer", "charged_micro", "recorded_at",
  ]) || !isAutomationId(row.run_id) || !runTenant || runTenant !== traceTenant ||
      typeof traceId !== "string" || !/^blake3:[0-9a-f]{64}$/.test(traceId) ||
      !text(row.agent_principal, 255) || !text(row.answer, 4 * 1024 * 1024, true) ||
      !safeInteger(row.charged_micro, 0, Number.MAX_SAFE_INTEGER) || !timestamp(row.recorded_at)) {
    return null;
  }
  return row as unknown as AutomationResultVM;
}

export function parseAutomationErasure(value: unknown): AutomationErasureVM | null {
  const envelope = record(value);
  const row = record(envelope?.erasure);
  const runTenant = referencePart(row?.run_ref, "agent", "run", row?.run_id);
  const traceId = typeof row?.trace_ref === "string" ? row.trace_ref.split("/").at(-1) : null;
  const traceTenant = referencePart(row?.trace_ref, "knowledge", "doc", traceId);
  if (!envelope || !exact(envelope, ["erasure"]) || !row || !exact(row, [
    "run_id", "run_ref", "trace_ref", "erased", "already_erased", "available_results",
    "recreation_blocked",
  ]) || !isAutomationId(row.run_id) || !runTenant || runTenant !== traceTenant ||
      typeof traceId !== "string" || !/^blake3:[0-9a-f]{64}$/.test(traceId) ||
      row.erased !== true || typeof row.already_erased !== "boolean" ||
      row.available_results !== 0 || row.recreation_blocked !== true) return null;
  return row as unknown as AutomationErasureVM;
}

export function parseAutomationLifecycle(value: unknown): AutomationLifecycleVM | null {
  const row = record(value);
  const trigger = automation(row?.trigger);
  if (!row || !exact(row, ["action", "changed", "canceled_firings", "durable", "trigger"]) ||
      !["pause", "resume", "disable"].includes(row.action as string) ||
      typeof row.changed !== "boolean" || !safeInteger(row.canceled_firings, 0, 1_000_000) ||
      row.durable !== true || !trigger) return null;
  return { ...row, trigger } as AutomationLifecycleVM;
}
