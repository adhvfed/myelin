import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { array, integer, record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

describe("collaboration lifecycle", () => {
  test("creates a public conversation and exchanges durable messages", async () => {
    const channel = uniqueName("system-chat");
    const created = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { channel, topic: "Coordinate the externally tested release" },
      expectedStatus: 201,
    });
    const conversation = record(created.body.conversation, "created conversation");
    const conversationId = string(conversation.id, "conversation id");
    expect(created.body).toMatchObject({ durable: true });
    expect(conversation).toMatchObject({
      channel,
      topic: "Coordinate the externally tested release",
    });

    const listed = await reviewerClient.json("/v1/chat/conversations?limit=100");
    expect(array(listed.body.items, "conversation list items")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: conversationId, channel })]),
    );

    const firstNonce = `author-${randomUUID()}`;
    const first = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "The lifecycle suite is running against the real backend.", client_nonce: firstNonce },
        expectedStatus: 201,
      },
    );
    expect(first.body).toMatchObject({ durable: true });
    expect(first.body.message_id).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);

    const replay = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "The lifecycle suite is running against the real backend.", client_nonce: firstNonce },
        expectedStatus: 201,
      },
    );
    expect(replay.body).toEqual(first.body);

    await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: {
          content: "Confirmed from a second principal.",
          client_nonce: `reviewer-${randomUUID()}`,
        },
        expectedStatus: 201,
      },
    );

    const messages = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=100`,
    );
    expect(messages.body).toMatchObject({ conversation: { id: conversationId, channel } });
    expect(array(messages.body.items, "conversation messages")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          content: "The lifecycle suite is running against the real backend.",
          is_you: true,
          state: "active",
        }),
        expect.objectContaining({
          content: "Confirmed from a second principal.",
          is_you: false,
          state: "active",
        }),
      ]),
    );
  });

  test("creates, edits, and conflict-checks a durable knowledge page", async () => {
    const title = uniqueName("System-tested product spec");
    const nonce = `knowledge-${randomUUID()}`;
    const createBody = {
      title,
      template: "product-spec",
      visibility: "team",
      client_nonce: nonce,
    };
    const created = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: createBody,
      expectedStatus: 201,
    });
    const page = record(created.body.page, "created knowledge page");
    const pageId = string(page.id, "knowledge page id");
    const initialVersion = integer(page.version, "knowledge page version");
    expect(created.body).toMatchObject({ created: true, durable: true });
    expect(page).toMatchObject({ title, visibility: "team", can_edit: true });
    expect(array(page.blocks, "template blocks").length).toBeGreaterThan(1);

    const replay = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: createBody,
      expectedStatus: 200,
    });
    expect(replay.body).toMatchObject({ created: false, durable: true, page: { id: pageId } });

    const reviewerView = await reviewerClient.json(
      `/v1/knowledge/pages/${encodeURIComponent(pageId)}`,
    );
    expect(reviewerView.body).toMatchObject({
      page: { id: pageId, title, visibility: "team", can_edit: false },
    });

    const editedTitle = `${title} — approved`;
    const saved = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`, {
      method: "PUT",
      body: {
        expected_version: initialVersion,
        title: editedTitle,
        visibility: "team",
        blocks: [
          { type: "heading", markdown: "Outcome" },
          { type: "paragraph", markdown: "Every engineering workflow is observable end to end." },
          { type: "task_list", markdown: "Keep the contract suite green." },
        ],
      },
    });
    const savedPage = record(saved.body.page, "saved knowledge page");
    const savedVersion = integer(saved.body.version, "saved knowledge version");
    expect(saved.body).toMatchObject({ durable: true });
    expect(savedVersion).toBeGreaterThan(initialVersion);
    expect(savedPage).toMatchObject({ id: pageId, title: editedTitle, version: savedVersion });
    expect(array(savedPage.blocks, "saved knowledge blocks")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ type: "paragraph", markdown: expect.stringContaining("observable") }),
      ]),
    );

    const stale = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`, {
      method: "PUT",
      body: {
        expected_version: initialVersion,
        title: "Stale overwrite",
        visibility: "team",
        blocks: [{ type: "paragraph", markdown: "This must not win." }],
      },
      expectedStatus: 409,
    });
    expect(stale.body).toHaveProperty("error");

    const persisted = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`);
    expect(persisted.body).toMatchObject({ page: { title: editedTitle, version: savedVersion } });
    const pages = await systemClient.json("/v1/knowledge/pages?limit=100");
    expect(array(pages.body.items, "knowledge list items")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: pageId, title: editedTitle })]),
    );
  });

  test("moves an issue through authorization, discovery, and completion", async () => {
    const title = uniqueName("Ship the backend lifecycle suite");
    const proposed = await systemClient.json("/v1/issues", {
      method: "POST",
      body: {
        project_id: systemTestConfig.issues.projectId,
        type_id: systemTestConfig.issues.typeId,
        prefix: systemTestConfig.issues.prefix,
        title,
      },
      expectedStatus: 202,
    });
    const summary = record(proposed.body.issue, "issue proposal");
    const authorization = record(proposed.body.authorization, "issue authorization");
    const issueId = string(summary.id, "issue id");
    const requestEventId = string(authorization.request_event_id, "authorization request id");
    expect(summary.key).toMatch(/^MYL-\d+$/);
    expect(authorization.status).toBe("pending");

    const active = await eventually<JsonRecord>(
      async () => {
        const response = await systemClient.json(
          `/v1/issues/authorization-requests/${encodeURIComponent(requestEventId)}`,
          { expectedStatus: [200, 202] },
        );
        if (response.status === 202) {
          expect(response.body.status).toBe("pending");
          return undefined;
        }
        expect(response.body.status).toBe("active");
        return record(response.body.issue, "active issue");
      },
      { description: `issue authorization ${requestEventId}` },
    );
    expect(active).toMatchObject({ id: issueId, title, state_category: "unstarted" });

    const viewed = await reviewerClient.json(`/v1/issues/${encodeURIComponent(issueId)}`);
    expect(viewed.body).toMatchObject({ id: issueId, title, state_category: "unstarted" });

    const open = await systemClient.json("/v1/issues?state=open&limit=100");
    expect(array(open.body.items, "open issues")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: issueId, title })]),
    );

    const closed = await systemClient.json(`/v1/issues/${encodeURIComponent(issueId)}/close`, {
      method: "POST",
      body: {},
    });
    expect(closed.body).toMatchObject({ id: issueId, title, state_category: "completed" });

    const retry = await systemClient.json(`/v1/issues/${encodeURIComponent(issueId)}/close`, {
      method: "POST",
      body: {},
    });
    expect(retry.body).toMatchObject({ id: issueId, state_category: "completed" });

    const closedList = await systemClient.json("/v1/issues?state=closed&limit=100");
    expect(array(closedList.body.items, "closed issues")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: issueId, title })]),
    );
  });
});
