// The inbox-at-scale guarantee. Born from a live incident: ~1,100 accumulated
// inbox rows pushed every read past its budget (per-row authorization at
// ~20ms/row) and buried fresh mentions behind stale items, while writes stayed
// sub-second. This test seeds a deliberately large inbox and pins the two
// properties that incident violated:
//
//   1. a fresh mention is readable on the FIRST page, within a hard budget,
//      no matter how much older material the inbox holds;
//   2. a single page read stays within a per-page budget.
//
// The seed lands in the REVIEWER's inbox so the primary principal's inbox -
// which the other notification journeys walk - stays lean, and the seed is
// retired (signal-resolved -> done) afterwards so this test never becomes the
// debris it guards against.
import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { reviewerClient, uniqueName } from "../src/context.js";
import { ExternalEventBus } from "../src/event-bus.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import {
  findInboxItem,
  notificationSignalEnvelope,
  readInboxPage,
  retireSeededInbox,
  seedInbox,
} from "../src/journeys/inbox.js";
import { systemTestConfig } from "../src/config.js";

const SEED_COUNT = 250;
const STALE_APPROVAL_COUNT = 30;
const FRESH_MENTION_BUDGET_MS = 15_000;
const PAGE_READ_BUDGET_MS = 5_000;

describe.sequential("notification inbox at scale", () => {
  // The subjects must name an artifact the recipient can actually read: the
  // inbox read path authorizes every row against its subject and silently
  // drops the unreadable ones (fail-closed - a notification must never leak
  // an artifact its viewer cannot see). So the seed mentions PRs of a real
  // repository OWNED BY THE RECIPIENT - a mention about someone else's
  // repository is (correctly) invisible to a reviewer without a pull grant.
  const repoSlug = uniqueName("inbox-scale");
  const staleApprovalRepoSlug = uniqueName("inbox-stale-approvals");
  const seedPrefix = `myelin://${systemTestConfig.tenant}/git/pr/${repoSlug}`;
  const seed = {
    actor: systemTestConfig.principal,
    recipient: systemTestConfig.reviewerPrincipal,
    subjectPrefix: seedPrefix,
    count: SEED_COUNT,
  };
  const staleApprovals = {
    actor: systemTestConfig.principal,
    recipient: systemTestConfig.reviewerPrincipal,
    subjectPrefix:
      `myelin://${systemTestConfig.tenant}/git/pr/${staleApprovalRepoSlug}`,
    count: STALE_APPROVAL_COUNT,
    reason: "approval_requested" as const,
  };
  const freshMention = {
    actor: systemTestConfig.principal,
    recipient: systemTestConfig.reviewerPrincipal,
    dedupKey: `fresh-${repoSlug}`,
    subject: `${seedPrefix}:${SEED_COUNT + 1}`,
  };
  let bus: ExternalEventBus;

  beforeAll(async () => {
    await new GitProject(repoSlug, reviewerClient).create();
    await new GitProject(staleApprovalRepoSlug, reviewerClient).create();
    bus = await ExternalEventBus.connect(systemTestConfig.natsUrl);
    await seedInbox(bus, reviewerClient, seed);
    await seedInbox(bus, reviewerClient, staleApprovals);
    await retireSeededInbox(bus, reviewerClient, staleApprovals);
  }, 360_000);

  afterAll(async () => {
    if (bus !== undefined) {
      await retireSeededInbox(bus, reviewerClient, seed);
      await bus.publish(notificationSignalEnvelope({ ...freshMention, state: "Resolved" }));
      await bus.close();
    }
  }, 360_000);

  test("surfaces fresh work ahead of completed high-priority approvals", async () => {
    const subject = freshMention.subject;
    const publishedAt = Date.now();
    await bus.publish(notificationSignalEnvelope(freshMention));

    const firstPageItem = await eventually(async () => {
      const page = await readInboxPage(reviewerClient, null, 25);
      return page.items.find((item) => item.subject === subject);
    }, {
      description:
        `a fresh mention to surface on the FIRST inbox page over ${SEED_COUNT}+ older items`,
      timeoutMs: FRESH_MENTION_BUDGET_MS,
      intervalMs: 500,
    });
    const surfacedInMs = Date.now() - publishedAt;
    expect(firstPageItem).toMatchObject({ subject, state: "unread" });
    expect(surfacedInMs).toBeLessThan(FRESH_MENTION_BUDGET_MS);
  });

  test("keeps a single page read within its latency budget", async () => {
    const startedAt = Date.now();
    const page = await readInboxPage(reviewerClient, null, 100);
    const elapsedMs = Date.now() - startedAt;
    expect(page.items.length).toBeGreaterThan(0);
    expect(
      elapsedMs,
      `one 100-item inbox page took ${elapsedMs}ms against a ` +
        `${PAGE_READ_BUDGET_MS}ms budget - the read path is degrading with inbox size`,
    ).toBeLessThan(PAGE_READ_BUDGET_MS);
  });

  test("still finds a mid-seed item without losing it behind the scale", async () => {
    const midSubject = `${seedPrefix}:${Math.floor(SEED_COUNT / 2)}`;
    const found = await findInboxItem(reviewerClient, midSubject);
    expect(found).toMatchObject({ subject: midSubject });
  });
});
