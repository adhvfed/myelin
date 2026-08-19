// Chat journeys: opening a topic in a project, posting into it, and reading
// it back - the three moves every chat test starts from.
import { randomUUID } from "node:crypto";

import type { SystemTestClient } from "../client.js";
import { array, record, string, type JsonRecord } from "../json.js";

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
    options: { references?: string[]; idempotencyKey?: string } = {},
  ): Promise<string> {
    const posted = await client.json(
      `/v1/chat/conversations/${encodeURIComponent(this.id)}/messages`,
      {
        method: "POST",
        body: {
          content,
          ...(options.references === undefined ? {} : { references: options.references }),
        },
        idempotencyKey: options.idempotencyKey ?? `message-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    return string(posted.body.message_id, "posted message id");
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
