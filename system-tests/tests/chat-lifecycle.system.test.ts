import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { Conversation } from "../src/journeys/chat.js";
import { findInboxItem } from "../src/journeys/inbox.js";
import { awaitActiveIssue } from "../src/journeys/issues.js";
import { awaitBacklink } from "../src/journeys/refs.js";
import { array, record, string } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

describe("chat collaboration lifecycle", () => {
  test("turns a teammate's words into one durable nudge without leaking private rooms", async () => {
    const privateProject = await systemClient.json("/v1/projects", {
      method: "POST",
      body: {
        name: uniqueName("Private mention project"),
        issue_prefix: `M${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`,
      },
      idempotencyKey: `private-mention-project-${randomUUID()}`,
      expectedStatus: 201,
    });
    const privateProjectId = string(
      record(privateProject.body.project, "private mention project").id,
      "private mention project id",
    );
    const privateRoom = await Conversation.open(systemClient, {
      projectId: privateProjectId,
      channel: uniqueName("private-mentions"),
      topic: "Keep the room and its notifications private",
    });
    const privateProbe = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(privateRoom.id)}/messages`,
      {
        method: "POST",
        body: {
          content: "\uFFFC should never learn that this room exists.",
          nodes: [{ kind: "mention", principal_id: systemTestConfig.reviewerPrincipal }],
        },
        idempotencyKey: `private-mention-${randomUUID()}`,
        expectedStatus: 400,
      },
    );
    expect(privateProbe.body).toMatchObject({ error: { code: "bad_request" } });
    expect(await privateRoom.messages(systemClient)).toEqual([]);

    const room = await Conversation.open(systemClient, {
      projectId: systemTestConfig.issues.projectId,
      channel: uniqueName("release-mentions"),
      topic: "Ask a teammate without configuring Slack",
    });
    const message = "\uFFFC please review the release while the context is fresh.";
    const retryKey = `chat-mention-${randomUUID()}`;
    const firstMessageId = await room.post(reviewerClient, message, {
      nodes: [{ kind: "mention", principal_id: systemTestConfig.principal }],
      idempotencyKey: retryKey,
    });
    const replayedMessageId = await room.post(reviewerClient, message, {
      nodes: [{ kind: "mention", principal_id: systemTestConfig.principal }],
      idempotencyKey: retryKey,
    });
    expect(replayedMessageId).toBe(firstMessageId);

    const [storedMention] = (await room.messages(systemClient)).filter(
      (item) => item.id === firstMessageId,
    );
    expect(storedMention).toMatchObject({
      content: message,
      is_you: false,
      nodes: [{ kind: "mention", principal_id: systemTestConfig.principal }],
    });

    const messageRef =
      `myelin://${systemTestConfig.tenant}/chat/message/${firstMessageId}` +
      `#message-${firstMessageId}`;
    const notification = await eventually(
      () => findInboxItem(systemClient, messageRef),
      { description: "the real Chat mention to reach the mentioned teammate's durable inbox" },
    );
    expect(notification).toMatchObject({
      subject: messageRef,
      subsystem: "chat",
      reason: "mentioned",
      class: "direct",
      coalesce_count: 1,
      state: "unread",
    });
    expect(await findInboxItem(reviewerClient, messageRef)).toBeUndefined();
  });

  test("lets project collaborators talk while private project rooms stay private", async () => {
    const channel = uniqueName("system-chat");
    const topic = "Coordinate the externally tested release";
    const projectId = systemTestConfig.issues.projectId;
    const conversationRetryKey = `conversation-${randomUUID()}`;
    const created = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { project_id: projectId, channel, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 201,
    });
    const conversation = record(created.body.conversation, "created conversation");
    const conversationId = string(conversation.id, "conversation id");
    expect(created.body).toMatchObject({ durable: true });
    expect(conversation).toMatchObject({
      ref: `myelin://${systemTestConfig.tenant}/chat/channel/${conversationId}`,
      project_id: projectId,
      channel,
      topic,
    });

    const retriedCreate = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { project_id: projectId, channel, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 200,
    });
    expect(retriedCreate.body).toMatchObject({
      durable: true,
      conversation: { id: conversationId, project_id: projectId, channel, topic },
    });

    const conflictingRetry = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { project_id: projectId, channel: `${channel}-different`, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 409,
    });
    expect(conflictingRetry.body).toMatchObject({ error: { code: "conflict" } });

    const listed = await reviewerClient.json("/v1/chat/conversations?limit=100");
    expect(array(listed.body.items, "conversation list items")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: conversationId, channel })]),
    );

    const privateProject = await systemClient.json("/v1/projects", {
      method: "POST",
      body: {
        name: uniqueName("Private project room"),
        issue_prefix: `C${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`,
      },
      idempotencyKey: `private-chat-project-${randomUUID()}`,
      expectedStatus: 201,
    });
    const privateProjectId = string(
      record(privateProject.body.project, "private Chat project").id,
      "private Chat project id",
    );
    const privateRoom = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: {
        project_id: privateProjectId,
        channel,
        topic,
      },
      idempotencyKey: `private-chat-room-${randomUUID()}`,
      expectedStatus: 201,
    });
    const privateRoomId = string(
      record(privateRoom.body.conversation, "private project conversation").id,
      "private project conversation id",
    );
    const foundersRooms = await systemClient.json("/v1/chat/conversations?limit=100");
    expect(array(foundersRooms.body.items, "founder's project conversations")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: conversationId, project_id: projectId, channel, topic }),
        expect.objectContaining({ id: privateRoomId, project_id: privateProjectId, channel, topic }),
      ]),
    );
    const peerRooms = await reviewerClient.json("/v1/chat/conversations?limit=100");
    expect(array(peerRooms.body.items, "peer's project conversations")).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ id: privateRoomId })]),
    );
    const absentPrivateHistory = await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(privateRoomId)}/messages?limit=10`,
      { expectedStatus: 404 },
    );
    expect(absentPrivateHistory.body).toMatchObject({ error: { code: "not_found" } });
    const absentPrivatePost = await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(privateRoomId)}/messages`,
      {
        method: "POST",
        body: { content: "I should not be able to discover this room." },
        idempotencyKey: `private-chat-probe-${randomUUID()}`,
        expectedStatus: 404,
      },
    );
    expect(absentPrivatePost.body).toMatchObject({ error: { code: "not_found" } });

    const sharedWork = await awaitActiveIssue(systemClient, uniqueName("Shared work with private context"));
    const sharedWorkRef = string(sharedWork.ref, "shared work reference");
    const privateContext = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(privateRoomId)}/messages`,
      {
        method: "POST",
        body: {
          content: "The private room may discuss shared work without revealing itself: ￼",
          references: [sharedWorkRef],
        },
        idempotencyKey: `private-chat-context-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    const privateMessageRef = `myelin://${systemTestConfig.tenant}/chat/message/${string(
      privateContext.body.message_id,
      "private context message id",
    )}`;
    await awaitBacklink(systemClient, sharedWorkRef, privateMessageRef, "links");

    const peerBacklinks = await reviewerClient.json(
      `/v1/refs/backlinks?ref=${encodeURIComponent(sharedWorkRef)}`,
    );
    expect(
      array(peerBacklinks.body.items, "peer-visible backlinks")
        .map((item) => record(item, "peer-visible backlink"))
        .some((item) => item.root_ref === privateMessageRef),
    ).toBe(false);

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
        body: { content: "Confirmed from a second principal." },
        idempotencyKey: `reviewer-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    const reviewerMessageId = string(reviewerMessage.body.message_id, "reviewer message id");

    const finalMessage = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "The paged history is consistent." },
        idempotencyKey: `author-final-${randomUUID()}`,
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

  test("hands an issue into a shared conversation by its canonical reference", async () => {
    const issue = await awaitActiveIssue(systemClient, uniqueName("Coordinate the referenced rollout"));
    const issueRef = string(issue.ref, "issue reference");
    expect(issueRef).toMatch(/^myelin:\/\/[^/]+\/issue\/issue\/MYL-\d+$/);

    const created = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: {
        project_id: systemTestConfig.issues.projectId,
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
