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
    });
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
  });

  it("allows a familiar room name in another project without weakening retries", () => {
    const chat = new ChatFixtures();
    chat.reset({ empty: true });
    const room = (project_id: string) => ({
      project_id,
      channel: "engineering",
      topic: "release readiness",
    });

    expect(chat.createConversation(room(FIRST_PROJECT)).status).toBe(201);
    expect(chat.createConversation(room(FIRST_PROJECT)).status).toBe(409);
    expect(chat.createConversation(room(SECOND_PROJECT)).status).toBe(201);
    expect(chat.createConversation({ channel: "engineering", topic: "release readiness" }).status)
      .toBe(400);
  });
});
