import { describe, expect, it } from "vitest";

import { ChatFixtures } from "../../dev-edge/chat-contract.mjs";

const FIRST_PROJECT = "11111111-1111-1111-1111-111111111111";
const SECOND_PROJECT = "22222222-2222-2222-2222-222222222222";

describe("development Chat contract", () => {
  it("keeps a room's canonical reference and project on every response", () => {
    const chat = new ChatFixtures();
    chat.reset({ empty: true });

    const created = chat.createConversation({
      project_id: FIRST_PROJECT,
      channel: "engineering",
      topic: "release readiness",
    }, "create-1");
    if (!created.json) throw new Error("expected a created conversation");

    expect(created).toMatchObject({
      status: 201,
      json: {
        durable: true,
        conversation: {
          project_id: FIRST_PROJECT,
          channel: "engineering",
          topic: "release readiness",
        },
      },
    });
    expect(created.json.conversation.ref).toBe(
      `myelin://acme/chat/channel/${created.json.conversation.id}`,
    );
    expect(chat.listConversations({ cursor: undefined, limit: 50 }).items).toEqual([
      created.json.conversation,
    ]);

    const reference = "myelin://acme/issue/issue/MYL-7";
    expect(chat.postMessage(created.json.conversation.id, {
      content: "Track \uFFFC.",
      references: [reference],
    }, "message-1").status).toBe(201);
    expect(chat.listMessages(created.json.conversation.id, { before: undefined, limit: 50 })
      ?.items[0]).toMatchObject({
      content: "Track \uFFFC.",
      nodes: [{
        kind: "artifact_ref",
        ref: reference,
        card: { kind: "projection", title: "Issue MYL-7", state: "open" },
      }],
    });
  });

  it("keeps message retry identity in the standard operation header", () => {
    const chat = new ChatFixtures();
    chat.reset({ empty: true });
    const created = chat.createConversation({
      project_id: FIRST_PROJECT,
      channel: "reliability",
      topic: "lost acknowledgements",
    }, "create-1");
    if (!created.json) throw new Error("expected a created conversation");
    const conversationId = created.json.conversation.id;
    const message = { content: "Commit this once." };

    const first = chat.postMessage(conversationId, message, "message-1");
    expect(chat.postMessage(conversationId, message, "message-1")).toEqual(first);
    expect(chat.listMessages(conversationId, { before: undefined, limit: 50 })?.items)
      .toHaveLength(1);
    expect(chat.postMessage(conversationId, message, undefined).status).toBe(400);
    expect(chat.postMessage(
      conversationId,
      { ...message, client_nonce: "legacy-body-token" },
      "message-2",
    ).status).toBe(400);
  });

  it("scopes topic retry identities to one exact draft", () => {
    const chat = new ChatFixtures();
    chat.reset({ empty: true });
    const room = (project_id: string) => ({
      project_id,
      channel: "engineering",
      topic: "release readiness",
    });

    expect(chat.createConversation(room(FIRST_PROJECT), "create-1").status).toBe(201);
    expect(chat.createConversation(room(FIRST_PROJECT), "create-1").status).toBe(200);
    expect(chat.createConversation(room(FIRST_PROJECT), "create-2").status).toBe(200);
    expect(chat.createConversation(room(SECOND_PROJECT), "create-1").status).toBe(409);
    expect(chat.createConversation(room(SECOND_PROJECT), "create-3").status).toBe(201);
    expect(chat.createConversation(
      { channel: "engineering", topic: "release readiness" },
      "create-4",
    ).status).toBe(400);
  });
});
