import { describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import { systemClient } from "../src/context.js";
import { array, record } from "../src/json.js";

function expectError(body: Record<string, unknown>, code: string): void {
  expect(body).toMatchObject({ error: { code } });
  const error = record(body.error, "error envelope");
  expect(error.message).toBeTypeOf("string");
  expect(JSON.stringify(body).length).toBeLessThan(2_048);
  expect(JSON.stringify(body)).not.toMatch(/postgres|sqlx|backtrace|panicked at/i);
}

describe("external API contracts", () => {
  test.each([
    ["Chat", "/v1/chat/conversations?limit=0"],
    ["Chat duplicate query", "/v1/chat/conversations?limit=1&limit=2"],
    ["Knowledge", "/v1/knowledge/pages?limit=101"],
    ["Issues", "/v1/issues?state=OPEN"],
    ["CI", "/v1/ci/runs?limit=01"],
    ["Notifications", "/v1/notif/inbox?view=everything"],
    ["Git code search without text", "/v1/git/search/code?repo=core"],
    ["Git code search with an unknown coordinate", "/v1/git/search/code?q=needle&limit=100"],
    ["Git code search with an invalid repository", "/v1/git/search/code?q=needle&repo=team%2F%2Fcore"],
  ])("rejects malformed %s input", async (_domain, path) => {
    const response = await systemClient.json(path, { expectedStatus: 400 });
    expectError(response.body, "bad_request");
  });

  test.each([
    ["Chat", "/v1/chat/conversations/not-a-ulid/messages"],
    ["Knowledge", "/v1/knowledge/pages/not-a-ulid"],
    ["Issues", "/v1/issues/AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA"],
    ["CI", "/v1/ci/runs/not-a-uuid"],
  ])("requires a canonical %s resource identifier", async (_domain, path) => {
    const response = await systemClient.json(path, { expectedStatus: 400 });
    expectError(response.body, "bad_request");
  });

  test("rejects unknown mutation fields instead of silently discarding them", async () => {
    const chat = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { channel: "contract", topic: "strict input", ignored: true },
      expectedStatus: 400,
    });
    expectError(chat.body, "bad_request");

    const knowledge = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: {
        title: "Strict page",
        template: "blank",
        visibility: "team",
        ignored: true,
      },
      expectedStatus: 400,
    });
    expectError(knowledge.body, "bad_request");

    const issues = await systemClient.json("/v1/issues", {
      method: "POST",
      body: { project_id: "x", type_id: "y", prefix: "Z", title: "Strict issue", ignored: true },
      expectedStatus: 400,
    });
    expectError(issues.body, "bad_request");
  });

  test("keeps retry identity in the standard operation header", async () => {
    const body = {
      title: "Header-scoped retry contract",
      template: "blank",
      visibility: "private",
    };
    const missing = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body,
      idempotencyKey: false,
      expectedStatus: 400,
    });
    expectError(missing.body, "bad_request");

    const legacy = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: { ...body, client_nonce: "legacy-body-token" },
      expectedStatus: 400,
    });
    expectError(legacy.body, "bad_request");

    const messagePath = "/v1/chat/conversations/01J00000000000000000000000/messages";
    const missingMessageKey = await systemClient.json(messagePath, {
      method: "POST",
      body: { content: "Header-scoped retry contract" },
      idempotencyKey: false,
      expectedStatus: 400,
    });
    expectError(missingMessageKey.body, "bad_request");

    const legacyMessageKey = await systemClient.json(messagePath, {
      method: "POST",
      body: {
        content: "Header-scoped retry contract",
        client_nonce: "legacy-body-token",
      },
      expectedStatus: 400,
    });
    expectError(legacyMessageKey.body, "bad_request");
  });

  test("enforces interactive payload limits before durable work", async () => {
    const issues = await systemClient.json("/v1/issues", {
      method: "POST",
      body: { project_id: "x", type_id: "y", prefix: "Z", title: "x".repeat(5_000) },
      expectedStatus: 413,
    });
    expectError(issues.body, "payload_too_large");

    const knowledge = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: {
        title: "x".repeat(321 * 1_024),
        template: "blank",
        visibility: "team",
      },
      expectedStatus: 413,
    });
    expectError(knowledge.body, "payload_too_large");
  });

  test.each([
    ["surrounding whitespace", " padded title "],
    ["line breaks", "line\nbreak"],
    ["control characters", "hidden\u0085control"],
  ])("rejects issue titles with %s", async (_case, title) => {
    const response = await systemClient.json("/v1/issues", {
      method: "POST",
      body: { project_id: systemTestConfig.issues.projectId, title },
      expectedStatus: 400,
    });
    expectError(response.body, "bad_request");
  });

  test("exposes a recipient-scoped, bounded notification inbox", async () => {
    const inbox = await systemClient.json("/v1/notif/inbox?view=all&limit=50");
    expect(array(inbox.body.items, "notification inbox items").length).toBeLessThanOrEqual(50);
    expect(inbox.body).toMatchObject({ page: { limit: 50 } });
    expect(inbox.headers.get("cache-control")).toContain("no-store");

    const reviewRequests = await systemClient.json(
      "/v1/notif/inbox?view=review-requests&limit=25",
    );
    for (const item of array(reviewRequests.body.items, "review-request notification items")) {
      expect(record(item, "review-request notification item").reason).toBe("review_requested");
    }
    expect(reviewRequests.body).toMatchObject({ page: { limit: 25 } });

    const missing = await systemClient.json("/v1/notif/inbox/system-test-missing-item/read", {
      method: "POST",
      body: {},
      expectedStatus: 404,
    });
    expectError(missing.body, "not_found");
  });
});
