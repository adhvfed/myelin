import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { array, integer, record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

async function awaitActiveIssue(title: string): Promise<JsonRecord> {
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
  const authorization = record(proposed.body.authorization, "issue authorization");
  const requestEventId = string(authorization.request_event_id, "authorization request id");

  return eventually<JsonRecord>(
    async () => {
      const response = await systemClient.json(
        `/v1/issues/authorization-requests/${encodeURIComponent(requestEventId)}`,
        { expectedStatus: [200, 202] },
      );
      return response.status === 200 ? record(response.body.issue, "active issue") : undefined;
    },
    { description: `issue authorization ${requestEventId}` },
  );
}

describe("collaboration lifecycle", () => {
  test("lets a founder create and rediscover a project without operator-provided IDs", async () => {
    const name = uniqueName("Developer experience");
    const issuePrefix = `P${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`;
    const retryKey = `project-${randomUUID()}`;
    const created = await systemClient.json("/v1/projects", {
      method: "POST",
      body: { name, issue_prefix: issuePrefix },
      idempotencyKey: retryKey,
      expectedStatus: 201,
    });
    const project = record(created.body.project, "created project");
    const projectId = string(project.id, "project id");
    const projectRef = string(project.ref, "project ref");
    const defaultIssueTypeId = string(project.default_issue_type_id, "default issue type id");

    expect(created.body).toMatchObject({ created: true, durable: true });
    expect(project).toMatchObject({ name, issue_prefix: issuePrefix });
    expect(projectId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
    expect(defaultIssueTypeId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
    expect(projectRef).toBe(
      `myelin://${systemTestConfig.tenant}/identity/project/${projectId}`,
    );

    const replay = await systemClient.json("/v1/projects", {
      method: "POST",
      body: { name, issue_prefix: issuePrefix },
      idempotencyKey: retryKey,
      expectedStatus: 200,
    });
    expect(replay.body).toMatchObject({
      created: false,
      durable: true,
      project: { id: projectId, ref: projectRef },
    });

    const conflictingReplay = await systemClient.json("/v1/projects", {
      method: "POST",
      body: { name: `${name} changed`, issue_prefix: issuePrefix },
      idempotencyKey: retryKey,
      expectedStatus: 409,
    });
    expect(conflictingReplay.body).toMatchObject({ error: { code: "conflict" } });

    const rediscovered = await systemClient.json(`/v1/projects/${projectId}`);
    expect(rediscovered.body).toMatchObject({ project: { id: projectId, ref: projectRef, name } });

    const listed = await systemClient.json("/v1/projects?limit=100");
    expect(array(listed.body.items, "founder's projects")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: projectId, ref: projectRef, name })]),
    );

    const hiddenFromPeer = await reviewerClient.json(`/v1/projects/${projectId}`, {
      expectedStatus: 404,
    });
    expect(hiddenFromPeer.body).toMatchObject({ error: { code: "not_found" } });
    const peerProjects = await reviewerClient.json("/v1/projects?limit=100");
    expect(array(peerProjects.body.items, "peer's projects")).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ id: projectId })]),
    );
  });

  test("creates a public conversation and exchanges durable messages", async () => {
    const channel = uniqueName("system-chat");
    const topic = "Coordinate the externally tested release";
    const conversationRetryKey = `conversation-${randomUUID()}`;
    const created = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { channel, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 201,
    });
    const conversation = record(created.body.conversation, "created conversation");
    const conversationId = string(conversation.id, "conversation id");
    expect(created.body).toMatchObject({ durable: true });
    expect(conversation).toMatchObject({
      channel,
      topic,
    });

    const retriedCreate = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { channel, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 200,
    });
    expect(retriedCreate.body).toMatchObject({
      durable: true,
      conversation: { id: conversationId, channel, topic },
    });

    const conflictingRetry = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { channel: `${channel}-different`, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 409,
    });
    expect(conflictingRetry.body).toMatchObject({ error: { code: "conflict" } });

    const listed = await reviewerClient.json("/v1/chat/conversations?limit=100");
    expect(array(listed.body.items, "conversation list items")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: conversationId, channel })]),
    );

    const firstRetryKey = `author-${randomUUID()}`;
    const first = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "The lifecycle suite is running against the real backend." },
        idempotencyKey: firstRetryKey,
        expectedStatus: 201,
      },
    );
    expect(first.body).toMatchObject({ durable: true });
    const firstMessageId = string(first.body.message_id, "first message id");
    expect(firstMessageId).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);

    const replay = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "The lifecycle suite is running against the real backend." },
        idempotencyKey: firstRetryKey,
        expectedStatus: 201,
      },
    );
    expect(replay.body).toEqual(first.body);

    const reviewerMessage = await reviewerClient.json(
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
    const reviewerMessageId = string(reviewerMessage.body.message_id, "reviewer message id");

    const finalMessage = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: {
          content: "The paged history is consistent.",
          client_nonce: `author-final-${randomUUID()}`,
        },
        expectedStatus: 201,
      },
    );
    const finalMessageId = string(finalMessage.body.message_id, "final message id");

    const recent = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=2`,
    );
    const recentItems = array(recent.body.items, "recent conversation messages").map((item) =>
      record(item, "recent conversation message"),
    );
    expect(recentItems.map((item) => item.id)).toEqual([reviewerMessageId, finalMessageId]);
    const recentPage = record(recent.body.page, "recent message page");
    expect(recentPage).toMatchObject({ limit: 2, next_cursor: reviewerMessageId });

    const older = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=2&before=${encodeURIComponent(reviewerMessageId)}`,
    );
    expect(array(older.body.items, "older conversation messages")).toEqual([
      expect.objectContaining({ id: firstMessageId }),
    ]);
    expect(older.body).toMatchObject({ page: { limit: 2, next_cursor: null } });

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
        expect.objectContaining({
          content: "The paged history is consistent.",
          is_you: true,
          state: "active",
        }),
      ]),
    );
  });

  test("creates, edits, and conflict-checks a durable knowledge page", async () => {
    const title = uniqueName("System-tested product spec");
    const retryKey = `knowledge-${randomUUID()}`;
    const createBody = {
      title,
      template: "product-spec",
      visibility: "team",
    };
    const created = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: createBody,
      idempotencyKey: retryKey,
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
      idempotencyKey: retryKey,
      expectedStatus: 200,
    });
    expect(replay.body).toMatchObject({ created: false, durable: true, page: { id: pageId } });

    const reviewerView = await reviewerClient.json(
      `/v1/knowledge/pages/${encodeURIComponent(pageId)}`,
    );
    expect(reviewerView.body).toMatchObject({
      page: { id: pageId, title, visibility: "team", can_edit: false },
    });

    const reviewerSave = await reviewerClient.json(
      `/v1/knowledge/pages/${encodeURIComponent(pageId)}`,
      {
        method: "PUT",
        body: {
          expected_version: initialVersion,
          title: "A reviewer must not overwrite this page",
          visibility: "team",
          blocks: [{ type: "paragraph", markdown: "Unauthorized replacement." }],
        },
        expectedStatus: 404,
      },
    );
    expect(reviewerSave.body).toMatchObject({ error: { code: "not_found" } });

    const unchanged = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`);
    expect(unchanged.body).toMatchObject({
      page: { id: pageId, title, version: initialVersion, can_edit: true },
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

  test("hands an issue into a shared conversation by its canonical reference", async () => {
    const issue = await awaitActiveIssue(uniqueName("Coordinate the referenced rollout"));
    const issueRef = string(issue.ref, "issue reference");
    expect(issueRef).toMatch(/^myelin:\/\/[^/]+\/issue\/issue\/MYL-\d+$/);

    const created = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: {
        channel: uniqueName("system-chat-ref"),
        topic: "Share work without copying it",
      },
      idempotencyKey: `conversation-ref-${randomUUID()}`,
      expectedStatus: 201,
    });
    const conversation = record(created.body.conversation, "reference conversation");
    const conversationId = string(conversation.id, "reference conversation id");

    await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: {
          content: "Follow progress in \uFFFC.",
          references: [issueRef],
        },
        idempotencyKey: `message-ref-${randomUUID()}`,
        expectedStatus: 201,
      },
    );

    const history = await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=10`,
    );
    expect(array(history.body.items, "referenced conversation messages")).toEqual([
      expect.objectContaining({
        content: "Follow progress in \uFFFC.",
        nodes: [{ kind: "artifact_ref", ref: issueRef }],
      }),
    ]);
  });
});
