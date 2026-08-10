import { describe, expect, it } from "vitest";

import { inboxReasonLabel, parseInboxPage, parseInboxReadReceipt } from "./inbox-response";

const valid = {
  items: [{
    id: "01JITEM",
    reason: "review_requested",
    class: "direct",
    subsystem: "git",
    subject: "myelin://acme/git/pr/core:42",
    subject_root: "myelin://acme/git/pr/core:42",
    coalesce_count: 2,
    state: "unread",
    snooze_until: null,
    occurred_at: "2026-07-22T12:00:00.000000Z",
    priority: 70,
    action: null,
  }],
  page: { next_cursor: "ni1_abc", limit: 50 },
};

describe("notification inbox response", () => {
  it("accepts the exact bounded structured contract", () => {
    expect(parseInboxPage(valid)).toEqual(valid);
    expect(inboxReasonLabel("review_requested")).toBe("Review requested");
  });

  it("accepts one exact automation approval action", () => {
    const action = {
      kind: "automation_firing_approval",
      automation_id: "44444444-4444-4444-8444-444444444444",
      event_id: "issue-owner-updated-1",
    } as const;
    expect(parseInboxPage({
      ...valid,
      items: [{ ...valid.items[0], reason: "approval_requested", action }],
    })?.items[0]?.action).toEqual(action);
  });

  it.each([
    { ...valid, extra: true },
    { ...valid, page: { next_cursor: "opaque", limit: 50 } },
    { ...valid, page: { next_cursor: null, limit: 0 } },
    { ...valid, items: [{ ...valid.items[0], title: "server must not send rendered PII" }] },
    { ...valid, items: [{ ...valid.items[0], subject: "https://example.test/private" }] },
    { ...valid, items: [{ ...valid.items[0], priority: 71 }] },
    { ...valid, items: [{ ...valid.items[0], state: "new" }] },
    { ...valid, items: [{ ...valid.items[0], action: {} }] },
    { ...valid, items: [{ ...valid.items[0], action: {
      kind: "automation_firing_approval",
      automation_id: "not-an-id",
      event_id: "event-1",
    } }] },
  ])("rejects malformed or surplus wire data %#", (wire) => {
    expect(parseInboxPage(wire)).toBeNull();
  });

  it("accepts only the exact mark-read receipt", () => {
    expect(parseInboxReadReceipt({ id: "01JITEM", state: "read" })).toEqual({
      id: "01JITEM",
      state: "read",
    });
    expect(parseInboxReadReceipt({ id: "01JITEM", state: "unread" })).toBeNull();
    expect(parseInboxReadReceipt({ id: "01JITEM", state: "read", extra: true })).toBeNull();
  });
});
