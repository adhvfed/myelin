import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { randomUUID } from "node:crypto";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import { systemClient, uniqueName } from "../src/context.js";
import { record, string } from "../src/json.js";
import { finish, runCli, runCliWith, startCli, waitForCode } from "../src/myelin-cli.js";

describe("private agent work from the CLI", () => {
  let configDirectory: string;
  let login: ChildProcessWithoutNullStreams | undefined;

  beforeAll(async () => {
    configDirectory = await mkdtemp(resolve(tmpdir(), "myelin-private-thread-cli-"));
  });

  afterAll(async () => {
    if (login && login.exitCode === null) login.kill("SIGTERM");
    await rm(configDirectory, { recursive: true, force: true });
  });

  test("a developer names the problem and resumes it with the agent and SSH", async () => {
    login = startCli(
      configDirectory,
      "--edge",
      systemTestConfig.edgeUrl,
      "auth",
      "login",
      "--no-browser",
    );
    const approval = await waitForCode(login);
    await systemClient.json("/v1/auth/device/approval", {
      method: "POST",
      body: { user_code: approval.code },
    });
    expect(await finish(login, approval)).toContain("Approved. Your CLI session is ready");

    const runMyelin = (...args: string[]) => runCli(configDirectory, ...args);
    const initialAgentDefaults = "# CLI companion defaults\n\nStart from the durable workspace.";
    const activated = await runMyelin(
      "--json",
      "--idempotency-key",
      `cli-private-agent-${randomUUID()}`,
      "agent",
      "create",
      uniqueName("CLI checkout companion"),
      "--tool",
      "chat.read_messages",
      "--tool",
      "chat.post",
      "--tool",
      "workspace.read_file",
      "--tool",
      "workspace.write_file",
      "--workspace-instructions",
      initialAgentDefaults,
    );
    expect(activated.exitCode, activated.stderr).toBe(0);
    const companion = record(
      record(JSON.parse(activated.stdout), "CLI agent activation").agent,
      "activated private-work agent",
    );
    const companionId = string(companion.id, "activated private-work agent id");
    expect(companion).toMatchObject({
      workspace_profile: { revision: 1, instructions: { path: "AGENTS.md" } },
    });
    const revisedAgentDefaults = `${initialAgentDefaults}\nExplain the retry boundary.`;
    const configured = await runMyelin(
      "--json",
      "--idempotency-key",
      `cli-agent-config-${randomUUID()}`,
      "agent",
      "configure",
      companionId,
      "--expected-revision",
      "1",
      "--workspace-instructions",
      revisedAgentDefaults,
    );
    expect(configured.exitCode, configured.stderr).toBe(0);
    expect(JSON.parse(configured.stdout)).toMatchObject({
      changed: true,
      durable: true,
      agent: { id: companionId, workspace_profile: { revision: 2 } },
    });

    const threadName = uniqueName("Investigate checkout final-reader race");
    const personalInstructions = "# CLI owner workflow\n\nPreserve final readers before cleanup.";
    const started = await runMyelin(
      "--json",
      "--idempotency-key",
      `cli-private-thread-${randomUUID()}`,
      "agent",
      "thread",
      "start",
      threadName,
      "--agent",
      companionId,
      "--retention-days",
      "3",
      "--instructions",
      personalInstructions,
    );
    expect(started.exitCode, started.stderr).toBe(0);
    const thread = record(
      record(JSON.parse(started.stdout), "CLI thread creation").thread,
      "created private thread",
    );
    const threadId = string(thread.id, "created private thread id");
    expect(thread).toMatchObject({
      name: threadName,
      agent_id: companionId,
      workspace: {
        state: "ready",
        retention_days: 3,
        personalization: {
          path: "AGENTS.md",
          strategy: "append",
          content_digest: expect.stringMatching(/^blake3:[0-9a-f]{64}$/),
        },
      },
    });

    const shown = await runMyelin("agent", "thread", "show", threadId);
    expect(shown.exitCode, shown.stderr).toBe(0);
    expect(shown.stdout).toContain(`Private agent thread: ${threadName}`);
    expect(shown.stdout).toContain(`myelin agent thread say ${threadId}`);
    expect(shown.stdout).toContain(`myelin agent thread ssh ${threadId}`);
    expect(shown.stdout).toContain(
      `myelin mcp serve --as ${companionId} --thread ${threadId}`,
    );

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

    const agentObservation = uniqueName("The thread-bound MCP agent was here");
    const agentPath = "notes/from-thread-agent.txt";
    const bridge = await runCliWith(
      configDirectory,
      {
        input: [
          JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} }),
          JSON.stringify({
            jsonrpc: "2.0",
            id: 2,
            method: "tools/call",
            params: {
              name: "workspace.read_file",
              arguments: { path: "AGENTS.md" },
            },
          }),
          JSON.stringify({
            jsonrpc: "2.0",
            id: 3,
            method: "tools/call",
            params: {
              name: "workspace.write_file",
              arguments: { path: agentPath, content: agentObservation },
              _meta: { "com.myelin/idempotencyKey": `cli-thread-write-${randomUUID()}` },
            },
          }),
          "",
        ].join("\n"),
      },
      ["mcp", "serve", "--as", companionId, "--thread", threadId],
    );
    expect(bridge.exitCode, bridge.stderr).toBe(0);
    expect(bridge.stderr).toBe("");
    const bridgeResponses = bridge.stdout.trim().split("\n").map((line) => JSON.parse(line));
    expect(bridgeResponses).toHaveLength(3);
    expect(JSON.stringify(bridgeResponses[1])).toContain("# CLI owner workflow");
    expect(JSON.stringify(bridgeResponses[1])).toContain("Preserve final readers before cleanup.");
    expect(JSON.stringify(bridgeResponses[1])).toContain("# CLI companion defaults");
    expect(JSON.stringify(bridgeResponses[1])).toContain("Explain the retry boundary.");
    expect(record(bridgeResponses[2]?.result, "thread-bound MCP write result"))
      .toMatchObject({ isError: false, _meta: { tool: "workspace.write_file" } });

    const agentWorkspace = await runMyelin(
      "agent",
      "thread",
      "ssh",
      threadId,
      "--command",
      `cat ${agentPath}`,
    );
    expect(agentWorkspace.exitCode, agentWorkspace.stderr).toBe(0);
    expect(agentWorkspace.stdout).toBe(agentObservation);

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

    const uploadPath = resolve(configDirectory, "editor-upload.txt");
    const downloadPath = resolve(configDirectory, "editor-download.txt");
    const editorObservation = uniqueName("The standard CLI SFTP client was here");
    await writeFile(uploadPath, editorObservation, { encoding: "utf8", mode: 0o600 });
    const transferredWorkspace = await runCliWith(
      configDirectory,
      {
        input: [
          `put ${uploadPath} notes/from-cli-sftp.txt`,
          `get notes/from-cli-sftp.txt ${downloadPath}`,
          "bye",
          "",
        ].join("\n"),
      },
      ["agent", "thread", "ssh", threadId, "--sftp"],
    );
    expect(transferredWorkspace.exitCode, transferredWorkspace.stderr).toBe(0);
    expect(await readFile(downloadPath, "utf8")).toBe(editorObservation);

    for (const result of [
      activated,
      started,
      shown,
      listed,
      said,
      history,
      bridge,
      agentWorkspace,
      wroteWorkspace,
      resumedWorkspace,
      transferredWorkspace,
    ]) {
      expect(result.stdout).not.toContain(systemTestConfig.token);
      expect(result.stderr).not.toContain(systemTestConfig.token);
      expect(`${result.stdout}${result.stderr}`).not.toMatch(/v4\.public\./);
      expect(`${result.stdout}${result.stderr}`).not.toContain("id_ed25519");
    }
  }, 120_000);
});
