import { execFile, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { randomUUID } from "node:crypto";
import { promisify } from "node:util";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import { systemClient, uniqueName } from "../src/context.js";
import { record, string } from "../src/json.js";
import { finish, runCli, runCliWith, startCli, waitForCode } from "../src/myelin-cli.js";
import {
  generateEphemeralSshKey,
  sftpBatchPath,
  type EphemeralSshKey,
} from "../src/ssh.js";

const run = promisify(execFile);

describe("private agent work from the CLI", () => {
  let configDirectory: string;
  let login: ChildProcessWithoutNullStreams | undefined;
  let editorKey: EphemeralSshKey | undefined;

  beforeAll(async () => {
    configDirectory = await mkdtemp(resolve(tmpdir(), "myelin private thread cli-"));
  });

  afterAll(async () => {
    if (login && login.exitCode === null) login.kill("SIGTERM");
    await editorKey?.remove();
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
    const initialAgentName = uniqueName("CLI checkout companion");
    const activated = await runMyelin(
      "--json",
      "--idempotency-key",
      `cli-private-agent-${randomUUID()}`,
      "agent",
      "create",
      initialAgentName,
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
      workspace_profile: {
        revision: 1,
        personalization: { format: "workspace_augmentation_v1" },
      },
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
    const renamedAgentName = uniqueName("CLI checkout continuity companion");
    const renamed = await runMyelin(
      "--json",
      "--idempotency-key",
      `cli-agent-rename-${randomUUID()}`,
      "agent",
      "rename",
      companionId,
      renamedAgentName,
      "--expected-name",
      initialAgentName,
    );
    expect(renamed.exitCode, renamed.stderr).toBe(0);
    expect(JSON.parse(renamed.stdout)).toMatchObject({
      changed: true,
      durable: true,
      agent: { id: companionId, name: renamedAgentName },
    });
    const namedAgents = await runMyelin("agent", "list", "--limit", "100");
    expect(namedAgents.exitCode, namedAgents.stderr).toBe(0);
    expect(namedAgents.stdout).toContain(renamedAgentName);
    expect(namedAgents.stdout).not.toContain(initialAgentName);

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
          format: "workspace_augmentation_v1",
          content_digest: expect.stringMatching(/^blake3:[0-9a-f]{64}$/),
        },
      },
    });
    const initialExpiry = Date.parse(string(
      record(thread.workspace, "created private workspace").expires_at,
      "created private workspace expiry",
    ));
    const renewalKey = `cli-private-renewal-${randomUUID()}`;
    const renew = () => runMyelin(
      "--json",
      "--idempotency-key",
      renewalKey,
      "agent",
      "thread",
      "renew",
      threadId,
      "--retention-days",
      "30",
    );
    const renewed = await renew();
    expect(renewed.exitCode, renewed.stderr).toBe(0);
    const renewalReceipt = record(JSON.parse(renewed.stdout), "CLI workspace renewal");
    const renewedThread = record(renewalReceipt.thread, "renewed private thread");
    expect(renewalReceipt).toMatchObject({ changed: true, durable: true });
    expect(renewedThread).toMatchObject({
      id: threadId,
      workspace: {
        id: record(thread.workspace, "created private workspace").id,
        retention_days: 30,
      },
    });
    expect(Date.parse(string(
      record(renewedThread.workspace, "renewed private workspace").expires_at,
      "renewed private workspace expiry",
    ))).toBeGreaterThan(initialExpiry);
    const replayed = await renew();
    expect(replayed.exitCode, replayed.stderr).toBe(0);
    expect(JSON.parse(replayed.stdout)).toMatchObject({
      changed: false,
      durable: true,
      thread: {
        id: threadId,
        workspace: { id: record(thread.workspace, "created private workspace").id },
      },
    });

    const shown = await runMyelin("agent", "thread", "show", threadId);
    expect(shown.exitCode, shown.stderr).toBe(0);
    expect(shown.stdout).toContain(`Private agent thread: ${threadName}`);
    expect(shown.stdout).toContain(`myelin agent thread say ${threadId}`);
    expect(shown.stdout).toContain(`myelin agent thread ssh ${threadId}`);
    expect(shown.stdout).toContain(`agent thread renew ${threadId} --retention-days 30`);
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
          `put ${sftpBatchPath(uploadPath)} notes/from-cli-sftp.txt`,
          `get notes/from-cli-sftp.txt ${sftpBatchPath(downloadPath)}`,
          "bye",
          "",
        ].join("\n"),
      },
      ["agent", "thread", "ssh", threadId, "--sftp"],
    );
    expect(transferredWorkspace.exitCode, transferredWorkspace.stderr).toBe(0);
    expect(await readFile(downloadPath, "utf8")).toBe(editorObservation);

    editorKey = await generateEphemeralSshKey();
    const configuredEditor = await runMyelin(
      "--json",
      "agent",
      "thread",
      "ssh-config",
      threadId,
      "--identity",
      editorKey.privateKeyPath,
    );
    expect(configuredEditor.exitCode, configuredEditor.stderr).toBe(0);
    const editorProfile = record(JSON.parse(configuredEditor.stdout), "editor SSH profile");
    const editorHost = string(editorProfile.host_alias, "editor SSH host alias");
    const editorConfig = string(editorProfile.config_path, "editor SSH config path");
    const editorKnownHosts = string(
      editorProfile.known_hosts_path,
      "editor known-hosts path",
    );
    expect(editorProfile).toMatchObject({
      host_alias: `myelin-thread-${threadId}`,
      expires_at: expect.stringMatching(/^20[0-9]{2}-/),
      workspace: {
        id: string(record(thread.workspace, "created workspace").id, "workspace id"),
        generation: 1,
      },
    });
    expect((await stat(editorConfig)).mode & 0o777).toBe(0o600);
    expect((await stat(editorKnownHosts)).mode & 0o777).toBe(0o600);
    const editorConfigText = await readFile(editorConfig, "utf8");
    expect(editorConfigText).toContain(`Host ${editorHost}`);
    expect(editorConfigText).toContain(`IdentityFile \"${editorKey.privateKeyPath}\"`);
    expect(editorConfigText).toContain("StrictHostKeyChecking yes");
    expect(editorConfigText).toContain("ControlMaster auto");
    expect(editorConfigText).not.toContain(systemTestConfig.token);

    const configuredEditorObservation = uniqueName("The configured editor was here");
    const editorCommand = await run(
      "ssh",
      [
        "-F",
        editorConfig,
        editorHost,
        `printf %s '${configuredEditorObservation}' > notes/from-configured-editor.txt`,
      ],
      { timeout: 30_000, maxBuffer: 512 * 1024 },
    );
    expect(editorCommand.stderr).toBe("");

    const profileUpload = resolve(configDirectory, "profile-upload.txt");
    const profileDownload = resolve(configDirectory, "profile-download.txt");
    const profileBatch = resolve(configDirectory, "profile-sftp.batch");
    await writeFile(profileUpload, configuredEditorObservation, { mode: 0o600 });
    await writeFile(
      profileBatch,
      `put ${sftpBatchPath(profileUpload)} notes/from-profile-sftp.txt\nget notes/from-profile-sftp.txt ${sftpBatchPath(profileDownload)}\n`,
      { mode: 0o600 },
    );
    await run("sftp", ["-F", editorConfig, "-b", profileBatch, editorHost], {
      timeout: 30_000,
      maxBuffer: 512 * 1024,
    });
    expect(await readFile(profileDownload, "utf8")).toBe(configuredEditorObservation);
    const editorRead = await run(
      "ssh",
      ["-F", editorConfig, editorHost, "cat notes/from-configured-editor.txt"],
      { timeout: 30_000, maxBuffer: 512 * 1024 },
    );
    expect(editorRead.stdout).toBe(configuredEditorObservation);
    await run("ssh", ["-F", editorConfig, "-O", "exit", editorHost], {
      timeout: 10_000,
      maxBuffer: 512 * 1024,
    });

    for (const result of [
      activated,
      renamed,
      namedAgents,
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
      configuredEditor,
    ]) {
      expect(result.stdout).not.toContain(systemTestConfig.token);
      expect(result.stderr).not.toContain(systemTestConfig.token);
      expect(`${result.stdout}${result.stderr}`).not.toMatch(/v4\.public\./);
      expect(`${result.stdout}${result.stderr}`).not.toContain("id_ed25519");
    }
  }, 120_000);
});
