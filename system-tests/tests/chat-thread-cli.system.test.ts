import { randomUUID } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import { browserApprovedCliCredential, uniqueName } from "../src/context.js";
import { array, record, string } from "../src/json.js";
import { runCliWith, type CliResult } from "../src/myelin-cli.js";

describe("focused Chat replies from the CLI", () => {
  let configDirectory: string;

  beforeAll(async () => {
    configDirectory = await mkdtemp(resolve(tmpdir(), "myelin-chat-thread-cli-"));
  });

  afterAll(async () => {
    await rm(configDirectory, { recursive: true, force: true });
  });

  test("a developer replies beside the room and returns to the whole decision", async () => {
    const session = await browserApprovedCliCredential();
    const runMyelin = (...args: string[]): Promise<CliResult> => runCliWith(
      configDirectory,
      {
        environment: {
          MYELIN_EDGE: systemTestConfig.edgeUrl,
          MYELIN_TOKEN: session.token,
          MYELIN_TOKEN_SCHEME: session.tokenScheme,
        },
      },
      args,
    );
    const topic = uniqueName("Keep the release decision focused");
    const created = await runMyelin(
      "--json",
      "--idempotency-key",
      `cli-chat-thread-room-${randomUUID()}`,
      "chat",
      "create",
      uniqueName("release-decisions"),
      "--topic",
      topic,
      "--project",
      systemTestConfig.issues.projectId,
    );
    expect(created.exitCode, created.stderr).toBe(0);
    const conversation = record(
      record(JSON.parse(created.stdout), "CLI conversation receipt").conversation,
      "CLI conversation",
    );
    const conversationId = string(conversation.id, "CLI conversation id");

    const rootWords = "Should the release wait for the restore proof?";
    const sent = await runMyelin(
      "--idempotency-key",
      `cli-chat-root-${randomUUID()}`,
      "chat",
      "send",
      conversationId,
      rootWords,
    );
    expect(sent.exitCode, sent.stderr).toBe(0);
    const rootMessageId = /^sent \(([0-9A-HJKMNP-TV-Z]{26})\)\n$/.exec(sent.stdout)?.[1];
    expect(rootMessageId).toBeDefined();

    const replyWords = "Yes. Keep the reader invariant as the release gate.";
    const replied = await runMyelin(
      "--idempotency-key",
      `cli-chat-reply-${randomUUID()}`,
      "chat",
      "reply",
      rootMessageId!,
      replyWords,
    );
    expect(replied.exitCode, replied.stderr).toBe(0);

    const room = await runMyelin("--json", "chat", "history", conversationId, "--limit", "10");
    expect(room.exitCode, room.stderr).toBe(0);
    const roomMessages = array(
      record(JSON.parse(room.stdout), "CLI room history").items,
      "CLI room messages",
    );
    expect(roomMessages).toEqual([
      expect.objectContaining({
        id: rootMessageId,
        content: rootWords,
        thread_root_id: null,
        reply_count: 1,
      }),
    ]);
    expect(room.stdout).not.toContain(replyWords);

    const thread = await runMyelin("--json", "chat", "thread", rootMessageId!, "--limit", "10");
    expect(thread.exitCode, thread.stderr).toBe(0);
    const threadHistory = record(JSON.parse(thread.stdout), "CLI thread history");
    expect(threadHistory).toMatchObject({
      ref: `myelin://${systemTestConfig.tenant}/chat/thread/${rootMessageId}` +
        `#thread-${rootMessageId}`,
      root: { id: rootMessageId, content: rootWords, reply_count: 1 },
      items: [{ content: replyWords, thread_root_id: rootMessageId }],
    });

    const rendered = await runMyelin("chat", "thread", rootMessageId!, "--limit", "10");
    expect(rendered.exitCode, rendered.stderr).toBe(0);
    expect(rendered.stdout).toContain(topic);
    expect(rendered.stdout).toContain(rootWords);
    expect(rendered.stdout).toContain(replyWords);
    for (const result of [created, sent, replied, room, thread, rendered]) {
      expect(result.stdout).not.toContain(session.token);
      expect(result.stderr).not.toContain(session.token);
    }
  });
});
