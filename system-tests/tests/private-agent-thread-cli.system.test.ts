import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { randomUUID } from "node:crypto";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import { browserApprovedCliCredential, uniqueName } from "../src/context.js";
import { activateExternalAgent } from "../src/journeys/agents.js";
import { record, string } from "../src/json.js";
import { runCliWith, type CliResult } from "../src/myelin-cli.js";

describe("private agent work from the CLI", () => {
  let configDirectory: string;

  beforeAll(async () => {
    configDirectory = await mkdtemp(resolve(tmpdir(), "myelin-private-thread-cli-"));
  });

  afterAll(async () => {
    await rm(configDirectory, { recursive: true, force: true });
  });

  test("a developer names the problem, returns to its history, and enters its workspace", async () => {
    const session = await browserApprovedCliCredential();
    const companion = await activateExternalAgent(
      session.client,
      uniqueName("CLI checkout companion"),
      ["chat.read_messages", "chat.post", "workspace.read_file", "workspace.write_file"],
    );
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

    const threadName = uniqueName("Investigate checkout final-reader race");
    const started = await runMyelin(
      "--json",
      "--idempotency-key",
      `cli-private-thread-${randomUUID()}`,
      "agent",
      "thread",
      "start",
      threadName,
      "--agent",
      companion.agent.id,
      "--retention-days",
      "3",
    );
    expect(started.exitCode, started.stderr).toBe(0);
    const thread = record(
      record(JSON.parse(started.stdout), "CLI thread creation").thread,
      "created private thread",
    );
    const threadId = string(thread.id, "created private thread id");
    expect(thread).toMatchObject({
      name: threadName,
      agent_id: companion.agent.id,
      workspace: { state: "ready", retention_days: 3 },
    });

    const shown = await runMyelin("agent", "thread", "show", threadId);
    expect(shown.exitCode, shown.stderr).toBe(0);
    expect(shown.stdout).toContain(`Private agent thread: ${threadName}`);
    expect(shown.stdout).toContain(`myelin agent thread say ${threadId}`);
    expect(shown.stdout).toContain(`myelin agent thread ssh ${threadId}`);

    const listed = await runMyelin("agent", "thread", "list", "--limit", "100");
    expect(listed.exitCode, listed.stderr).toBe(0);
    expect(listed.stdout).toContain(threadName);
    expect(listed.stdout).toContain(string(thread.ref, "created private thread ref"));

    const problem = "Please preserve the final reader before checkout cleanup.";
    const said = await runMyelin(
      "--idempotency-key",
      `cli-private-message-${randomUUID()}`,
      "agent",
      "thread",
      "say",
      threadId,
      problem,
    );
    expect(said.exitCode, said.stderr).toBe(0);
    expect(said.stdout).toMatch(/^sent \([0-9A-HJKMNP-TV-Z]{26}\)\n$/);

    const history = await runMyelin(
      "agent",
      "thread",
      "history",
      threadId,
      "--limit",
      "10",
    );
    expect(history.exitCode, history.stderr).toBe(0);
    expect(history.stdout).toContain(problem);

    const observation = uniqueName("The CLI was here");
    const wroteWorkspace = await runMyelin(
      "agent",
      "thread",
      "ssh",
      threadId,
      "--command",
      `mkdir -p notes && printf %s '${observation}' > notes/from-cli.txt`,
    );
    expect(wroteWorkspace.exitCode, wroteWorkspace.stderr).toBe(0);

    const resumedWorkspace = await runMyelin(
      "agent",
      "thread",
      "ssh",
      threadId,
      "--command",
      "cat notes/from-cli.txt",
    );
    expect(resumedWorkspace.exitCode, resumedWorkspace.stderr).toBe(0);
    expect(resumedWorkspace.stdout).toBe(observation);

    for (const result of [started, shown, listed, said, history, wroteWorkspace, resumedWorkspace]) {
      expect(result.stdout).not.toContain(session.token);
      expect(result.stderr).not.toContain(session.token);
      expect(`${result.stdout}${result.stderr}`).not.toContain("id_ed25519");
    }
  }, 120_000);
});
