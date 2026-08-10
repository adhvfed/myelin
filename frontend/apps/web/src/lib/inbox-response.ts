const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

export type InboxStateToken = "unread" | "seen" | "read" | "snoozed" | "archived" | "done";

export interface AutomationApprovalAction {
  kind: "automation_firing_approval";
  automation_id: string;
  event_id: string;
}

export interface AgentEffectApprovalAction {
  kind: "agent_effect_approval";
  gate_id: string;
  run_id: string;
}

export type InboxAction = AutomationApprovalAction | AgentEffectApprovalAction;

export interface InboxItem {
  id: string;
  reason: string;
  class: string;
  subsystem: "issue" | "chat" | "git" | "knowledge" | "ci" | "unknown";
  subject: string;
  subject_root: string;
  coalesce_count: number;
  state: InboxStateToken;
  snooze_until: string | null;
  occurred_at: string;
  priority: 15 | 35 | 55 | 70 | 90;
  action: InboxAction | null;
}

export interface InboxPage {
  items: InboxItem[];
  page: { next_cursor: string | null; limit: number };
}

export interface InboxReadReceipt {
  id: string;
  state: "read";
}

const REASONS = new Set([
  "approval_requested", "escalated", "sla", "review_requested", "assigned", "mentioned",
  "replied", "agent_proposal", "watched", "state_changed", "fyi", "blocked", "unblocked",
  "thread_watched", "shared", "comments",
]);
const CLASSES = new Set(["critical", "direct", "participating", "watching", "fyi"]);
const SUBSYSTEMS = new Set(["issue", "chat", "git", "knowledge", "ci", "unknown"]);
const STATES = new Set(["unread", "seen", "read", "snoozed", "archived", "done"]);
const PRIORITIES = new Set([15, 35, 55, 70, 90]);

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(value).every((key) => allowed.has(key));
}

function boundedText(value: unknown, maximum: number): value is string {
  return typeof value === "string" && value.length > 0 &&
    utf8.encode(value).byteLength <= maximum && !/[\p{Cc}]/u.test(value);
}

function timestamp(value: unknown): value is string {
  return boundedText(value, 64) && Number.isFinite(Date.parse(value));
}

function item(value: unknown): InboxItem | null {
  const row = record(value);
  if (!row || !exact(row, [
    "id", "reason", "class", "subsystem", "subject", "subject_root", "coalesce_count",
    "state", "snooze_until", "occurred_at", "priority", "action",
  ])) return null;
  const action = row.action === null ? null : record(row.action);
  if (action !== null) {
    const automation = action.kind === "automation_firing_approval" &&
      exact(action, ["kind", "automation_id", "event_id"]) &&
      boundedText(action.automation_id, 36) &&
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
        action.automation_id,
      ) && boundedText(action.event_id, 255);
    const agentEffect = action.kind === "agent_effect_approval" &&
      exact(action, ["kind", "gate_id", "run_id"]) &&
      boundedText(action.gate_id, 37) && /^gate:[0-9a-f]{32}$/.test(action.gate_id) &&
      boundedText(action.run_id, 255) && !action.run_id.includes("/");
    if (!automation && !agentEffect) return null;
  }
  if (!boundedText(row.id, 512) || !REASONS.has(row.reason as string) ||
      !CLASSES.has(row.class as string) || !SUBSYSTEMS.has(row.subsystem as string) ||
      !boundedText(row.subject, 512) || !row.subject.startsWith("myelin://") ||
      !boundedText(row.subject_root, 512) || !row.subject_root.startsWith("myelin://") ||
      !Number.isSafeInteger(row.coalesce_count) || (row.coalesce_count as number) < 1 ||
      (row.coalesce_count as number) > 2_147_483_647 || !STATES.has(row.state as string) ||
      (row.snooze_until !== null && !timestamp(row.snooze_until)) ||
      !timestamp(row.occurred_at) || !PRIORITIES.has(row.priority as number)) return null;
  return row as unknown as InboxItem;
}

/** Strictly decode the recipient-scoped structured inbox contract. Surplus or malformed fields fail. */
export function parseInboxPage(value: unknown): InboxPage | null {
  const envelope = record(value);
  const page = record(envelope?.page);
  if (!envelope || !exact(envelope, ["items", "page"]) || !Array.isArray(envelope.items) ||
      !page || !exact(page, ["next_cursor", "limit"]) || !Number.isSafeInteger(page.limit) ||
      (page.limit as number) < 1 || (page.limit as number) > 100 ||
      envelope.items.length > (page.limit as number) ||
      (page.next_cursor !== null &&
        (!boundedText(page.next_cursor, 1_024) || !page.next_cursor.startsWith("ni1_")))) return null;
  const items = envelope.items.map(item);
  return items.every((row): row is InboxItem => row !== null)
    ? { items, page: { next_cursor: page.next_cursor as string | null, limit: page.limit as number } }
    : null;
}

export function parseInboxReadReceipt(value: unknown): InboxReadReceipt | null {
  const receipt = record(value);
  if (!receipt || !exact(receipt, ["id", "state"]) ||
      !boundedText(receipt.id, 512) || receipt.state !== "read") return null;
  return receipt as unknown as InboxReadReceipt;
}

export function inboxReasonLabel(reason: string): string {
  const labels: Record<string, string> = {
    approval_requested: "Approval requested",
    escalated: "Escalation",
    sla: "SLA alert",
    review_requested: "Review requested",
    assigned: "Assigned to you",
    mentioned: "Mentioned you",
    replied: "New reply",
    agent_proposal: "Agent proposal",
    watched: "Watched item changed",
    state_changed: "State changed",
    fyi: "For your information",
    blocked: "Item blocked",
    unblocked: "Item unblocked",
    thread_watched: "Watched thread changed",
    shared: "Shared with you",
    comments: "New comments",
  };
  return labels[reason] ?? "Notification";
}
