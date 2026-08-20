// Notification journeys: publishing signals the way producers do
// (straight onto the event bus) and reading the recipient's durable inbox the
// way clients do (paged, newest relevance first).
//
// `seedInbox` exists because the inbox read path degrades with inbox size -
// a lesson learned live when accumulated test debris pushed reads past their
// budgets. Scale tests seed deliberately instead of relying on leftovers.
import { randomUUID } from "node:crypto";

import type { SystemTestClient } from "../client.js";
import { ExternalEventBus, type ExternalEventEnvelope } from "../event-bus.js";
import { eventually } from "../eventually.js";
import { array, record, string, type JsonRecord } from "../json.js";
import { findPaged, walkPaged } from "../paging.js";
import { systemTestConfig } from "../config.js";

export type NotificationReason =
  | "approval_requested"
  | "escalated"
  | "sla"
  | "review_requested"
  | "assigned"
  | "mentioned"
  | "replied"
  | "agent_proposal"
  | "watched"
  | "state_changed"
  | "fyi"
  | "blocked"
  | "unblocked"
  | "thread_watched"
  | "shared"
  | "comments";

export interface NotificationOptions {
  eventId?: string;
  actor: string;
  recipient: string;
  dedupKey: string;
  subject: string;
  reason?: NotificationReason;
  /// "Open" (the default) delivers; "Resolved" retires every inbox item the
  /// dedup key produced - the router flips them to `done`.
  state?: "Open" | "Resolved";
}

export interface InboxSeed {
  actor: string;
  recipient: string;
  subjectPrefix: string;
  count: number;
  reason?: NotificationReason;
}

export function pullRequestSubject(repository: string, number: number): string {
  if (!Number.isSafeInteger(number) || number < 1) {
    throw new Error("a notification journey needs a canonical positive pull-request number");
  }
  return `myelin://${systemTestConfig.tenant}/git/pr/${repository}:${number}`;
}

export function notificationSignalEnvelope(
  options: NotificationOptions,
): ExternalEventEnvelope {
  const eventId = options.eventId ?? randomUUID();
  const state = options.state ?? "Open";
  const reason = options.reason ?? "mentioned";
  const now = new Date().toISOString();
  const principal = (principalId: string) => ({
    tenant: systemTestConfig.tenant,
    region: systemTestConfig.region,
    principal_id: principalId,
    kind: "Human",
    data_role: "Controller",
    status: "Active",
  });
  return {
    event_id: eventId,
    type_: state === "Resolved" ? "signal.resolved" : "signal.opened",
    schema_ver: 1,
    tenant: systemTestConfig.tenant,
    region: systemTestConfig.region,
    actor: principal(options.actor),
    subject: `sig.${systemTestConfig.tenant}.warning.system_test`,
    aggregate: `signal:${options.dedupKey}`,
    causation_id: null,
    correlation_id: eventId,
    caused_by: null,
    depth: 0,
    contains_personal_data: false,
    data_role: "Controller",
    visibility: "Internal",
    pii_key_ref: null,
    occurred_at: now,
    recorded_at: now,
    payload: {
      rule_id: "system_test",
      tenant: systemTestConfig.tenant,
      severity: "Warning",
      dedup_key: options.dedupKey,
      subject: options.subject,
      count: 1,
      state,
      first_seen: now,
      last_seen: now,
      notification_reason: reason,
      mentions: [{ Mention: principal(options.recipient) }],
    },
  };
}

export async function readInboxPage(
  client: SystemTestClient,
  cursor: string | null,
  limit: number,
): Promise<{ items: JsonRecord[]; nextCursor: string | null }> {
  const search = new URLSearchParams({ view: "all", limit: String(limit) });
  if (cursor !== null) search.set("cursor", cursor);
  const response = await client.json(`/v1/notif/inbox?${search.toString()}`);
  const page = record(response.body.page, "notification page envelope");
  return {
    items: array(response.body.items, "notification inbox items")
      .map((item) => record(item, "notification inbox item")),
    nextCursor: page.next_cursor === null
      ? null
      : string(page.next_cursor, "notification page cursor"),
  };
}

/// Bounded search for one subject across the recipient's whole inbox.
export async function findInboxItem(
  client: SystemTestClient,
  subject: string,
): Promise<JsonRecord | undefined> {
  return findPaged(
    client,
    "/v1/notif/inbox?view=all",
    (item) => item.subject === subject,
  );
}

/// Counts matching items across the recipient's whole inbox through the one
/// shared, guarded cursor walk.
async function countInboxItems(
  client: SystemTestClient,
  predicate: (item: JsonRecord) => boolean,
): Promise<number> {
  let count = 0;
  for await (const item of walkPaged(
    client,
    "/v1/notif/inbox?view=all",
    { maxPages: 200 },
  )) {
    if (predicate(item)) count += 1;
  }
  return count;
}

/// Seeds `count` distinct delivered mentions for one recipient, then waits
/// until every one is readable. Each mention is its own aggregate (distinct
/// dedup key), so "the last one landed" proves nothing about the rest - the
/// wait counts the seeded subjects in the inbox instead.
///
/// The subjectPrefix must name an artifact the recipient can READ: the inbox
/// read path authorizes each row against its subject and silently drops
/// unreadable ones (fail-closed), so a fabricated subject seeds rows that no
/// read will ever return.
export async function seedInbox(
  bus: ExternalEventBus,
  client: SystemTestClient,
  options: InboxSeed,
): Promise<void> {
  const reason = options.reason ?? "mentioned";
  for (let index = 0; index < options.count; index += 1) {
    await bus.publish(notificationSignalEnvelope({
      actor: options.actor,
      recipient: options.recipient,
      dedupKey: `${options.subjectPrefix.split("/").pop() ?? "seed"}-${index + 1}`,
      subject: `${options.subjectPrefix}:${index + 1}`,
      reason,
    }));
  }
  await eventually(
    async () => {
      const landed = await countInboxItems(
        client,
        (item) => typeof item.subject === "string" &&
          item.subject.startsWith(`${options.subjectPrefix}:`) &&
          item.reason === reason,
      );
      return landed >= options.count ? landed : undefined;
    },
    {
      description: `all ${options.count} seeded mentions to be readable in the inbox`,
      timeoutMs: 300_000,
      intervalMs: 5_000,
    },
  );
}

/// Retires a seeded inbox the way the product retires signals: a Resolved
/// signal per seeded dedup key flips every derived inbox item to `done`.
/// Scale tests MUST call this so their seeds do not become the accumulated
/// debris the scale test exists to guard against.
export async function retireSeededInbox(
  bus: ExternalEventBus,
  client: SystemTestClient,
  options: InboxSeed,
): Promise<void> {
  const reason = options.reason ?? "mentioned";
  for (let index = 0; index < options.count; index += 1) {
    await bus.publish(notificationSignalEnvelope({
      actor: options.actor,
      recipient: options.recipient,
      dedupKey: `${options.subjectPrefix.split("/").pop() ?? "seed"}-${index + 1}`,
      subject: `${options.subjectPrefix}:${index + 1}`,
      reason,
      state: "Resolved",
    }));
  }
  await eventually(
    async () => {
      const retired = await countInboxItems(
        client,
        (item) => typeof item.subject === "string" &&
          item.subject.startsWith(`${options.subjectPrefix}:`) &&
          item.reason === reason &&
          item.state === "done",
      );
      return retired >= options.count ? retired : undefined;
    },
    {
      description: `all ${options.count} seeded ${reason} notifications to retire`,
      timeoutMs: 300_000,
      intervalMs: 5_000,
    },
  );
}
