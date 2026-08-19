// Notification journeys: publishing mention signals the way producers do
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
import { systemTestConfig } from "../config.js";

export interface MentionOptions {
  eventId?: string;
  actor: string;
  recipient: string;
  dedupKey: string;
  subject: string;
  /// "Open" (the default) delivers; "Resolved" retires every inbox item the
  /// dedup key produced - the router flips them to `done`.
  state?: "Open" | "Resolved";
}

export function pullRequestSubject(repository: string, number: number): string {
  if (!Number.isSafeInteger(number) || number < 1) {
    throw new Error("a notification journey needs a canonical positive pull-request number");
  }
  return `myelin://${systemTestConfig.tenant}/git/pr/${repository}:${number}`;
}

export function mentionSignalEnvelope(options: MentionOptions): ExternalEventEnvelope {
  const eventId = options.eventId ?? randomUUID();
  const state = options.state ?? "Open";
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
  let cursor: string | null = null;
  const seenCursors = new Set<string>();
  for (let page = 0; page < 100; page += 1) {
    const current = await readInboxPage(client, cursor, 100);
    const found = current.items.find((item) => item.subject === subject);
    if (found !== undefined || current.nextCursor === null) return found;
    if (seenCursors.has(current.nextCursor)) {
      throw new Error("notification inbox repeated an opaque cursor");
    }
    seenCursors.add(current.nextCursor);
    cursor = current.nextCursor;
  }
  throw new Error("notification inbox exceeded the bounded search");
}

/// Publishes a mention and waits until it is readable in the inbox.
export async function deliverMention(
  bus: ExternalEventBus,
  client: SystemTestClient,
  options: MentionOptions,
): Promise<JsonRecord> {
  await bus.publish(mentionSignalEnvelope(options));
  return eventually(
    () => findInboxItem(client, options.subject),
    { description: `the mention for ${options.subject} to reach the durable inbox` },
  );
}

/// Counts the recipient's inbox items whose subject starts with `prefix`,
/// walking the whole paged inbox with the standard cursor guards.
export async function countInboxItems(
  client: SystemTestClient,
  prefix: string,
): Promise<number> {
  let cursor: string | null = null;
  const seenCursors = new Set<string>();
  let count = 0;
  for (let page = 0; page < 200; page += 1) {
    const current = await readInboxPage(client, cursor, 100);
    count += current.items.filter(
      (item) => typeof item.subject === "string" && item.subject.startsWith(prefix),
    ).length;
    if (current.nextCursor === null) return count;
    if (seenCursors.has(current.nextCursor)) {
      throw new Error("notification inbox repeated an opaque cursor");
    }
    seenCursors.add(current.nextCursor);
    cursor = current.nextCursor;
  }
  throw new Error("notification inbox exceeded the bounded count walk");
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
  options: { actor: string; recipient: string; subjectPrefix: string; count: number },
): Promise<void> {
  for (let index = 0; index < options.count; index += 1) {
    await bus.publish(mentionSignalEnvelope({
      actor: options.actor,
      recipient: options.recipient,
      dedupKey: `${options.subjectPrefix.split("/").pop() ?? "seed"}-${index + 1}`,
      subject: `${options.subjectPrefix}:${index + 1}`,
    }));
  }
  await eventually(
    async () => {
      const landed = await countInboxItems(client, `${options.subjectPrefix}:`);
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
  options: { actor: string; recipient: string; subjectPrefix: string; count: number },
): Promise<void> {
  for (let index = 0; index < options.count; index += 1) {
    await bus.publish(mentionSignalEnvelope({
      actor: options.actor,
      recipient: options.recipient,
      dedupKey: `${options.subjectPrefix.split("/").pop() ?? "seed"}-${index + 1}`,
      subject: `${options.subjectPrefix}:${index + 1}`,
      state: "Resolved",
    }));
  }
}
