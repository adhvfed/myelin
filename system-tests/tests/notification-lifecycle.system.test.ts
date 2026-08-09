import { randomUUID } from "node:crypto";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import type { SystemTestClient } from "../src/client.js";
import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { ExternalEventBus, type ExternalEventEnvelope } from "../src/event-bus.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { array, record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

interface SignalOptions {
  eventId?: string;
  actor: string;
  recipient: string;
  dedupKey: string;
  subject: string;
}

function signalEnvelope(options: SignalOptions): ExternalEventEnvelope {
  const eventId = options.eventId ?? randomUUID();
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
    type_: "signal.opened",
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
      state: "Open",
      first_seen: now,
      last_seen: now,
      mentions: [{ Mention: principal(options.recipient) }],
    },
  };
}

async function findInboxItem(
  client: SystemTestClient,
  subject: string,
): Promise<JsonRecord | undefined> {
  const response = await client.json("/v1/notif/inbox?view=all&limit=100");
  return array(response.body.items, "notification inbox items")
    .map((item) => record(item, "notification inbox item"))
    .find((item) => item.subject === subject);
}

describe.sequential("notification delivery lifecycle", () => {
  const slug = uniqueName("system-notif");
  const project = new GitProject(slug, systemClient);
  let bus: ExternalEventBus;

  beforeAll(async () => {
    await project.create();
    bus = await ExternalEventBus.connect(systemTestConfig.natsUrl);
  });

  afterAll(async () => {
    await bus?.close();
  });

  test("routes, de-duplicates, collapses, scopes, and marks a durable mention read", async () => {
    const dedupKey = uniqueName("mention");
    const subject = `myelin://${systemTestConfig.tenant}/git/pr/${slug}:1`;
    const envelope = signalEnvelope({
      actor: systemTestConfig.reviewerPrincipal,
      recipient: systemTestConfig.principal,
      dedupKey,
      subject,
    });

    const firstAck = await bus.publish(envelope);
    expect(firstAck.duplicate).toBe(false);
    const duplicateAck = await bus.publish(envelope);
    expect(duplicateAck.duplicate).toBe(true);

    const delivered = await eventually(
      () => findInboxItem(systemClient, subject),
      { description: "the notification worker to persist the mentioned repository owner's inbox row" },
    );
    expect(delivered).toMatchObject({
      reason: "mentioned",
      class: "direct",
      subsystem: "git",
      subject,
      coalesce_count: 1,
      state: "unread",
    });
    expect(await findInboxItem(reviewerClient, subject)).toBeUndefined();

    await bus.publish(signalEnvelope({
      actor: systemTestConfig.reviewerPrincipal,
      recipient: systemTestConfig.principal,
      dedupKey,
      subject,
    }));
    const collapsed = await eventually(async () => {
      const item = await findInboxItem(systemClient, subject);
      return item?.coalesce_count === 2 ? item : undefined;
    }, { description: "the notification worker to collapse a repeated signal" });
    expect(collapsed.coalesce_count).toBe(2);

    const itemId = string(collapsed.id, "notification inbox item id");
    const inaccessible = await reviewerClient.json(
      `/v1/notif/inbox/${encodeURIComponent(itemId)}/read`,
      {
        method: "POST",
        body: {},
        expectedStatus: 404,
      },
    );
    expect(inaccessible.body).toHaveProperty("error.code", "not_found");

    const read = await systemClient.json(`/v1/notif/inbox/${encodeURIComponent(itemId)}/read`, {
      method: "POST",
      body: {},
    });
    expect(read.body).toEqual({ id: itemId, state: "read" });
    const marked = await eventually(async () => {
      const item = await findInboxItem(systemClient, subject);
      return item?.state === "read" ? item : undefined;
    }, { description: "the marked-read notification state" });
    expect(marked.state).toBe("read");
  });

  test("suppresses self-notifications before processing a later delivery", async () => {
    const selfSubject = `myelin://${systemTestConfig.tenant}/git/pr/${slug}:self`;
    await bus.publish(signalEnvelope({
      actor: systemTestConfig.principal,
      recipient: systemTestConfig.principal,
      dedupKey: uniqueName("self"),
      subject: selfSubject,
    }));

    const markerSubject = `myelin://${systemTestConfig.tenant}/git/pr/${slug}:marker`;
    await bus.publish(signalEnvelope({
      actor: systemTestConfig.reviewerPrincipal,
      recipient: systemTestConfig.principal,
      dedupKey: uniqueName("marker"),
      subject: markerSubject,
    }));
    await eventually(
      () => findInboxItem(systemClient, markerSubject),
      { description: "the delivery ordered after the self-notification" },
    );
    expect(await findInboxItem(systemClient, selfSubject)).toBeUndefined();
  });
});
