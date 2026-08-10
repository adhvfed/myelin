import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { gitRepositoryUrl, systemTestConfig } from "../src/config.js";
import { eventually } from "../src/eventually.js";
import { git } from "../src/git-cli.js";
import { GitProject } from "../src/git-project.js";
import { array, integer, record, string, type JsonRecord } from "../src/json.js";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

type AgentRunEnvelope = {
  run: {
    id: string;
    ref: string;
    agent_id: string;
    agent_ref: string;
    principal_id: string;
    trigger_actor: string;
    selected_tools: Array<{ name: string; version: number; ref: string }>;
    effective_grants: string[];
    state: string;
    issued_at: string;
    expires_at: string;
  };
  credential: { scheme: string; token: string; expires_at: string };
  created: boolean;
  durable: boolean;
};

async function askAgent(
  run: AgentRunEnvelope,
  id: number,
  tool: string,
  arguments_: JsonRecord,
): Promise<JsonRecord> {
  const response = await systemClient.json(`/v1/agent-runs/${run.run.id}/mcp`, {
    method: "POST",
    body: {
      jsonrpc: "2.0",
      id,
      method: "tools/call",
      params: { name: tool, arguments: arguments_ },
    },
    token: run.credential.token,
    tokenScheme: "agent",
    expectedStatus: 200,
  });
  expect(JSON.stringify(response.body)).not.toContain(run.credential.token);
  const result = record(response.body.result, `${tool} MCP result`);
  expect(result, `${tool} MCP call failed: ${JSON.stringify(result)}`).toMatchObject({
    isError: false,
    _meta: { tool },
  });
  const content = array(result.content, `${tool} MCP content`);
  expect(content).toHaveLength(1);
  const text = string(record(content[0], `${tool} MCP content item`).text, `${tool} MCP text`);
  expect(text).not.toContain(run.credential.token);
  return record(JSON.parse(text), `${tool} payload`);
}

async function askAgentToAct(
  run: AgentRunEnvelope,
  id: number,
  tool: string,
  arguments_: JsonRecord,
  idempotencyKey: string = `system-${tool}-${randomUUID()}`,
): Promise<JsonRecord> {
  const response = await systemClient.json(`/v1/agent-runs/${run.run.id}/mcp`, {
    method: "POST",
    body: {
      jsonrpc: "2.0",
      id,
      method: "tools/call",
      params: {
        name: tool,
        arguments: arguments_,
        _meta: { "com.myelin/idempotencyKey": idempotencyKey },
      },
    },
    token: run.credential.token,
    tokenScheme: "agent",
    expectedStatus: 200,
  });
  expect(JSON.stringify(response.body)).not.toContain(run.credential.token);
  const result = record(response.body.result, `${tool} MCP result`);
  expect(result, `${tool} MCP call failed: ${JSON.stringify(result)}`).toMatchObject({
    isError: false,
    _meta: { tool, eventId: expect.any(String) },
  });
  const metadata = record(result._meta, `${tool} MCP result metadata`);
  const receipt = record(result.structuredContent, `${tool} MCP structured receipt`);
  expect(receipt).toMatchObject({ event_id: metadata.eventId });
  return receipt;
}

function cliEnvironment(
  configDirectory: string,
  additions: NodeJS.ProcessEnv = {},
): NodeJS.ProcessEnv {
  const environment: NodeJS.ProcessEnv = {
    ...process.env,
    MYELIN_CONFIG_DIR: configDirectory,
    MYELIN_TEST_CREDENTIAL_STORE: "file",
  };
  delete environment.MYELIN_TOKEN;
  delete environment.MYELIN_TOKEN_SCHEME;
  delete environment.MYELIN_EDGE;
  delete environment.MYELIN_PROFILE;
  return { ...environment, ...additions };
}

function startCli(configDirectory: string, ...args: string[]): ChildProcessWithoutNullStreams {
  return spawn(
    "cargo",
    ["run", "--quiet", "-p", "myelin-cli", "--", ...args],
    {
      cwd: repository,
      env: cliEnvironment(configDirectory),
      stdio: "pipe",
    },
  );
}

async function waitForCode(
  child: ChildProcessWithoutNullStreams,
): Promise<{ code: string; stdout: () => string; stderr: () => string }> {
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => { stdout += chunk; });
  child.stderr.on("data", (chunk: string) => { stderr += chunk; });

  const code = await new Promise<string>((resolveCode, reject) => {
    const timeout = setTimeout(() => {
      reject(new Error(`CLI did not print an approval code; stdout=${stdout} stderr=${stderr}`));
    }, 30_000);
    const inspect = () => {
      const match = stdout.match(/Confirm this code in your browser: ([A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4})/);
      if (!match) return;
      clearTimeout(timeout);
      resolveCode(match[1]!);
    };
    child.stdout.on("data", inspect);
    child.once("exit", (exitCode) => {
      clearTimeout(timeout);
      reject(new Error(`CLI exited ${exitCode} before approval; stdout=${stdout} stderr=${stderr}`));
    });
    inspect();
  });
  return { code, stdout: () => stdout, stderr: () => stderr };
}

async function finish(
  child: ChildProcessWithoutNullStreams,
  output: { stdout: () => string; stderr: () => string },
): Promise<string> {
  const exitCode = await new Promise<number | null>((resolveExit, reject) => {
    const timeout = setTimeout(() => reject(new Error("CLI did not finish after approval")), 15_000);
    child.once("error", reject);
    child.once("exit", (code) => {
      clearTimeout(timeout);
      resolveExit(code);
    });
  });
  expect(exitCode, `stderr=${output.stderr()}`).toBe(0);
  return output.stdout();
}

async function runCli(configDirectory: string, ...args: string[]) {
  return runCliWith(configDirectory, {}, args);
}

async function runCliWith(
  configDirectory: string,
  options: { environment?: NodeJS.ProcessEnv; input?: string },
  args: string[],
) {
  const child = spawn("cargo", ["run", "--quiet", "-p", "myelin-cli", "--", ...args], {
    cwd: repository,
    env: cliEnvironment(configDirectory, options.environment),
    stdio: "pipe",
  });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => { stdout += chunk; });
  child.stderr.on("data", (chunk: string) => { stderr += chunk; });
  child.stdin.end(options.input);
  const exitCode = await new Promise<number | null>((resolveExit, reject) => {
    child.once("error", reject);
    child.once("exit", resolveExit);
  });
  return { exitCode, stdout, stderr };
}

async function askGitForCredential(
  configDirectory: string,
  gitConfig: string,
  request: string,
) {
  const child = spawn("git", ["credential", "fill"], {
    cwd: repository,
    env: cliEnvironment(configDirectory, {
      GIT_CONFIG_GLOBAL: gitConfig,
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_TERMINAL_PROMPT: "0",
    }),
    stdio: "pipe",
  });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => { stdout += chunk; });
  child.stderr.on("data", (chunk: string) => { stderr += chunk; });
  child.stdin.end(request);
  const exitCode = await new Promise<number | null>((resolveExit, reject) => {
    child.once("error", reject);
    child.once("exit", resolveExit);
  });
  return { exitCode, stdout, stderr };
}

function profileSection(config: string, profile: string): string {
  const marker = `[profiles.${profile}]`;
  const start = config.indexOf(marker);
  if (start === -1) throw new Error(`config has no ${marker} section`);
  const next = config.indexOf("\n[profiles.", start + marker.length);
  return config.slice(start, next === -1 ? config.length : next);
}

function expireProfile(config: string, profile: string): string {
  const section = profileSection(config, profile);
  const expired = section.replace(/^expires_at_unix = \d+$/m, "expires_at_unix = 1");
  if (expired === section) throw new Error(`profile ${profile} has no numeric expiry`);
  return config.replace(section, expired);
}

describe("the CLI authentication journey", () => {
  test("a developer approves in the browser once, then works without copying an API key", async () => {
    const configDirectory = await mkdtemp(resolve(tmpdir(), "myelin-cli-system-"));
    let login: ChildProcessWithoutNullStreams | undefined;
    try {
      login = startCli(
        configDirectory,
        "--edge",
        systemTestConfig.edgeUrl,
        "auth",
        "login",
        "--no-browser",
      );
      const output = await waitForCode(login);

      const approved = await systemClient.json("/v1/auth/device/approval", {
        method: "POST",
        body: { user_code: output.code },
      });
      expect(approved.body).toEqual({ approved: true });

      const loginStory = await finish(login, output);
      expect(loginStory).toContain("Approved. Your CLI session is ready");
      expect(loginStory).not.toContain(systemTestConfig.token);

      const configPath = resolve(configDirectory, "config.toml");
      const firstConfig = await readFile(configPath, "utf8");
      const defaultProfile = profileSection(firstConfig, "default");
      expect(firstConfig).toContain('active_profile = "default"');
      expect(defaultProfile).toContain('scheme = "session"');
      expect(defaultProfile).toContain(`edge_url = "${systemTestConfig.edgeUrl}"`);
      expect(defaultProfile).toContain(`tenant = "${systemTestConfig.tenant}"`);
      expect(defaultProfile).toContain(`region = "${systemTestConfig.region}"`);
      expect(defaultProfile).toMatch(/^credential_ref = "[A-Za-z0-9_-]{22}"$/m);
      expect(defaultProfile).toMatch(/^expires_at_unix = \d+$/m);
      expect(firstConfig).not.toContain(systemTestConfig.token);
      if (process.platform !== "win32") {
        expect((await stat(configPath)).mode & 0o777).toBe(0o600);
      }

      // A second browser-approved identity becomes another named context—not another copied key.
      login = startCli(
        configDirectory,
        "--profile",
        "reviewer",
        "--edge",
        systemTestConfig.edgeUrl,
        "auth",
        "login",
        "--no-browser",
      );
      const reviewerOutput = await waitForCode(login);
      await reviewerClient.json("/v1/auth/device/approval", {
        method: "POST",
        body: { user_code: reviewerOutput.code },
      });
      const reviewerLoginStory = await finish(login, reviewerOutput);
      expect(reviewerLoginStory).toContain("Approved. Your CLI session is ready");
      expect(reviewerLoginStory).not.toContain(systemTestConfig.reviewerToken);

      const contexts = await runCli(configDirectory, "--json", "context", "list");
      expect(contexts.exitCode, contexts.stderr).toBe(0);
      expect(JSON.parse(contexts.stdout)).toMatchObject({
        profiles: [
          {
            name: "default",
            active: false,
            tenant: systemTestConfig.tenant,
            region: systemTestConfig.region,
          },
          {
            name: "reviewer",
            active: true,
            tenant: systemTestConfig.tenant,
            region: systemTestConfig.region,
          },
        ],
      });
      const twoProfileConfig = await readFile(configPath, "utf8");
      expect(twoProfileConfig.match(/^credential_ref = /gm)).toHaveLength(2);
      expect(twoProfileConfig).not.toContain(systemTestConfig.token);
      expect(twoProfileConfig).not.toContain(systemTestConfig.reviewerToken);

      const reviewerContext = await runCli(configDirectory, "--json", "context", "current");
      expect(reviewerContext.exitCode, reviewerContext.stderr).toBe(0);
      expect(JSON.parse(reviewerContext.stdout)).toMatchObject({
        profile: "reviewer",
        edge_url: systemTestConfig.edgeUrl,
        identity: {
          principal_id: systemTestConfig.reviewerPrincipal,
          tenant: systemTestConfig.tenant,
          region: systemTestConfig.region,
        },
      });

      const gitConfig = resolve(configDirectory, "gitconfig");
      const gitEnvironment = {
        GIT_CONFIG_GLOBAL: gitConfig,
        GIT_CONFIG_NOSYSTEM: "1",
      };
      const configureReviewerGit = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["auth", "configure-git"],
      );
      expect(configureReviewerGit.exitCode, configureReviewerGit.stderr).toBe(0);

      const chooseDefault = await runCli(configDirectory, "context", "use", "default");
      expect(chooseDefault.exitCode, chooseDefault.stderr).toBe(0);
      expect(chooseDefault.stdout).toContain("Using CLI context `default`");

      const status = await runCli(configDirectory, "auth", "status");
      expect(status.exitCode, status.stderr).toBe(0);
      expect(status.stdout).toContain(systemTestConfig.principal);
      expect(status.stdout).toContain(`tenant=${systemTestConfig.tenant}`);

      // The same browser session explains the platform's shared vocabulary to people and agents.
      // No GitHub token, Slack token, or separate agent credential is another prerequisite.
      const toolList = await runCli(configDirectory, "--json", "tool", "list", "--limit", "2");
      expect(toolList.exitCode, toolList.stderr).toBe(0);
      const catalogue = JSON.parse(toolList.stdout) as {
        items: Array<{
          name: string;
          ref: string;
          version: number;
          input_schema: { type?: string };
          required_capabilities: string[];
        }>;
        page: { next_cursor: string | null; limit: number };
      };
      expect(catalogue.items).toHaveLength(2);
      expect(catalogue.page).toMatchObject({ limit: 2 });
      expect(catalogue.page.next_cursor).toMatch(/^[a-z][a-z0-9_]*\.[a-z][a-z0-9_]*\.v\d+$/);
      for (const tool of catalogue.items) {
        expect(tool.name).toMatch(/^[a-z][a-z0-9_]*\.[a-z][a-z0-9_]*$/);
        expect(tool.ref).toBe(
          `myelin://${systemTestConfig.tenant}/agent/tool/${tool.name}/v${tool.version}`,
        );
        expect(tool.input_schema.type).toBe("object");
        expect(tool.required_capabilities.length).toBeGreaterThan(0);
      }

      const showTool = await runCli(
        configDirectory,
        "--json",
        "tool",
        "show",
        catalogue.items[0]!.name,
      );
      expect(showTool.exitCode, showTool.stderr).toBe(0);
      expect(JSON.parse(showTool.stdout)).toMatchObject({ tool: catalogue.items[0] });

      const nextTools = await runCli(
        configDirectory,
        "tool",
        "list",
        "--limit",
        "2",
        "--cursor",
        catalogue.page.next_cursor!,
      );
      expect(nextTools.exitCode, nextTools.stderr).toBe(0);
      expect(nextTools.stdout).toContain("myelin://");

      const mcpDescription = await runCli(configDirectory, "tool", "describe", "--mcp");
      expect(mcpDescription.exitCode, mcpDescription.stderr).toBe(0);
      const mcpManifest = JSON.parse(mcpDescription.stdout) as {
        tools: Array<{
          name: string;
          inputSchema: { type?: string };
          annotations: { requiresApproval?: boolean; readOnlyHint?: boolean };
        }>;
      };
      expect(mcpManifest.tools.map((tool) => tool.name)).toEqual(
        expect.arrayContaining([
          "git.open_pr",
          "git.write_file",
          "git.list_repositories",
          "git.read_file",
          "git.search_code",
          "git.merge",
          "ci.read_run",
          "ci.read_log",
          "issues.create",
          "issues.list",
          "issues.view",
          "knowledge.list_pages",
          "knowledge.read_page",
          "chat.list_conversations",
          "chat.post",
          "chat.read_messages",
        ]),
      );
      expect(mcpManifest.tools.every((tool) => tool.inputSchema.type === "object")).toBe(true);
      expect(mcpDescription.stdout).not.toContain(systemTestConfig.token);

      // A human activates an external collaborator by choosing from that same vocabulary.
      // Retrying creates neither a second identity nor a long-lived credential to distribute.
      const agentName = uniqueName("Review companion");
      const agentRetryKey = `cli-agent-${randomUUID()}`;
      const createAgent = await runCli(
        configDirectory,
        "--json",
        "--idempotency-key",
        agentRetryKey,
        "agent",
        "create",
        agentName,
        "--tool",
        "ci.read_run",
        "--tool",
        "git.open_pr",
        "--tool",
        "git.write_file",
        "--tool",
        "git.list_repositories",
        "--tool",
        "git.read_file",
        "--tool",
        "git.search_code",
        "--tool",
        "issues.create",
        "--tool",
        "issues.list",
        "--tool",
        "issues.view",
        "--tool",
        "knowledge.list_pages",
        "--tool",
        "knowledge.read_page",
        "--tool",
        "chat.list_conversations",
        "--tool",
        "chat.post",
        "--tool",
        "chat.read_messages",
      );
      expect(createAgent.exitCode, createAgent.stderr).toBe(0);
      const activated = JSON.parse(createAgent.stdout) as {
        agent: {
          id: string;
          ref: string;
          principal_id: string;
          name: string;
          runtime_ref: string;
          on_behalf_of: string;
          status: string;
          selected_tools: Array<{ name: string; version: number; ref: string }>;
          effective_tools: Array<{ name: string; version: number; ref: string }>;
          grants: string[];
        };
        created: boolean;
        durable: boolean;
      };
      expect(activated).toMatchObject({
        created: true,
        durable: true,
        agent: {
          name: agentName,
          runtime_ref: "external:mcp",
          on_behalf_of: systemTestConfig.principal,
          status: "active",
          selected_tools: [
            { name: "chat.list_conversations", version: 1 },
            { name: "chat.post", version: 1 },
            { name: "chat.read_messages", version: 1 },
            { name: "ci.read_run", version: 1 },
            { name: "git.list_repositories", version: 1 },
            { name: "git.open_pr", version: 1 },
            { name: "git.read_file", version: 1 },
            { name: "git.search_code", version: 1 },
            { name: "git.write_file", version: 1 },
            { name: "issues.create", version: 1 },
            { name: "issues.list", version: 1 },
            { name: "issues.view", version: 1 },
            { name: "knowledge.list_pages", version: 1 },
            { name: "knowledge.read_page", version: 1 },
          ],
          grants: expect.arrayContaining([
            "agent.tools.read",
            "chat.read",
            "chat.post",
            "edge.identity.read",
            "issue.create",
            "issue.view",
            "knowledge.read",
            "repo.pull",
            "repo.push",
            "run.view",
          ]),
        },
      });
      expect(activated.agent.id).toMatch(
        /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
      );
      expect(activated.agent.principal_id).toBe(`agent:${activated.agent.id}`);
      expect(activated.agent.ref).toBe(
        `myelin://${systemTestConfig.tenant}/identity/agent/${activated.agent.id}`,
      );
      expect(activated.agent.effective_tools.map((tool) => tool.name)).toEqual(
        expect.arrayContaining([
          "ci.read_run",
          "ci.read_log",
          "git.open_pr",
          "git.write_file",
          "git.list_repositories",
          "git.read_file",
          "git.search_code",
          "issues.create",
          "issues.list",
          "issues.view",
          "knowledge.list_pages",
          "knowledge.read_page",
          "chat.list_conversations",
          "chat.post",
          "chat.read_messages",
        ]),
      );
      for (const tool of activated.agent.selected_tools) {
        expect(tool.ref).toBe(
          `myelin://${systemTestConfig.tenant}/agent/tool/${tool.name}/v${tool.version}`,
        );
      }
      expect(createAgent.stdout).not.toContain(systemTestConfig.token);
      expect(createAgent.stdout).not.toMatch(/api[_ -]?key|credential/i);

      const replayAgent = await runCli(
        configDirectory,
        "--json",
        "--idempotency-key",
        agentRetryKey,
        "agent",
        "create",
        agentName,
        "--tool",
        "ci.read_run",
        "--tool",
        "git.open_pr",
        "--tool",
        "git.write_file",
        "--tool",
        "git.list_repositories",
        "--tool",
        "git.read_file",
        "--tool",
        "git.search_code",
        "--tool",
        "issues.create",
        "--tool",
        "issues.list",
        "--tool",
        "issues.view",
        "--tool",
        "knowledge.list_pages",
        "--tool",
        "knowledge.read_page",
        "--tool",
        "chat.list_conversations",
        "--tool",
        "chat.post",
        "--tool",
        "chat.read_messages",
      );
      expect(replayAgent.exitCode, replayAgent.stderr).toBe(0);
      expect(JSON.parse(replayAgent.stdout)).toMatchObject({
        created: false,
        durable: true,
        agent: { id: activated.agent.id, ref: activated.agent.ref },
      });

      const agentRoster = await runCli(
        configDirectory,
        "--json",
        "agent",
        "list",
        "--limit",
        "100",
      );
      expect(agentRoster.exitCode, agentRoster.stderr).toBe(0);
      const roster = JSON.parse(agentRoster.stdout) as {
        items: Array<{ id: string; ref: string; name: string }>;
      };
      expect(roster.items).toContainEqual(
        expect.objectContaining({
          id: activated.agent.id,
          ref: activated.agent.ref,
          name: agentName,
        }),
      );

      const showAgent = await runCli(configDirectory, "agent", "show", activated.agent.id);
      expect(showAgent.exitCode, showAgent.stderr).toBe(0);
      expect(showAgent.stdout).toContain(`Agent: ${agentName}`);
      expect(showAgent.stdout).toContain(activated.agent.ref);
      expect(showAgent.stdout).toContain("no long-lived API key was created");
      expect(showAgent.stdout).not.toContain(systemTestConfig.token);

      // Starting work exchanges that browser session for one minute of agent authority. A lost
      // response is safe to retry: Edge returns the same run and credential, not a sibling run.
      const defaultCredentialRef = defaultProfile.match(
        /^credential_ref = "([A-Za-z0-9_-]{22})"$/m,
      )?.[1];
      expect(defaultCredentialRef).toBeTruthy();
      const browserSession = await readFile(
        resolve(configDirectory, ".test-credentials", defaultCredentialRef!),
        "utf8",
      );
      const runRetryKey = `cli-agent-run-${randomUUID()}`;
      const startRun = await systemClient.json(`/v1/agents/${activated.agent.id}/runs`, {
        method: "POST",
        body: {},
        token: browserSession,
        tokenScheme: "session",
        idempotencyKey: runRetryKey,
        expectedStatus: 201,
      });
      const running = startRun.body as unknown as AgentRunEnvelope;
      expect(startRun.headers.get("cache-control")).toBe("no-store");
      expect(running).toMatchObject({
        created: true,
        durable: true,
        run: {
          agent_id: activated.agent.id,
          agent_ref: activated.agent.ref,
          principal_id: activated.agent.principal_id,
          trigger_actor: systemTestConfig.principal,
          selected_tools: [
            { name: "chat.list_conversations", version: 1 },
            { name: "chat.post", version: 1 },
            { name: "chat.read_messages", version: 1 },
            { name: "ci.read_run", version: 1 },
            { name: "git.list_repositories", version: 1 },
            { name: "git.open_pr", version: 1 },
            { name: "git.read_file", version: 1 },
            { name: "git.search_code", version: 1 },
            { name: "git.write_file", version: 1 },
            { name: "issues.create", version: 1 },
            { name: "issues.list", version: 1 },
            { name: "issues.view", version: 1 },
            { name: "knowledge.list_pages", version: 1 },
            { name: "knowledge.read_page", version: 1 },
          ],
          effective_grants: activated.agent.grants,
          state: "ready",
        },
        credential: { scheme: "agent" },
      });
      expect(running.run.id).toMatch(
        /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
      );
      expect(running.run.ref).toBe(
        `myelin://${systemTestConfig.tenant}/agent/run/${running.run.id}`,
      );
      expect(running.credential.token).toMatch(/^v4\.public\./);
      expect(running.credential.expires_at).toBe(running.run.expires_at);
      expect(Date.parse(running.run.expires_at) - Date.parse(running.run.issued_at)).toBe(60_000);

      const replayRun = await systemClient.json(`/v1/agents/${activated.agent.id}/runs`, {
        method: "POST",
        body: {},
        token: browserSession,
        tokenScheme: "session",
        idempotencyKey: runRetryKey,
        expectedStatus: 200,
      });
      expect(replayRun.body).toMatchObject({
        created: false,
        durable: true,
        run: { id: running.run.id, ref: running.run.ref, state: "ready" },
        credential: { scheme: "agent", token: running.credential.token },
      });

      // An MCP client brings only the transient run credential. Edge recovers every other fact
      // from durable identity state and presents exactly the tools the human selected—not a
      // capability-equivalent sibling and never a second long-lived provider key.
      const initialized = await systemClient.json(`/v1/agent-runs/${running.run.id}/mcp`, {
        method: "POST",
        body: { jsonrpc: "2.0", id: 1, method: "initialize", params: {} },
        token: running.credential.token,
        tokenScheme: "agent",
        expectedStatus: 200,
      });
      expect(initialized.headers.get("cache-control")).toBe("no-store");
      expect(initialized.body).toMatchObject({
        jsonrpc: "2.0",
        id: 1,
        result: {
          protocolVersion: "2025-06-18",
          serverInfo: { name: "myelin-mcp" },
        },
      });

      const discovered = await systemClient.json(`/v1/agent-runs/${running.run.id}/mcp`, {
        method: "POST",
        body: { jsonrpc: "2.0", id: 2, method: "tools/list" },
        token: running.credential.token,
        tokenScheme: "agent",
        expectedStatus: 200,
      });
      const discoveredResult = record(discovered.body.result, "MCP tools/list result");
      const discoveredTools = array(discoveredResult.tools, "MCP tools/list tools").map(
        (tool, index) => record(tool, `MCP tool ${index}`),
      );
      expect(discoveredTools.map((tool) => string(tool.name, "MCP tool name"))).toEqual([
        "chat.list_conversations",
        "chat.post",
        "chat.read_messages",
        "ci.read_run",
        "git.list_repositories",
        "git.open_pr",
        "git.read_file",
        "git.search_code",
        "git.write_file",
        "issues.create",
        "issues.list",
        "issues.view",
        "knowledge.list_pages",
        "knowledge.read_page",
      ]);
      const toolsByName = new Map(
        discoveredTools.map((tool) => [string(tool.name, "MCP tool name"), tool]),
      );
      const schemaFor = (name: string) =>
        record(toolsByName.get(name)?.inputSchema, `${name} input schema`);
      expect(schemaFor("chat.list_conversations")).toMatchObject({
        type: "object",
        additionalProperties: false,
      });
      expect(schemaFor("chat.post")).toMatchObject({
        type: "object",
        required: ["conversation_id", "content"],
        additionalProperties: false,
      });
      expect(schemaFor("chat.read_messages")).toMatchObject({
        type: "object",
        required: ["conversation_id"],
        additionalProperties: false,
      });
      expect(schemaFor("ci.read_run")).toMatchObject({
        type: "object",
        required: ["run_id"],
        additionalProperties: false,
      });
      expect(schemaFor("git.list_repositories")).toMatchObject({
        type: "object",
        additionalProperties: false,
      });
      expect(schemaFor("git.open_pr")).toMatchObject({
        type: "object",
        required: ["repo", "title"],
        additionalProperties: false,
      });
      expect(schemaFor("git.read_file")).toMatchObject({
        type: "object",
        required: ["repo", "ref", "path"],
        additionalProperties: false,
      });
      expect(schemaFor("git.search_code")).toMatchObject({
        type: "object",
        required: ["query"],
        additionalProperties: false,
      });
      expect(schemaFor("git.write_file")).toMatchObject({
        type: "object",
        required: ["repo", "ref", "path", "contents", "base_oid"],
        additionalProperties: false,
      });
      expect(schemaFor("issues.create")).toMatchObject({
        type: "object",
        required: ["project_id", "title"],
        additionalProperties: false,
      });
      expect(schemaFor("issues.list")).toMatchObject({
        type: "object",
        additionalProperties: false,
      });
      expect(schemaFor("issues.view")).toMatchObject({
        type: "object",
        required: ["issue_id"],
        additionalProperties: false,
      });
      expect(schemaFor("knowledge.list_pages")).toMatchObject({
        type: "object",
        additionalProperties: false,
      });
      expect(schemaFor("knowledge.read_page")).toMatchObject({
        type: "object",
        required: ["page_id"],
        additionalProperties: false,
      });

      // Finishing work destroys that transient identity as one durable operation. The run can be
      // inspected as closed, while its bearer material becomes useless immediately.
      const closeRun = await systemClient.json(`/v1/agent-runs/${running.run.id}/close`, {
        method: "POST",
        body: {},
        token: running.credential.token,
        tokenScheme: "agent",
        expectedStatus: 200,
      });
      expect(closeRun.headers.get("cache-control")).toBe("no-store");
      expect(closeRun.body).toEqual({
        run: {
          id: running.run.id,
          ref: running.run.ref,
          agent_id: activated.agent.id,
          agent_ref: activated.agent.ref,
          state: "closed",
        },
        closed: true,
        durable: true,
      });

      await systemClient.json("/v1/whoami", {
        token: running.credential.token,
        tokenScheme: "agent",
        expectedStatus: 401,
      });
      await systemClient.json(`/v1/agents/${activated.agent.id}/runs`, {
        method: "POST",
        body: {},
        token: browserSession,
        tokenScheme: "session",
        idempotencyKey: runRetryKey,
        expectedStatus: 409,
      });

      // An MCP client needs only the ordinary CLI command in its server configuration. The CLI
      // exchanges the saved browser session for a transient run behind the scenes, keeps protocol
      // stdout clean, ignores notifications, and closes the run when the client closes stdin.
      const bridge = await runCliWith(
        configDirectory,
        {
          input: [
            JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} }),
            JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }),
            JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/list" }),
            "",
          ].join("\n"),
        },
        ["mcp", "serve", "--as", activated.agent.id],
      );
      expect(bridge.exitCode, bridge.stderr).toBe(0);
      expect(bridge.stderr).toBe("");
      expect(bridge.stdout).not.toMatch(/v4\.public/);
      expect(bridge.stdout).not.toContain(browserSession);
      expect(bridge.stdout).not.toContain(systemTestConfig.token);
      const bridgeResponses = bridge.stdout
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line) as Record<string, unknown>);
      expect(bridgeResponses).toHaveLength(2);
      expect(bridgeResponses.at(0)).toMatchObject({
        jsonrpc: "2.0",
        id: 1,
        result: {
          protocolVersion: "2025-06-18",
          serverInfo: { name: "myelin-mcp" },
        },
      });
      const bridgeTools = array(
        record(bridgeResponses.at(1)?.result, "CLI MCP tools/list result").tools,
        "CLI MCP tools/list tools",
      ).map((tool, index) => string(record(tool, `CLI MCP tool ${index}`).name, "tool name"));
      expect(bridgeTools).toEqual([
        "chat.list_conversations",
        "chat.post",
        "chat.read_messages",
        "ci.read_run",
        "git.list_repositories",
        "git.open_pr",
        "git.read_file",
        "git.search_code",
        "git.write_file",
        "issues.create",
        "issues.list",
        "issues.view",
        "knowledge.list_pages",
        "knowledge.read_page",
      ]);

      // Pausing a collaborator is one durable human action: new work stops and in-flight bearer
      // authority dies with it. The same request is safe to repeat after a lost response.
      const workBeforePause = await systemClient.json(
        `/v1/agents/${activated.agent.id}/runs`,
        {
          method: "POST",
          body: {},
          token: browserSession,
          tokenScheme: "session",
          idempotencyKey: `cli-agent-before-pause-${randomUUID()}`,
          expectedStatus: 201,
        },
      );
      const pausedRun = workBeforePause.body as unknown as AgentRunEnvelope;
      const pauseKey = `cli-agent-pause-${randomUUID()}`;
      const pauseAgent = await runCli(
        configDirectory,
        "--idempotency-key",
        pauseKey,
        "agent",
        "suspend",
        activated.agent.id,
      );
      expect(pauseAgent.exitCode, pauseAgent.stderr).toBe(0);
      expect(pauseAgent.stdout).toContain(`Suspended agent: ${agentName}`);
      expect(pauseAgent.stdout).toContain("stopped 1 active run");
      expect(pauseAgent.stdout).toContain("blocked until this identity is resumed");
      expect(pauseAgent.stdout).not.toContain(pausedRun.credential.token);

      const replayPause = await runCli(
        configDirectory,
        "--idempotency-key",
        pauseKey,
        "agent",
        "suspend",
        activated.agent.id,
      );
      expect(replayPause.exitCode, replayPause.stderr).toBe(0);
      expect(replayPause.stdout).toBe(pauseAgent.stdout);

      const suspendedAgent = await runCli(
        configDirectory,
        "--json",
        "agent",
        "show",
        activated.agent.id,
      );
      expect(suspendedAgent.exitCode, suspendedAgent.stderr).toBe(0);
      expect(JSON.parse(suspendedAgent.stdout)).toMatchObject({
        agent: { id: activated.agent.id, status: "suspended" },
      });
      await systemClient.json("/v1/whoami", {
        token: pausedRun.credential.token,
        tokenScheme: "agent",
        expectedStatus: 401,
      });

      const workWhilePaused = await runCliWith(
        configDirectory,
        {
          input: `${JSON.stringify({
            jsonrpc: "2.0",
            id: 1,
            method: "initialize",
            params: {},
          })}\n`,
        },
        ["mcp", "serve", "--as", activated.agent.id],
      );
      expect(workWhilePaused.exitCode).toBe(1);
      expect(workWhilePaused.stdout).toBe("");
      expect(workWhilePaused.stderr).toContain("agent is suspended");

      // Resume permits fresh work but never resurrects authority killed by the pause.
      const resumeAgent = await runCli(
        configDirectory,
        "--idempotency-key",
        `cli-agent-resume-${randomUUID()}`,
        "agent",
        "resume",
        activated.agent.id,
      );
      expect(resumeAgent.exitCode, resumeAgent.stderr).toBe(0);
      expect(resumeAgent.stdout).toContain(`Resumed agent: ${agentName}`);
      expect(resumeAgent.stdout).toContain("previously terminated runs remain closed");
      await systemClient.json("/v1/whoami", {
        token: pausedRun.credential.token,
        tokenScheme: "agent",
        expectedStatus: 401,
      });

      // A founder names the first project once. Its generated identity becomes context,
      // so show and subsequent work no longer carry an operator-provided UUID.
      const cliProjectName = uniqueName("Developer experience");
      const cliProjectPrefix = `C${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`;
      const createProject = await runCli(
        configDirectory,
        "--json",
        "--idempotency-key",
        `cli-project-${randomUUID()}`,
        "project",
        "create",
        cliProjectName,
        "--prefix",
        cliProjectPrefix,
      );
      expect(createProject.exitCode, createProject.stderr).toBe(0);
      const createdProject = JSON.parse(createProject.stdout) as {
        project: {
          id: string;
          ref: string;
          name: string;
          issue_prefix: string;
          default_issue_type_id: string;
        };
        created: boolean;
      };
      expect(createdProject).toMatchObject({
        created: true,
        project: { name: cliProjectName, issue_prefix: cliProjectPrefix },
      });
      expect(createdProject.project.ref).toBe(
        `myelin://${systemTestConfig.tenant}/identity/project/${createdProject.project.id}`,
      );

      const showActiveProject = await runCli(configDirectory, "project", "show");
      expect(showActiveProject.exitCode, showActiveProject.stderr).toBe(0);
      expect(showActiveProject.stdout).toContain(cliProjectName);
      expect(showActiveProject.stdout).toContain(createdProject.project.ref);

      const listProjects = await runCli(configDirectory, "project", "list", "--limit", "100");
      expect(listProjects.exitCode, listProjects.stderr).toBe(0);
      expect(listProjects.stdout).toContain(cliProjectName);
      expect(listProjects.stdout).toContain(createdProject.project.ref);

      const contextAfterCreation = await runCli(configDirectory, "--json", "context", "current");
      expect(contextAfterCreation.exitCode, contextAfterCreation.stderr).toBe(0);
      expect(JSON.parse(contextAfterCreation.stdout)).toMatchObject({
        profile: "default",
        project: createdProject.project.id,
      });

      // The active project's prefix and issue type are platform metadata, not user ceremony.
      const contextualIssueTitle = uniqueName("Created from the active project");
      const contextualIssueKey = uniqueName("cli-context-issue");
      const contextualIssue = await runCli(
        configDirectory,
        "--json",
        "--idempotency-key",
        contextualIssueKey,
        "issue",
        "create",
        contextualIssueTitle,
      );
      expect(contextualIssue.exitCode, contextualIssue.stderr).toBe(0);
      const issueProposal = record(JSON.parse(contextualIssue.stdout), "CLI issue proposal");
      const issueSummary = record(issueProposal.issue, "CLI issue summary");
      const issueAuthorization = record(
        issueProposal.authorization,
        "CLI issue authorization",
      );
      const issueId = string(issueSummary.id, "CLI issue id");
      const issueKey = string(issueSummary.key, "CLI issue key");
      const requestEventId = string(
        issueAuthorization.request_event_id,
        "CLI issue authorization request id",
      );
      expect(issueProposal).toMatchObject({
        created: true,
        durable: true,
        issue: {
          project_id: createdProject.project.id,
          key: expect.stringMatching(new RegExp(`^${createdProject.project.issue_prefix}-\\d+$`)),
        },
        authorization: { status: "pending" },
      });

      const replayedContextualIssue = await runCli(
        configDirectory,
        "--json",
        "--idempotency-key",
        contextualIssueKey,
        "issue",
        "create",
        contextualIssueTitle,
      );
      expect(replayedContextualIssue.exitCode, replayedContextualIssue.stderr).toBe(0);
      expect(JSON.parse(replayedContextualIssue.stdout)).toEqual({
        ...issueProposal,
        created: false,
      });
      const conflictingContextualIssue = await runCli(
        configDirectory,
        "--json",
        "--idempotency-key",
        contextualIssueKey,
        "issue",
        "create",
        `${contextualIssueTitle} but changed`,
      );
      expect(conflictingContextualIssue.exitCode).toBe(1);
      expect(conflictingContextualIssue.stdout).toBe("");
      expect(conflictingContextualIssue.stderr).toContain(
        "idempotency key was already used for a different issue",
      );

      const contextAfterProject = await runCli(configDirectory, "--json", "context", "current");
      expect(contextAfterProject.exitCode, contextAfterProject.stderr).toBe(0);
      expect(JSON.parse(contextAfterProject.stdout)).toMatchObject({
        profile: "default",
        project: createdProject.project.id,
      });

      const activeIssue = await eventually<JsonRecord>(
        async () => {
          const response = await systemClient.json(
            `/v1/issues/authorization-requests/${encodeURIComponent(requestEventId)}`,
            { expectedStatus: [200, 202] },
          );
          return response.status === 200
            ? record(response.body.issue, "authorized CLI issue")
            : undefined;
        },
        { description: `authorization for CLI issue ${issueKey}` },
      );
      expect(activeIssue).toMatchObject({ id: issueId, key: issueKey, title: contextualIssueTitle });

      const knowledgeTitle = uniqueName("How we ship safely");
      const contextualPage = await runCli(
        configDirectory,
        "--json",
        "--idempotency-key",
        `cli-context-page-${randomUUID()}`,
        "doc",
        "page",
        "create",
        "--title",
        knowledgeTitle,
        "--template",
        "product-spec",
      );
      expect(contextualPage.exitCode, contextualPage.stderr).toBe(0);
      const pageEnvelope = record(JSON.parse(contextualPage.stdout), "CLI knowledge page");
      const knowledgePage = record(pageEnvelope.page, "CLI knowledge document");
      const knowledgePageId = string(knowledgePage.id, "CLI knowledge page id");
      expect(pageEnvelope).toMatchObject({ created: true, durable: true });
      expect(knowledgePage).toMatchObject({ title: knowledgeTitle, visibility: "team" });

      const chatChannel = `delivery-${randomUUID().replaceAll("-", "").slice(0, 8)}`;
      const chatTopic = uniqueName("Release coordination");
      const contextualConversation = await runCli(
        configDirectory,
        "--json",
        "--idempotency-key",
        `cli-context-chat-${randomUUID()}`,
        "chat",
        "create",
        chatChannel,
        "--topic",
        chatTopic,
      );
      expect(contextualConversation.exitCode, contextualConversation.stderr).toBe(0);
      const conversationEnvelope = record(
        JSON.parse(contextualConversation.stdout),
        "CLI Chat conversation",
      );
      const conversation = record(conversationEnvelope.conversation, "CLI Chat channel");
      const conversationId = string(conversation.id, "CLI Chat conversation id");
      expect(conversationEnvelope).toMatchObject({ durable: true });
      expect(conversation).toMatchObject({
        channel: chatChannel,
        topic: chatTopic,
        ref: `myelin://${systemTestConfig.tenant}/chat/channel/${conversationId}`,
      });

      const chatMessage = uniqueName("The release train is ready");
      const sentMessage = await runCli(
        configDirectory,
        "--json",
        "--idempotency-key",
        `cli-context-message-${randomUUID()}`,
        "chat",
        "send",
        conversationId,
        chatMessage,
      );
      expect(sentMessage.exitCode, sentMessage.stderr).toBe(0);
      expect(JSON.parse(sentMessage.stdout)).toMatchObject({ durable: true });

      const sourceRepository = new GitProject(uniqueName("agent-context"), systemClient);
      expect(await sourceRepository.create()).toMatchObject({ durable: true });
      const sourceMarker = `credentialless_release_${randomUUID().replaceAll("-", "")}`;
      const sourcePath = "src/release.ts";
      const sourceContents = [
        `export const releaseMarker = "${sourceMarker}";`,
        "export const providerCredentialsRequired = false;",
        "",
      ].join("\n");
      await sourceRepository.writeFile("main", sourcePath, sourceContents);
      const agentBranch = `agent/investigate-${randomUUID().replaceAll("-", "").slice(0, 8)}`;
      const proposedSourceContents = sourceContents.replace(
        "providerCredentialsRequired = false",
        "providerCredentialsRequired = false as const",
      );

      // Once resumed, the collaborator reads the founder's issue, product spec, release room, and
      // source through Myelin itself. The run credential and the founder's live permissions
      // intersect at Edge; no GitHub, Linear, Notion, or Slack token, copied browser session,
      // tenant selector, or provider setup reaches the agent.
      const workAfterResume = await systemClient.json(
        `/v1/agents/${activated.agent.id}/runs`,
        {
          method: "POST",
          body: {},
          token: browserSession,
          tokenScheme: "session",
          idempotencyKey: `cli-agent-after-resume-${randomUUID()}`,
          expectedStatus: 201,
        },
      );
      const resumedRun = workAfterResume.body as unknown as AgentRunEnvelope;
      const issuePage = await askAgent(resumedRun, 3, "issues.list", {
        key: createdProject.project.issue_prefix,
        limit: 10,
      });
      expect(array(issuePage.items, "agent-visible issues")).toEqual(
        expect.arrayContaining([
          expect.objectContaining({
            id: issueId,
            key: issueKey,
            title: contextualIssueTitle,
            ref: `myelin://${systemTestConfig.tenant}/issue/issue/${issueKey}`,
          }),
        ]),
      );
      expect(issuePage.page).toMatchObject({ limit: 10 });

      const viewedIssue = await askAgent(resumedRun, 4, "issues.view", { issue_id: issueId });
      expect(viewedIssue).toMatchObject({
        id: issueId,
        key: issueKey,
        title: contextualIssueTitle,
        project_id: createdProject.project.id,
        ref: `myelin://${systemTestConfig.tenant}/issue/issue/${issueKey}`,
      });

      const pageList = await askAgent(resumedRun, 5, "knowledge.list_pages", { limit: 10 });
      expect(array(pageList.items, "agent-visible knowledge pages")).toEqual(
        expect.arrayContaining([
          expect.objectContaining({
            id: knowledgePageId,
            title: knowledgeTitle,
            ref: `myelin://${systemTestConfig.tenant}/knowledge/page/${knowledgePageId}`,
          }),
        ]),
      );
      expect(pageList.page).toMatchObject({ limit: 10 });

      const readPage = await askAgent(resumedRun, 6, "knowledge.read_page", {
        page_id: knowledgePageId,
      });
      expect(readPage).toMatchObject({
        id: knowledgePageId,
        title: knowledgeTitle,
        ref: `myelin://${systemTestConfig.tenant}/knowledge/page/${knowledgePageId}`,
      });
      expect(array(readPage.blocks, "agent-visible knowledge blocks")).toEqual(
        expect.arrayContaining([
          expect.objectContaining({ type: "heading", markdown: "Problem", state: "active" }),
        ]),
      );

      const conversations = await askAgent(resumedRun, 7, "chat.list_conversations", {
        limit: 10,
      });
      expect(array(conversations.items, "agent-visible Chat conversations")).toEqual(
        expect.arrayContaining([
          expect.objectContaining({
            id: conversationId,
            channel: chatChannel,
            topic: chatTopic,
            ref: `myelin://${systemTestConfig.tenant}/chat/channel/${conversationId}`,
          }),
        ]),
      );
      expect(conversations.page).toMatchObject({ limit: 10 });

      const chatHistory = await askAgent(resumedRun, 8, "chat.read_messages", {
        conversation_id: conversationId,
        limit: 10,
      });
      expect(chatHistory.conversation).toMatchObject({
        id: conversationId,
        channel: chatChannel,
        topic: chatTopic,
        ref: `myelin://${systemTestConfig.tenant}/chat/channel/${conversationId}`,
      });
      expect(array(chatHistory.items, "agent-visible Chat messages")).toEqual(
        expect.arrayContaining([
          expect.objectContaining({ content: chatMessage, state: "active" }),
        ]),
      );
      expect(chatHistory.page).toMatchObject({ limit: 10 });

      const agentRepositories = await askAgent(resumedRun, 9, "git.list_repositories", {
        limit: 100,
      });
      expect(array(agentRepositories.items, "agent-visible repositories")).toEqual(
        expect.arrayContaining([
          expect.objectContaining({
            state: "populated",
            slug: `${systemTestConfig.tenant}/${sourceRepository.slug}`,
          }),
        ]),
      );
      expect(agentRepositories.page).toMatchObject({ limit: 100 });

      const codeSearch = await askAgent(resumedRun, 10, "git.search_code", {
        query: sourceMarker,
        repo: sourceRepository.slug,
      });
      expect(array(codeSearch.items, "agent-visible code search results")).toEqual([
        expect.objectContaining({
          repo: sourceRepository.slug,
          ref: "refs/heads/main",
          path: sourcePath,
          excerpt: expect.stringContaining(sourceMarker),
        }),
      ]);
      expect(codeSearch).toMatchObject({ complete: true });

      const sourceFile = await askAgent(resumedRun, 11, "git.read_file", {
        repo: sourceRepository.slug,
        ref: "main",
        path: sourcePath,
      });
      expect(sourceFile).toMatchObject({
        path: sourcePath,
        contents: sourceContents,
        is_binary: false,
        is_truncated: false,
        preview_unavailable: false,
      });

      // The collaborator authors the proposed change through the governed Git surface. The
      // optimistic blob OID prevents lost updates, while start_ref creates a review branch without
      // granting the agent a reusable Git credential.
      const agentWriteKey = `agent-write-${randomUUID()}`;
      const writtenByAgent = await askAgentToAct(
        resumedRun,
        12,
        "git.write_file",
        {
          repo: sourceRepository.slug,
          ref: agentBranch,
          path: sourcePath,
          contents: proposedSourceContents,
          base_oid: string(sourceFile.base_oid, "agent-visible source blob OID"),
          start_ref: "main",
        },
        agentWriteKey,
      );
      const replayedAgentWrite = await askAgentToAct(
        resumedRun,
        13,
        "git.write_file",
        {
          repo: sourceRepository.slug,
          ref: agentBranch,
          path: sourcePath,
          contents: proposedSourceContents,
          base_oid: string(sourceFile.base_oid, "agent-visible source blob OID"),
          start_ref: "main",
        },
        agentWriteKey,
      );
      expect(replayedAgentWrite).toEqual(writtenByAgent);
      const agentWrite = record(writtenByAgent.data, "agent file-write receipt data");
      const agentBranchCommit = string(agentWrite.commit_oid, "agent branch commit OID");
      expect(agentBranchCommit).toMatch(/^[0-9a-f]{40}$/);
      expect(writtenByAgent.ref).toBe(
        `myelin://${systemTestConfig.tenant}/git/commit/${sourceRepository.slug}:${agentBranchCommit}`,
      );

      // Authorship and authorization stay separate: the PR is authored by the agent, while the
      // founder's live repository grant is the authorization basis. The agent receives neither a
      // copied Git credential nor a durable repository grant of its own.
      const agentPullRequestTitle = uniqueName("Make credentialless intent explicit");
      const agentPullRequestKey = `agent-pr-${randomUUID()}`;
      const openedByAgent = await askAgentToAct(
        resumedRun,
        14,
        "git.open_pr",
        {
          repo: sourceRepository.slug,
          title: agentPullRequestTitle,
          base_ref: "refs/heads/main",
          head_ref: `refs/heads/${agentBranch}`,
          head_oid: agentBranchCommit,
        },
        agentPullRequestKey,
      );
      const replayedAgentPullRequest = await askAgentToAct(
        resumedRun,
        15,
        "git.open_pr",
        {
          repo: sourceRepository.slug,
          title: agentPullRequestTitle,
          base_ref: "refs/heads/main",
          head_ref: `refs/heads/${agentBranch}`,
          head_oid: agentBranchCommit,
        },
        agentPullRequestKey,
      );
      expect(replayedAgentPullRequest).toEqual(openedByAgent);
      const agentPullRequestRef = string(openedByAgent.ref, "agent pull request ref");
      const agentPullRequestReceipt = record(
        openedByAgent.data,
        "agent pull request receipt data",
      );
      const agentPullRequestNumber = integer(
        agentPullRequestReceipt.number,
        "agent pull request number",
      );
      expect(agentPullRequestRef).toBe(
        `myelin://${systemTestConfig.tenant}/git/pr/${sourceRepository.slug}:${agentPullRequestNumber}`,
      );

      const humanVisibleAgentPullRequest = await eventually<JsonRecord>(
        async () => {
          const listed = await runCli(
            configDirectory,
            "--json",
            "git",
            "pr",
            "list",
            "--repo",
            sourceRepository.slug,
          );
          expect(listed.exitCode, listed.stderr).toBe(0);
          const matching = array(
            record(JSON.parse(listed.stdout), "human pull request list").items,
            "human-visible pull requests",
          )
            .map((pullRequest, index) =>
              record(pullRequest, `human-visible pull request ${index}`),
            )
            .filter((pullRequest) => pullRequest.title === agentPullRequestTitle);
          return matching.length === 1 ? matching[0] : undefined;
        },
        { description: "the agent-authored pull request to appear exactly once in the human CLI" },
      );
      expect(humanVisibleAgentPullRequest).toMatchObject({
        title: agentPullRequestTitle,
        head_ref: `refs/heads/${agentBranch}`,
        author_is_agent: true,
        author: `${activated.agent.principal_id}@${systemTestConfig.tenant}.noreply`,
      });
      expect(humanVisibleAgentPullRequest.number).toBe(agentPullRequestNumber);

      // The collaborator can now turn what it learned into durable team work. Project metadata
      // supplies the issue type and prefix, the human's live project access bounds the write, and
      // a lost MCP response can be retried without creating a second ticket.
      const agentIssueTitle = uniqueName("Investigate the credentialless release failure");
      const agentIssueKey = `agent-issue-${randomUUID()}`;
      const createdByAgent = await askAgentToAct(
        resumedRun,
        16,
        "issues.create",
        {
          project_id: createdProject.project.id,
          title: agentIssueTitle,
        },
        agentIssueKey,
      );
      const replayedAgentIssue = await askAgentToAct(
        resumedRun,
        17,
        "issues.create",
        {
          project_id: createdProject.project.id,
          title: agentIssueTitle,
        },
        agentIssueKey,
      );
      expect(replayedAgentIssue).toEqual(createdByAgent);
      const agentIssueRef = string(createdByAgent.ref, "agent-created issue ref");
      const agentIssueReceipt = record(createdByAgent.data, "agent-created issue receipt data");
      expect(agentIssueRef).toBe(
        `myelin://${systemTestConfig.tenant}/issue/issue/${string(
          agentIssueReceipt.key,
          "agent-created issue key",
        )}`,
      );

      const humanVisibleAgentIssue = await eventually<JsonRecord>(
        async () => {
          const listed = await runCli(
            configDirectory,
            "--json",
            "issue",
            "list",
            "--state",
            "all",
            "--limit",
            "100",
          );
          expect(listed.exitCode, listed.stderr).toBe(0);
          const matching = array(
            record(JSON.parse(listed.stdout), "human issue list").items,
            "human-visible issues",
          )
            .map((issue, index) => record(issue, `human-visible issue ${index}`))
            .filter((issue) => issue.title === agentIssueTitle);
          return matching.length === 1 ? matching[0] : undefined;
        },
        { description: "the agent-created issue to become visible exactly once in the human CLI" },
      );
      expect(humanVisibleAgentIssue).toMatchObject({
        project_id: createdProject.project.id,
        title: agentIssueTitle,
        created_by: activated.agent.principal_id,
        creator_kind: "agent",
        key: expect.stringMatching(new RegExp(`^${createdProject.project.issue_prefix}-\\d+$`)),
      });
      expect(agentIssueRef).toBe(
        `myelin://${systemTestConfig.tenant}/issue/issue/${string(
          humanVisibleAgentIssue.key,
          "human-visible agent issue key",
        )}`,
      );

      const agentChatMessage = `${uniqueName("Agent verified the release context")} \u{FFFC} \u{FFFC}`;
      const agentPostKey = `agent-chat-${randomUUID()}`;
      const postedByAgent = await askAgentToAct(
        resumedRun,
        18,
        "chat.post",
        {
          conversation_id: conversationId,
          content: agentChatMessage,
          references: [agentIssueRef, agentPullRequestRef],
        },
        agentPostKey,
      );
      const replayedAgentPost = await askAgentToAct(
        resumedRun,
        19,
        "chat.post",
        {
          conversation_id: conversationId,
          content: agentChatMessage,
          references: [agentIssueRef, agentPullRequestRef],
        },
        agentPostKey,
      );
      expect(replayedAgentPost).toEqual(postedByAgent);
      expect(postedByAgent.ref).toMatch(
        new RegExp(`^myelin://${systemTestConfig.tenant}/chat/message/[0-9A-Z]{26}$`),
      );

      const humanHistory = await runCli(
        configDirectory,
        "--json",
        "chat",
        "history",
        conversationId,
        "--limit",
        "100",
      );
      expect(humanHistory.exitCode, humanHistory.stderr).toBe(0);
      const humanVisibleMessages = array(
        record(JSON.parse(humanHistory.stdout), "human-visible Chat history").items,
        "human-visible Chat messages",
      ).map((message, index) => record(message, `human-visible Chat message ${index}`));
      expect(
        humanVisibleMessages.filter((message) => message.content === agentChatMessage),
      ).toEqual([
        expect.objectContaining({
          author_kind: "agent",
          is_you: false,
          state: "active",
          nodes: [
            expect.objectContaining({ kind: "artifact_ref" }),
            expect.objectContaining({ kind: "artifact_ref" }),
          ],
        }),
      ]);

      // Retirement is the irreversible counterpart: active work is torn down and neither the
      // CLI nor another client can quietly revive the durable identity.
      const retireAgent = await runCli(
        configDirectory,
        "--idempotency-key",
        `cli-agent-retire-${randomUUID()}`,
        "agent",
        "retire",
        activated.agent.id,
      );
      expect(retireAgent.exitCode, retireAgent.stderr).toBe(0);
      expect(retireAgent.stdout).toContain(`Retired agent: ${agentName}`);
      expect(retireAgent.stdout).toContain("stopped 1 active run");
      expect(retireAgent.stdout).toContain("cannot be resumed");
      await systemClient.json("/v1/whoami", {
        token: resumedRun.credential.token,
        tokenScheme: "agent",
        expectedStatus: 401,
      });

      const retiredAgent = await runCli(
        configDirectory,
        "--json",
        "agent",
        "show",
        activated.agent.id,
      );
      expect(retiredAgent.exitCode, retiredAgent.stderr).toBe(0);
      expect(JSON.parse(retiredAgent.stdout)).toMatchObject({
        agent: { id: activated.agent.id, status: "disabled" },
      });
      const reviveRetiredAgent = await runCli(
        configDirectory,
        "--idempotency-key",
        `cli-agent-revive-retired-${randomUUID()}`,
        "agent",
        "resume",
        activated.agent.id,
      );
      expect(reviveRetiredAgent.exitCode).toBe(1);
      expect(reviveRetiredAgent.stderr).toContain("retired agent cannot be resumed");

      // Migration uses that same browser-approved context: the file contains source records,
      // while project scope and credentials remain outside the export.
      const importJob = randomUUID();
      const importPath = resolve(configDirectory, "github-issues.json");
      await writeFile(
        importPath,
        JSON.stringify({
          records: [
            {
              source_id: uniqueName("github-acme-platform-41"),
              type_id: createdProject.project.default_issue_type_id,
              prefix: createdProject.project.issue_prefix,
              title: uniqueName("Imported through the browser session"),
            },
          ],
        }),
      );
      const importPreview = await runCli(
        configDirectory,
        "issue",
        "import",
        "--from",
        "github",
        "--job",
        importJob,
        "--input",
        importPath,
        "--dry-run",
      );
      expect(importPreview.exitCode, importPreview.stderr).toBe(0);
      expect(importPreview.stdout).toContain("1/1 ready");
      expect(importPreview.stdout).toContain("no data written");

      const imported = await runCli(
        configDirectory,
        "--json",
        "--idempotency-key",
        uniqueName("cli-import-run"),
        "issue",
        "import",
        "--from",
        "github",
        "--job",
        importJob,
        "--input",
        importPath,
        "--run",
      );
      expect(imported.exitCode, imported.stderr).toBe(0);
      expect(JSON.parse(imported.stdout)).toMatchObject({
        import: { job_id: importJob, source: "github", resumable: true },
        summary: { received: 1, created: 1, resumed: 0 },
      });

      const resumedImport = await runCli(
        configDirectory,
        "--idempotency-key",
        uniqueName("cli-import-resume"),
        "issue",
        "import",
        "--from",
        "github",
        "--job",
        importJob,
        "--input",
        importPath,
        "--run",
        "--resume",
      );
      expect(resumedImport.exitCode, resumedImport.stderr).toBe(0);
      expect(resumedImport.stdout).toContain("0 created, 1 resumed");

      const repositories = await runCli(configDirectory, "repo", "list");
      expect(repositories.exitCode, repositories.stderr).toBe(0);

      const configureGit = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["auth", "configure-git"],
      );
      expect(configureGit.exitCode, configureGit.stderr).toBe(0);
      expect(configureGit.stdout).toContain("Git is ready");
      expect(configureGit.stdout).not.toContain(systemTestConfig.token);

      // One Edge gets one Myelin helper. Reconfiguring replaces the old profile binding instead
      // of letting Git accept whichever helper happens to answer first.
      const edgeOrigin = new URL(systemTestConfig.edgeUrl).origin;
      const helpers = await git(
        ["config", "--global", "--get-all", `credential.${edgeOrigin}.helper`],
        { environment: gitEnvironment },
      );
      expect(helpers.stdout.trim().split("\n")).toHaveLength(1);
      expect(helpers.stdout).toContain("--profile 'default' auth git-credential");

      const edge = new URL(systemTestConfig.edgeUrl);
      const credential = await askGitForCredential(
        configDirectory,
        gitConfig,
        `protocol=${edge.protocol.slice(0, -1)}\nhost=${edge.host}\npath=${systemTestConfig.tenant}/${systemTestConfig.region}/repo.git\n\n`,
      );
      expect(credential.exitCode, credential.stderr).toBe(0);
      expect(credential.stdout).toContain("username=myelin-session");
      const gitPassword = credential.stdout.match(/^password=(.+)$/m)?.[1];
      expect(gitPassword).toBeTruthy();
      expect(gitPassword).not.toBe(systemTestConfig.token);

      const project = new GitProject(uniqueName("cli-session-wire"), systemClient);
      await project.create();
      const remote = await git(["ls-remote", gitRepositoryUrl(project.slug)], {
        environment: cliEnvironment(configDirectory, {
          ...gitEnvironment,
          GIT_TERMINAL_PROMPT: "0",
        }),
      });
      expect(remote.stdout).toBe("");
      expect(remote.stderr).not.toContain(String(gitPassword));

      const stranger = await runCliWith(
        configDirectory,
        {
          input: "protocol=https\nhost=not-myelin.example\npath=stolen.git\n\n",
        },
        ["auth", "git-credential", "get"],
      );
      expect(stranger.exitCode, stranger.stderr).toBe(0);
      expect(stranger.stdout).toBe("");

      // Once browser approval reaches its deadline, neither the CLI nor its Git helper pretends
      // that the old session is still useful—and neither needs to send the secret to discover it.
      await writeFile(configPath, expireProfile(await readFile(configPath, "utf8"), "default"));
      const afterExpiry = await runCli(configDirectory, "auth", "status");
      expect(afterExpiry.exitCode).toBe(3);
      expect(afterExpiry.stderr).toContain("saved CLI session has expired");
      expect(afterExpiry.stderr).not.toContain(String(gitPassword));

      const gitAfterExpiry = await runCliWith(
        configDirectory,
        {
          input: `protocol=${edge.protocol.slice(0, -1)}\nhost=${edge.host}\npath=acme/eu/repo.git\n\n`,
        },
        ["auth", "git-credential", "get"],
      );
      expect(gitAfterExpiry.exitCode).toBe(3);
      expect(gitAfterExpiry.stdout).toBe("");
      expect(gitAfterExpiry.stderr).toContain("saved CLI session has expired");
      expect(gitAfterExpiry.stderr).not.toContain(String(gitPassword));

      const unconfigureGit = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["auth", "unconfigure-git"],
      );
      expect(unconfigureGit.exitCode, unconfigureGit.stderr).toBe(0);
      expect(unconfigureGit.stdout).toContain("Removed the Myelin credential helper");

      const logout = await runCli(configDirectory, "auth", "logout");
      expect(logout.exitCode, logout.stderr).toBe(0);
      expect(logout.stdout).toContain("Removed the selected profile's stored credential");

      // Removing the active context falls through to the other saved identity without losing it.
      const reviewerAfterLogout = await runCli(configDirectory, "auth", "status");
      expect(reviewerAfterLogout.exitCode, reviewerAfterLogout.stderr).toBe(0);
      expect(reviewerAfterLogout.stdout).toContain(systemTestConfig.reviewerPrincipal);

      const finalLogout = await runCli(configDirectory, "auth", "logout");
      expect(finalLogout.exitCode, finalLogout.stderr).toBe(0);
      expect(await readdir(resolve(configDirectory, ".test-credentials"))).toEqual([]);
      await expect(stat(configPath)).rejects.toMatchObject({ code: "ENOENT" });

      const afterLogout = await runCli(configDirectory, "auth", "status");
      expect(afterLogout.exitCode).toBe(3);
      expect(afterLogout.stderr).toContain("run `myelin auth login`");
    } finally {
      if (login && login.exitCode === null) login.kill("SIGTERM");
      await rm(configDirectory, { recursive: true, force: true });
    }
  }, 120_000);
});
