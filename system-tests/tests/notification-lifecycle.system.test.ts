import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { ExternalEventBus } from "../src/event-bus.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import {
  findInboxItem,
  markInboxViewRead,
  notificationSignalEnvelope,
  pullRequestSubject,
  readInboxItem,
  readInboxPage,
  snoozeInboxItem,
} from "../src/journeys/inbox.js";
import { string } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

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
    const subject = pullRequestSubject(slug, 1);
    const envelope = notificationSignalEnvelope({
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

    await bus.publish(notificationSignalEnvelope({
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
    const addressed = await systemClient.json(
      `/v1/notif/inbox/${encodeURIComponent(itemId)}`,
    );
    expect(addressed.body).toMatchObject({
      id: itemId,
      subject,
      reason: "mentioned",
      coalesce_count: 2,
      state: "unread",
    });

    const hidden = await reviewerClient.json(
      `/v1/notif/inbox/${encodeURIComponent(itemId)}`,
      { expectedStatus: 404 },
    );
    expect(hidden.body).toHaveProperty("error.code", "not_found");

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

  test("retires every derived inbox item when its signal resolves", async () => {
    const dedupKey = uniqueName("resolvable");
    const subject = pullRequestSubject(slug, 6);
    const mention = {
      actor: systemTestConfig.reviewerPrincipal,
      recipient: systemTestConfig.principal,
      dedupKey,
      subject,
    };
    await bus.publish(notificationSignalEnvelope(mention));
    await eventually(
      () => findInboxItem(systemClient, subject),
      { description: "the resolvable mention to reach the durable inbox" },
    );

    await bus.publish(notificationSignalEnvelope({ ...mention, state: "Resolved" }));
    const retired = await eventually(async () => {
      const item = await findInboxItem(systemClient, subject);
      return item?.state === "done" ? item : undefined;
    }, { description: "the resolved signal to retire its inbox item" });
    expect(retired.state).toBe("done");
  });

  test("lets a person put work aside and clear only the view they chose", async () => {
    const mention = {
      actor: systemTestConfig.reviewerPrincipal,
      recipient: systemTestConfig.principal,
      dedupKey: uniqueName("snoozed-mention"),
      subject: pullRequestSubject(slug, 7),
      reason: "mentioned" as const,
    };
    const reviewRequest = {
      actor: systemTestConfig.reviewerPrincipal,
      recipient: systemTestConfig.principal,
      dedupKey: uniqueName("review-request"),
      subject: pullRequestSubject(slug, 8),
      reason: "review_requested" as const,
    };

    await bus.publish(notificationSignalEnvelope(mention));
    await bus.publish(notificationSignalEnvelope(reviewRequest));
    const { mentionedItem, reviewItem } = await eventually(async () => {
      const [mentionedItem, reviewItem] = await Promise.all([
        findInboxItem(systemClient, mention.subject),
        findInboxItem(systemClient, reviewRequest.subject),
      ]);
      return mentionedItem && reviewItem ? { mentionedItem, reviewItem } : undefined;
    }, { description: "both kinds of work to arrive in the person's inbox" });
    const mentionedId = string(mentionedItem.id, "mentioned notification id");
    const reviewId = string(reviewItem.id, "review-request notification id");

    const past = await systemClient.json(
      `/v1/notif/inbox/${encodeURIComponent(mentionedId)}/snooze`,
      {
        method: "POST",
        body: { until: new Date(Date.now() - 1_000).toISOString() },
        idempotencyKey: false,
        expectedStatus: 400,
      },
    );
    expect(past.body).toHaveProperty("error.code", "bad_request");

    const until = new Date(Date.now() + 6_000).toISOString();
    const snoozed = await snoozeInboxItem(systemClient, mentionedId, until);
    expect(snoozed.id).toBe(mentionedId);
    expect(Date.parse(snoozed.snoozeUntil)).toBe(Date.parse(until));
    expect(await readInboxItem(systemClient, mentionedId)).toMatchObject({
      state: "snoozed",
      subject: mention.subject,
    });

    const activePage = await readInboxPage(systemClient, null, 100);
    expect(activePage.items.some((item) => item.id === mentionedId)).toBe(false);

    const updated = await markInboxViewRead(systemClient, "review-requests");
    expect(updated).toBeGreaterThanOrEqual(1);
    await eventually(async () => {
      const item = await readInboxItem(systemClient, reviewId);
      return item.state === "read" ? item : undefined;
    }, { description: "the chosen review-request view to become read" });
    expect(await readInboxItem(systemClient, mentionedId)).toHaveProperty("state", "snoozed");

    await eventually(async () => {
      const item = await readInboxItem(systemClient, mentionedId);
      return item.state === "unread" ? item : undefined;
    }, {
      description: "the snoozed mention to return when its time arrives",
      timeoutMs: 15_000,
    });
    expect(await readInboxItem(systemClient, reviewId)).toHaveProperty("state", "read");

    await bus.publish(notificationSignalEnvelope({ ...mention, state: "Resolved" }));
    await bus.publish(notificationSignalEnvelope({ ...reviewRequest, state: "Resolved" }));
    await eventually(async () => {
      const states = await Promise.all([
        readInboxItem(systemClient, mentionedId),
        readInboxItem(systemClient, reviewId),
      ]);
      return states.every((item) => item.state === "done") ? true : undefined;
    }, { description: "both completed pieces of inbox work to retire" });

    const terminal = await systemClient.json(
      `/v1/notif/inbox/${encodeURIComponent(mentionedId)}/snooze`,
      {
        method: "POST",
        body: { until: new Date(Date.now() + 60_000).toISOString() },
        idempotencyKey: false,
        expectedStatus: 409,
      },
    );
    expect(terminal.body).toHaveProperty("error.code", "conflict");
  });

  test("suppresses self-notifications before processing a later delivery", async () => {
    const selfSubject = pullRequestSubject(slug, 2);
    await bus.publish(notificationSignalEnvelope({
      actor: systemTestConfig.principal,
      recipient: systemTestConfig.principal,
      dedupKey: uniqueName("self"),
      subject: selfSubject,
    }));

    const markerSubject = pullRequestSubject(slug, 3);
    await bus.publish(notificationSignalEnvelope({
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

  test("walks the whole inbox without losing or repeating a notification", async () => {
    const subjects = [4, 5].map((number) => pullRequestSubject(slug, number));
    for (const [index, subject] of subjects.entries()) {
      await bus.publish(notificationSignalEnvelope({
        actor: systemTestConfig.reviewerPrincipal,
        recipient: systemTestConfig.principal,
        dedupKey: uniqueName(`page-${index + 1}`),
        subject,
      }));
    }
    await eventually(async () => {
      const found = await Promise.all(subjects.map((subject) => findInboxItem(systemClient, subject)));
      return found.every((item) => item !== undefined) ? true : undefined;
    }, { description: "both notifications to reach the durable inbox" });

    const itemIds = new Set<string>();
    const cursors = new Set<string>();
    const foundSubjects = new Set<string>();
    let cursor: string | null = null;
    for (let page = 0; page < 500 && foundSubjects.size < subjects.length; page += 1) {
      const current = await readInboxPage(systemClient, cursor, 1);
      expect(current.items.length).toBeLessThanOrEqual(1);
      for (const item of current.items) {
        expect(itemIds.has(string(item.id, "paged notification id"))).toBe(false);
        itemIds.add(string(item.id, "paged notification id"));
        const subject = string(item.subject, "paged notification subject");
        if (subjects.includes(subject)) foundSubjects.add(subject);
      }
      if (current.nextCursor === null) break;
      cursor = current.nextCursor;
      expect(cursors.has(cursor)).toBe(false);
      cursors.add(cursor);
    }
    expect(foundSubjects).toEqual(new Set(subjects));
  });
});
