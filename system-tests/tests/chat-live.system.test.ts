import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { record, string } from "../src/json.js";

async function createConversation(projectId: string, channel: string, topic: string) {
  const created = await systemClient.json("/v1/chat/conversations", {
    method: "POST",
    body: { project_id: projectId, channel, topic },
    idempotencyKey: `chat-live-conversation-${randomUUID()}`,
    expectedStatus: 201,
  });
  return string(record(created.body.conversation, "created conversation").id, "conversation id");
}

describe("chat live delivery", () => {
  test("a collaborator's live subscription receives a posted message as a reference frame", async () => {
    const conversationId = await createConversation(
      systemTestConfig.issues.projectId,
      uniqueName("system-chat-live"),
      "Prove chat delivery is push, not polling",
    );

    const subscription = await reviewerClient.eventStream(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/events`,
    );
    expect(subscription.headers.get("content-type")).toContain("text/event-stream");
    try {
      const content = `The launch window opens at dawn - ${randomUUID()}`;
      const arrival = subscription.stream.waitFor(
        (event) => event.event === "chat.message.posted",
        { description: `chat.message.posted in ${conversationId}` },
      );
      const posted = await systemClient.json(
        `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
        {
          method: "POST",
          body: { content },
          idempotencyKey: `chat-live-post-${randomUUID()}`,
          expectedStatus: 201,
        },
      );
      const messageId = string(posted.body.message_id, "posted message id");

      const frame = await arrival;
      expect(JSON.parse(frame.data)).toMatchObject({
        type: "chat.message.posted",
        conversation: conversationId,
        message_id: messageId,
      });
      // references, not payloads: the stream must never carry message content
      expect(frame.data).not.toContain(content);
    } finally {
      subscription.stream.close();
    }
  });

  test("live subscriptions are refused exactly where reads are refused", async () => {
    const unauthenticated = await systemClient.json(
      "/v1/chat/conversations/01J0CONV000000000000000000/events",
      { authenticated: false, expectedStatus: 401 },
    );
    expect(unauthenticated.body).toHaveProperty("error.code", "unauthorized");

    const malformed = await systemClient.json("/v1/chat/conversations/not-a-ulid/events", {
      expectedStatus: 400,
    });
    expect(malformed.body).toHaveProperty("error.code", "bad_request");

    // a conversation in a project the peer cannot see must be undiscoverable
    // through the live stream, exactly as it is through reads
    const privateProject = await systemClient.json("/v1/projects", {
      method: "POST",
      body: {
        name: uniqueName("Private live room"),
        issue_prefix: `L${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`,
      },
      idempotencyKey: `chat-live-private-project-${randomUUID()}`,
      expectedStatus: 201,
    });
    const privateProjectId = string(
      record(privateProject.body.project, "private project").id,
      "private project id",
    );
    const privateRoomId = await createConversation(
      privateProjectId,
      uniqueName("system-chat-live-private"),
      "No peer may observe activity here",
    );
    const founderCanSubscribe = await systemClient.eventStream(
      `/v1/chat/conversations/${encodeURIComponent(privateRoomId)}/events`,
    );
    founderCanSubscribe.stream.close();
    const hidden = await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(privateRoomId)}/events`,
      { expectedStatus: 404 },
    );
    expect(hidden.body).toMatchObject({ error: { code: "not_found" } });
  });
});
