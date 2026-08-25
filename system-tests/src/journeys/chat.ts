// Chat journeys: opening a topic in a project, posting into it, and reading
// it back - the three moves every chat test starts from.
import { randomUUID } from "node:crypto";

import type { SystemTestClient } from "../client.js";
import { array, boolean, record, string, type JsonRecord } from "../json.js";

export type MessageNode =
  | { kind: "mention"; principal_id: string }
  | { kind: "artifact_ref"; ref: string };

export class Conversation {
  constructor(
    readonly id: string,
    readonly created: JsonRecord,
  ) {}

  static async open(
    client: SystemTestClient,
    options: { projectId: string; channel: string; topic: string; idempotencyKey?: string },
  ): Promise<Conversation> {
    const created = await client.json("/v1/chat/conversations", {
      method: "POST",
      body: { project_id: options.projectId, channel: options.channel, topic: options.topic },
      idempotencyKey: options.idempotencyKey ?? `conversation-${randomUUID()}`,
      expectedStatus: 201,
    });
    const conversation = record(created.body.conversation, "created conversation");
    return new Conversation(string(conversation.id, "conversation id"), conversation);
  }

  async post(
    client: SystemTestClient,
    content: string,
    options: { references?: string[]; nodes?: MessageNode[]; idempotencyKey?: string } = {},
  ): Promise<string> {
    const posted = await client.json(
      `/v1/chat/conversations/${encodeURIComponent(this.id)}/messages`,
      {
        method: "POST",
        body: {
          content,
          ...(options.references === undefined ? {} : { references: options.references }),
          ...(options.nodes === undefined ? {} : { nodes: options.nodes }),
        },
        idempotencyKey: options.idempotencyKey ?? `message-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    return string(posted.body.message_id, "posted message id");
  }

  async reply(
    client: SystemTestClient,
    rootMessageId: string,
    content: string,
    options: { references?: string[]; nodes?: MessageNode[]; idempotencyKey?: string } = {},
  ): Promise<string> {
    const posted = await client.json(
      `/v1/chat/messages/${encodeURIComponent(rootMessageId)}/replies`,
      {
        method: "POST",
        body: {
          content,
          ...(options.references === undefined ? {} : { references: options.references }),
          ...(options.nodes === undefined ? {} : { nodes: options.nodes }),
        },
        idempotencyKey: options.idempotencyKey ?? `chat-reply-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    return string(posted.body.message_id, "posted reply id");
  }

  async thread(
    client: SystemTestClient,
    rootMessageId: string,
    limit = 100,
  ): Promise<{ ref: string; following: boolean; root: JsonRecord; replies: JsonRecord[] }> {
    const response = await client.json(
      `/v1/chat/threads/${encodeURIComponent(rootMessageId)}/messages?limit=${limit}`,
    );
    const conversation = record(response.body.conversation, "thread conversation");
    if (string(conversation.id, "thread conversation id") !== this.id) {
      throw new Error("thread resolved outside its conversation");
    }
    return {
      ref: string(response.body.ref, "thread reference"),
      following: boolean(response.body.following, "thread following state"),
      root: record(response.body.root, "thread root message"),
      replies: array(response.body.items, "thread replies")
        .map((item) => record(item, "thread reply")),
    };
  }

  async followThread(client: SystemTestClient, rootMessageId: string): Promise<boolean> {
    const response = await client.json(
      `/v1/chat/threads/${encodeURIComponent(rootMessageId)}/follow`,
      { method: "PUT", body: {}, idempotencyKey: false },
    );
    return boolean(response.body.following, "followed thread state");
  }

  async muteThread(client: SystemTestClient, rootMessageId: string): Promise<boolean> {
    const response = await client.json(
      `/v1/chat/threads/${encodeURIComponent(rootMessageId)}/follow`,
      { method: "DELETE", idempotencyKey: false },
    );
    return boolean(response.body.following, "muted thread state");
  }

  async messages(client: SystemTestClient, limit = 100): Promise<JsonRecord[]> {
    const response = await client.json(
      `/v1/chat/conversations/${encodeURIComponent(this.id)}/messages?limit=${limit}`,
    );
    return array(response.body.items, "conversation messages")
      .map((item) => record(item, "conversation message"));
  }

  /// The live-delivery stream for this conversation (same visibility gate as
  /// reads). Callers own closing the returned stream.
  async events(client: SystemTestClient) {
    return client.eventStream(
      `/v1/chat/conversations/${encodeURIComponent(this.id)}/events`,
    );
  }
}
