import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { gitRepositoryUrl, systemTestConfig } from "../src/config.js";
import { eventually } from "../src/eventually.js";
import { awaitAuthorizedIssue } from "../src/issues.js";
import { git } from "../src/git-cli.js";
import { GitProject } from "../src/git-project.js";
import {
  askAgent,
  askAgentToAct,
  askAgentToBeDenied,
  askAgentToRequestApproval,
  type ActivatedAgentEnvelope,
  type AgentRunEnvelope,
} from "../src/journeys/agents.js";
import { awaitActiveIssue } from "../src/journeys/issues.js";
import { array, integer, record, string, type JsonRecord } from "../src/json.js";
import {
  cliEnvironment,
  finish,
  runCli,
  runCliWith,
  startCli,
  waitForCode,
} from "../src/myelin-cli.js";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

type CreatedProjectEnvelope = {
  project: {
    id: string;
    ref: string;
    name: string;
    issue_prefix: string;
    default_issue_type_id: string;
  };
  created: boolean;
};

function gitBlobOid(contents: string): string {
  const bytes = Buffer.from(contents);
  return createHash("sha1")
    .update(`blob ${bytes.length}\0`)
    .update(bytes)
    .digest("hex");
}

async function findThroughEveryCliPage(
  configDirectory: string,
  resource: "agent" | "automation",
  id: string,
): Promise<JsonRecord> {
  let cursor: string | undefined;
  const visited = new Set<string>();
  for (;;) {
    const args = ["--json", resource, "list", "--limit", "100"];
    if (cursor) args.push("--cursor", cursor);
    const listed = await runCli(configDirectory, ...args);
    expect(listed.exitCode, listed.stderr).toBe(0);
    const envelope = record(JSON.parse(listed.stdout), `${resource} roster page`);
    const item = array(envelope.items, `${resource} roster items`)
      .map((value, index) => record(value, `${resource} roster item ${index}`))
      .find((value) => value.id === id);
    if (item) return item;

    const next = record(envelope.page, `${resource} roster paging`).next_cursor;
    if (next === null) throw new Error(`${resource} ${id} was absent after walking every roster page`);
    cursor = string(next, `${resource} roster cursor`);
    if (visited.has(cursor)) throw new Error(`${resource} roster repeated cursor ${cursor}`);
    visited.add(cursor);
  }
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

// One journey, staged. The stages share hoisted state and MUST run in order -
// each one picks up exactly where the narrative left off, the way a real
// developer's session does. A failure in one stage fails the stages after it;
// the first red stage is the signal.
describe.sequential("the CLI authentication journey", () => {
  let configDirectory: string;
  let configPath: string;
  let gitConfig: string;
  let gitEnvironment: Record<string, string>;
  let login: ChildProcessWithoutNullStreams | undefined;
  let defaultProfile: string;
  let agentName: string;
  let activated: ActivatedAgentEnvelope;
  let browserSession: string;
  let createdProject: CreatedProjectEnvelope;
  let cliProjectName: string;
  let contextualIssueTitle: string;
  let issueId: string;
  let issueKey: string;
  let contextualIssueRef: string;
  let knowledgeTitle: string;
  let knowledgePageId: string;
  let knowledgePageRef: string;
  let chatChannel: string;
  let chatTopic: string;
  let conversationId: string;
  let chatMessage: string;
  let sourceRepository: GitProject;
  let sourceMarker: string;
  let sourcePath: string;
  let sourceContents: string;
  let proposedSourceContents: string;
  let agentBranch: string;
  let protectedAgentBranch: string;
  let resumedRun: AgentRunEnvelope;
  let sourceFile: JsonRecord;
  let agentVisiblePageRef: string;
  let agentPullRequestRef: string;
  let agentPullRequestTitle: string;
  let agentIssueRef: string;
  let agentIssueTitle: string;

  beforeAll(async () => {
    configDirectory = await mkdtemp(resolve(tmpdir(), "myelin-cli-system-"));
    configPath = resolve(configDirectory, "config.toml");
    gitConfig = resolve(configDirectory, "gitconfig");
    gitEnvironment = { GIT_CONFIG_GLOBAL: gitConfig, GIT_CONFIG_NOSYSTEM: "1" };
  });

  afterAll(async () => {
    if (login && login.exitCode === null) login.kill("SIGTERM");
    await rm(configDirectory, { recursive: true, force: true });
  });

  test("a developer approves in the browser once, then works across named contexts without copying a key", async () => {
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

    const firstConfig = await readFile(configPath, "utf8");
    defaultProfile = profileSection(firstConfig, "default");
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
  }, 120_000);

  test("one browser session explains the platform's shared tool vocabulary", async () => {
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
        "issues.close",
        "issues.create",
        "issues.list",
        "issues.view",
        "knowledge.link_work",
        "knowledge.list_pages",
        "knowledge.read_page",
        "projects.list",
        "chat.list_conversations",
        "chat.post",
        "chat.read_messages",
      ]),
    );
    expect(mcpManifest.tools.every((tool) => tool.inputSchema.type === "object")).toBe(true);
    expect(mcpDescription.stdout).not.toContain(systemTestConfig.token);
  }, 60_000);

  test("activating an external collaborator mints an identity, never a credential", async () => {
    // A human activates an external collaborator by choosing from that same vocabulary.
    // Retrying creates neither a second identity nor a long-lived credential to distribute.
    agentName = uniqueName("Review companion");
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
      "issues.close",
      "--tool",
      "issues.create",
      "--tool",
      "issues.list",
      "--tool",
      "issues.view",
      "--tool",
      "knowledge.link_work",
      "--tool",
      "knowledge.list_pages",
      "--tool",
      "knowledge.read_page",
      "--tool",
      "projects.list",
      "--tool",
      "chat.list_conversations",
      "--tool",
      "chat.post",
      "--tool",
      "chat.read_messages",
    );
    expect(createAgent.exitCode, createAgent.stderr).toBe(0);
    activated = JSON.parse(createAgent.stdout) as ActivatedAgentEnvelope;
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
          { name: "issues.close", version: 1 },
          { name: "issues.create", version: 2 },
          { name: "issues.list", version: 1 },
          { name: "issues.view", version: 2 },
          { name: "knowledge.link_work", version: 2 },
          { name: "knowledge.list_pages", version: 1 },
          { name: "knowledge.read_page", version: 2 },
          { name: "projects.list", version: 1 },
        ],
        grants: expect.arrayContaining([
          "agent.tools.read",
          "chat.read",
          "chat.post",
          "edge.identity.read",
          "issue.create",
          "issue.transition",
          "issue.view",
          "knowledge.edit",
          "knowledge.read",
          "project.view",
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
        "issues.close",
        "issues.create",
        "issues.list",
        "issues.view",
        "knowledge.link_work",
        "knowledge.list_pages",
        "knowledge.read_page",
        "projects.list",
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
      "issues.close",
      "--tool",
      "issues.create",
      "--tool",
      "issues.list",
      "--tool",
      "issues.view",
      "--tool",
      "knowledge.link_work",
      "--tool",
      "knowledge.list_pages",
      "--tool",
      "knowledge.read_page",
      "--tool",
      "projects.list",
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

    const newestAgent = await runCli(configDirectory, "agent", "list", "--limit", "1");
    expect(newestAgent.exitCode, newestAgent.stderr).toBe(0);
    expect(newestAgent.stdout).toContain(agentName);
    expect(newestAgent.stdout).toContain(activated.agent.ref);

    expect(await findThroughEveryCliPage(configDirectory, "agent", activated.agent.id))
      .toMatchObject({
        id: activated.agent.id,
        ref: activated.agent.ref,
        name: agentName,
      });

    const showAgent = await runCli(configDirectory, "agent", "show", activated.agent.id);
    expect(showAgent.exitCode, showAgent.stderr).toBe(0);
    expect(showAgent.stdout).toContain(`Agent: ${agentName}`);
    expect(showAgent.stdout).toContain(activated.agent.ref);
    expect(showAgent.stdout).toContain("no long-lived API key was created");
    expect(showAgent.stdout).not.toContain(systemTestConfig.token);
  }, 120_000);

  test("hosted agents and automations are ordinary product configuration", async () => {
    // Hosted work is ordinary product configuration too: the developer names an agent and
    // the event that should wake it. There is no GitHub app key, issue-tracker key, runner
    // credential, or copied bearer hidden between these two commands.
    const hostedAgentName = uniqueName("Mainline triage");
    const createHostedAgent = await runCli(
      configDirectory,
      "--json",
      "--idempotency-key",
      `cli-hosted-agent-${randomUUID()}`,
      "agent",
      "create",
      hostedAgentName,
      "--runtime",
      "hosted",
      "--tool",
      "ci.read_run",
      "--tool",
      "issues.create",
    );
    expect(createHostedAgent.exitCode, createHostedAgent.stderr).toBe(0);
    const hostedAgent = record(
      record(JSON.parse(createHostedAgent.stdout), "hosted agent activation").agent,
      "hosted agent",
    );
    const hostedAgentId = string(hostedAgent.id, "hosted agent id");
    expect(hostedAgent).toMatchObject({
      name: hostedAgentName,
      runtime_ref: "hosted:luna",
      status: "active",
    });

    const automationKey = `cli-automation-${randomUUID()}`;
    const automationTask = "Read the failed mainline run and open one focused issue.";
    const createAutomation = await runCli(
      configDirectory,
      "--json",
      "--idempotency-key",
      automationKey,
      "automation",
      "create",
      "--event",
      "ci.run.failed",
      "--repo",
      "core",
      "--branch",
      "main",
      "--run-as",
      hostedAgentId,
      "--task",
      automationTask,
      "--budget-minor-units",
      "250000",
      "--max-firings",
      "10",
      "--max-causal-depth",
      "4",
    );
    expect(createAutomation.exitCode, createAutomation.stderr).toBe(0);
    const automationEnvelope = record(
      JSON.parse(createAutomation.stdout),
      "CLI automation creation",
    );
    const automation = record(automationEnvelope.trigger, "CLI automation");
    const automationId = string(automation.id, "CLI automation id");
    expect(automationEnvelope).toMatchObject({ created: true, durable: true });
    expect(automation).toMatchObject({
      run_as_agent_id: hostedAgentId,
      event_type: "ci.run.failed",
      condition:
        `event.type == 'ci.run.failed' AND ` +
        `payload.repo_ref == 'myelin://${systemTestConfig.tenant}/git/repo/core' AND ` +
        `payload.source_ref == 'refs/heads/main'`,
      delegation_caveats: ["repo:core"],
      task: automationTask,
      budget_minor_units: 250000,
      max_firings: 10,
      firings_used: 0,
      state: "active",
    });

    const replayAutomation = await runCli(
      configDirectory,
      "--json",
      "--idempotency-key",
      automationKey,
      "automation",
      "create",
      "--event",
      "ci.run.failed",
      "--repo",
      "core",
      "--branch",
      "main",
      "--run-as",
      hostedAgentId,
      "--task",
      automationTask,
      "--budget-minor-units",
      "250000",
      "--max-firings",
      "10",
      "--max-causal-depth",
      "4",
    );
    expect(replayAutomation.exitCode, replayAutomation.stderr).toBe(0);
    expect(JSON.parse(replayAutomation.stdout)).toMatchObject({
      created: false,
      durable: true,
      trigger: { id: automationId, run_as_agent_id: hostedAgentId },
    });

    expect(await findThroughEveryCliPage(configDirectory, "automation", automationId))
      .toMatchObject({ id: automationId });

    const showAutomation = await runCli(
      configDirectory,
      "automation",
      "show",
      automationId,
    );
    expect(showAutomation.exitCode, showAutomation.stderr).toBe(0);
    expect(showAutomation.stdout).toContain("Automation: ci.run.failed -> agent:");
    expect(showAutomation.stdout).toContain(automationTask);
    expect(showAutomation.stdout).toContain("Myelin owns the integration credentials");

    const emptyHistory = await runCli(
      configDirectory,
      "automation",
      "history",
      automationId,
    );
    expect(emptyHistory.exitCode, emptyHistory.stderr).toBe(0);
    expect(emptyHistory.stdout).toBe("(no items)\n");

    const pauseAutomation = await runCli(
      configDirectory,
      "--idempotency-key",
      `cli-automation-pause-${randomUUID()}`,
      "automation",
      "pause",
      automationId,
    );
    expect(pauseAutomation.exitCode, pauseAutomation.stderr).toBe(0);
    expect(pauseAutomation.stdout).toContain("Paused automation:");
    expect(pauseAutomation.stdout).toContain("will not reserve work until resumed");

    const resumeAutomation = await runCli(
      configDirectory,
      "--idempotency-key",
      `cli-automation-resume-${randomUUID()}`,
      "automation",
      "resume",
      automationId,
    );
    expect(resumeAutomation.exitCode, resumeAutomation.stderr).toBe(0);
    expect(resumeAutomation.stdout).toContain("Resumed automation:");

    const disableAutomation = await runCli(
      configDirectory,
      "--idempotency-key",
      `cli-automation-disable-${randomUUID()}`,
      "automation",
      "disable",
      automationId,
    );
    expect(disableAutomation.exitCode, disableAutomation.stderr).toBe(0);
    expect(disableAutomation.stdout).toContain("Disabled automation:");
    expect(disableAutomation.stdout).toContain("cannot be resumed");

    const retireHostedAgent = await runCli(
      configDirectory,
      "--idempotency-key",
      `cli-hosted-agent-retire-${randomUUID()}`,
      "agent",
      "retire",
      hostedAgentId,
    );
    expect(retireHostedAgent.exitCode, retireHostedAgent.stderr).toBe(0);
    expect(retireHostedAgent.stdout).toContain(`Retired agent: ${hostedAgentName}`);
  }, 120_000);

  test("starting work exchanges the session for one minute of governed authority", async () => {
    // Starting work exchanges that browser session for one minute of agent authority. A lost
    // response is safe to retry: Edge returns the same run and credential, not a sibling run.
    const defaultCredentialRef = defaultProfile.match(
      /^credential_ref = "([A-Za-z0-9_-]{22})"$/m,
    )?.[1];
    expect(defaultCredentialRef).toBeTruthy();
    browserSession = await readFile(
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
          { name: "issues.close", version: 1 },
          { name: "issues.create", version: 2 },
          { name: "issues.list", version: 1 },
          { name: "issues.view", version: 2 },
          { name: "knowledge.link_work", version: 2 },
          { name: "knowledge.list_pages", version: 1 },
          { name: "knowledge.read_page", version: 2 },
          { name: "projects.list", version: 1 },
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

    // One transient credential opens one governed door. Even when the agent selected an Issues
    // or Chat tool, its bearer cannot walk around MCP and call that tool's ordinary REST twin.
    await systemClient.json("/v1/issues?limit=1", {
      token: running.credential.token,
      tokenScheme: "agent",
      expectedStatus: 403,
    });
    await systemClient.json("/v1/chat/conversations?limit=1", {
      token: running.credential.token,
      tokenScheme: "agent",
      expectedStatus: 403,
    });

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
      "issues.close",
      "issues.create",
      "issues.list",
      "issues.view",
      "knowledge.link_work",
      "knowledge.list_pages",
      "knowledge.read_page",
      "projects.list",
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
    expect(schemaFor("issues.close")).toMatchObject({
      type: "object",
      required: ["issue_ref"],
      additionalProperties: false,
    });
    expect(schemaFor("issues.create")).toMatchObject({
      type: "object",
      required: ["project_ref", "title"],
      additionalProperties: false,
    });
    expect(schemaFor("issues.list")).toMatchObject({
      type: "object",
      additionalProperties: false,
    });
    expect(schemaFor("issues.view")).toMatchObject({
      type: "object",
      required: ["issue_ref"],
      additionalProperties: false,
    });
    expect(schemaFor("knowledge.link_work")).toMatchObject({
      type: "object",
      required: ["page_ref", "reference"],
      additionalProperties: false,
    });
    expect(schemaFor("knowledge.list_pages")).toMatchObject({
      type: "object",
      additionalProperties: false,
    });
    expect(schemaFor("knowledge.read_page")).toMatchObject({
      type: "object",
      required: ["page_ref"],
      additionalProperties: false,
    });
    expect(schemaFor("projects.list")).toMatchObject({
      type: "object",
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
  }, 120_000);

  test("the ordinary CLI command is a complete MCP server configuration", async () => {
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
      "issues.close",
      "issues.create",
      "issues.list",
      "issues.view",
      "knowledge.link_work",
      "knowledge.list_pages",
      "knowledge.read_page",
      "projects.list",
    ]);
  }, 60_000);

  test("pausing kills in-flight authority and resume never resurrects it", async () => {
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
  }, 120_000);

  test("a founder builds working context: project, issue, spec, room, and source", async () => {
    // A founder names the first project once. Its generated identity becomes context,
    // so show and subsequent work no longer carry an operator-provided UUID.
    cliProjectName = uniqueName("Developer experience");
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
    createdProject = JSON.parse(createProject.stdout) as CreatedProjectEnvelope;
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
    contextualIssueTitle = uniqueName("Created from the active project");
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
    issueId = string(issueSummary.id, "CLI issue id");
    issueKey = string(issueSummary.key, "CLI issue key");
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

    const activeIssue = await awaitAuthorizedIssue(
      systemClient,
      requestEventId,
      `authorization for CLI issue ${issueKey}`,
    );
    expect(activeIssue).toMatchObject({ id: issueId, key: issueKey, title: contextualIssueTitle });
    contextualIssueRef = string(activeIssue.ref, "CLI issue reference");

    // Dependencies are ordinary CLI work, not an API-only graph feature. A developer can add one,
    // inspect the exact typed edge, and remove it again without learning an internal storage shape.
    const deliveryIssue = await awaitActiveIssue(
      systemClient,
      uniqueName("Deliver the contextual issue"),
      {
        projectId: createdProject.project.id,
        typeId: createdProject.project.default_issue_type_id,
        prefix: createdProject.project.issue_prefix,
      },
    );
    const deliveryRef = string(deliveryIssue.ref, "dependent issue reference");
    const relationKey = `cli-issue-relation-${randomUUID()}`;
    const addedRelation = await runCli(
      configDirectory,
      "--json",
      "--idempotency-key",
      relationKey,
      "issue",
      "relation",
      "add",
      issueKey,
      "blocks",
      deliveryRef,
    );
    expect(addedRelation.exitCode, addedRelation.stderr).toBe(0);
    const addedEnvelope = record(JSON.parse(addedRelation.stdout), "CLI issue relation receipt");
    const relation = record(addedEnvelope.relation, "CLI issue relation");
    const relationId = string(relation.id, "CLI issue relation id");
    expect(addedEnvelope).toMatchObject({ created: true, durable: true });
    expect(relation).toMatchObject({
      source_ref: contextualIssueRef,
      target_ref: deliveryRef,
      relation: "blocks",
    });

    const listedRelations = await runCli(
      configDirectory,
      "--json",
      "issue",
      "relation",
      "list",
      issueKey,
    );
    expect(listedRelations.exitCode, listedRelations.stderr).toBe(0);
    expect(array(JSON.parse(listedRelations.stdout).items, "CLI issue relations")).toEqual([
      expect.objectContaining({ id: relationId, target_ref: deliveryRef, relation: "blocks" }),
    ]);

    const removedRelation = await runCli(
      configDirectory,
      "--json",
      "issue",
      "relation",
      "remove",
      issueKey,
      relationId,
    );
    expect(removedRelation.exitCode, removedRelation.stderr).toBe(0);
    expect(JSON.parse(removedRelation.stdout)).toMatchObject({ removed: true, durable: true });

    const repeatedRemoval = await runCli(
      configDirectory,
      "--json",
      "issue",
      "relation",
      "remove",
      issueKey,
      relationId,
    );
    expect(repeatedRemoval.exitCode, repeatedRemoval.stderr).toBe(0);
    expect(JSON.parse(repeatedRemoval.stdout)).toMatchObject({
      relation_id: relationId,
      removed: false,
      durable: true,
    });

    knowledgeTitle = uniqueName("How we ship safely");
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
    knowledgePageId = string(knowledgePage.id, "CLI knowledge page id");
    knowledgePageRef = `myelin://${systemTestConfig.tenant}/knowledge/page/${knowledgePageId}`;
    expect(pageEnvelope).toMatchObject({ created: true, durable: true });
    expect(knowledgePage).toMatchObject({
      title: knowledgeTitle,
      visibility: "team",
      ref: knowledgePageRef,
    });

    // A developer can connect the living spec to delivery work without downloading and
    // replacing the document. The one retry identity names one block, so an ambiguous response
    // can be repeated safely and a changed payload is refused instead of becoming a second link.
    const humanKnowledgeLinkKey = `cli-context-link-${randomUUID()}`;
    const linkedFromCli = await runCli(
      configDirectory,
      "--json",
      "--idempotency-key",
      humanKnowledgeLinkKey,
      "doc",
      "page",
      "link",
      knowledgePageId,
      contextualIssueRef,
      "--note",
      "Delivery is tracked by",
    );
    expect(linkedFromCli.exitCode, linkedFromCli.stderr).toBe(0);
    const humanLinkReceipt = record(
      JSON.parse(linkedFromCli.stdout),
      "CLI Knowledge link receipt",
    );
    const humanLinkBlockRef = string(humanLinkReceipt.block_ref, "CLI Knowledge block ref");
    expect(humanLinkReceipt).toMatchObject({
      linked: true,
      durable: true,
      page_id: knowledgePageId,
      page_ref: knowledgePageRef,
      version: 2,
    });
    expect(humanLinkBlockRef).toMatch(
      new RegExp(`^${knowledgePageRef}#b[0-9A-HJKMNP-TV-Z]{26}$`),
    );

    const replayedCliLink = await runCli(
      configDirectory,
      "--json",
      "--idempotency-key",
      humanKnowledgeLinkKey,
      "doc",
      "page",
      "link",
      knowledgePageId,
      contextualIssueRef,
      "--note",
      "Delivery is tracked by",
    );
    expect(replayedCliLink.exitCode, replayedCliLink.stderr).toBe(0);
    expect(JSON.parse(replayedCliLink.stdout)).toEqual({
      ...humanLinkReceipt,
      linked: false,
    });

    const conflictingCliLink = await runCli(
      configDirectory,
      "--json",
      "--idempotency-key",
      humanKnowledgeLinkKey,
      "doc",
      "page",
      "link",
      knowledgePageId,
      contextualIssueRef,
      "--note",
      "The same key cannot silently change this note",
    );
    expect(conflictingCliLink.exitCode).toBe(1);
    expect(conflictingCliLink.stdout).toBe("");
    expect(conflictingCliLink.stderr).toContain(
      "idempotency key already identifies a different Knowledge link",
    );

    const pageAfterHumanLink = await runCli(
      configDirectory,
      "--json",
      "doc",
      "page",
      "get",
      knowledgePageId,
    );
    expect(pageAfterHumanLink.exitCode, pageAfterHumanLink.stderr).toBe(0);
    expect(
      array(
        record(
          record(JSON.parse(pageAfterHumanLink.stdout), "linked Knowledge envelope").page,
          "linked Knowledge page",
        ).blocks,
        "linked Knowledge blocks",
      ),
    ).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          markdown: "Delivery is tracked by \u{FFFC}",
          references: [contextualIssueRef],
          is_you: true,
        }),
      ]),
    );

    chatChannel = `delivery-${randomUUID().replaceAll("-", "").slice(0, 8)}`;
    chatTopic = uniqueName("Release coordination");
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
    conversationId = string(conversation.id, "CLI Chat conversation id");
    expect(conversationEnvelope).toMatchObject({ durable: true });
    expect(conversation).toMatchObject({
      channel: chatChannel,
      topic: chatTopic,
      ref: `myelin://${systemTestConfig.tenant}/chat/channel/${conversationId}`,
    });

    chatMessage = uniqueName("The release train is ready");
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

    sourceRepository = new GitProject(uniqueName("agent-context"), systemClient);

    // A typo must not be interpreted as a successful subset of a mutation. The exact same
    // repository name remains available after the rejected command, then succeeds once the
    // founder gives the complete grammar.
    const ambiguousRepositoryCreation = await runCli(
      configDirectory,
      "--idempotency-key",
      `cli-repo-ambiguous-${randomUUID()}`,
      "repo",
      "create",
      sourceRepository.slug,
      "second-slug",
    );
    expect(ambiguousRepositoryCreation.exitCode).toBe(2);
    expect(ambiguousRepositoryCreation.stdout).toBe("");
    expect(ambiguousRepositoryCreation.stderr).toContain("unknown git command token `second-slug`");
    await systemClient.json(sourceRepository.path, { expectedStatus: 404 });

    const createdRepository = await runCli(
      configDirectory,
      "--json",
      "--idempotency-key",
      `cli-repo-${randomUUID()}`,
      "repo",
      "create",
      sourceRepository.slug,
    );
    expect(createdRepository.exitCode, createdRepository.stderr).toBe(0);
    expect(JSON.parse(createdRepository.stdout)).toMatchObject({
      applied: { action: "git.repo.create", slug: sourceRepository.slug },
      created: true,
      durable: true,
    });

    const imaginaryAutoMerge = await runCli(
      configDirectory,
      "repo",
      "pr",
      "merge",
      sourceRepository.slug,
      "1",
      "--auto",
    );
    expect(imaginaryAutoMerge.exitCode).toBe(2);
    expect(imaginaryAutoMerge.stdout).toBe("");
    expect(imaginaryAutoMerge.stderr).toContain("unknown git command token `--auto`");

    sourceMarker = `credentialless_release_${randomUUID().replaceAll("-", "")}`;
    sourcePath = "src/release.ts";
    sourceContents = [
      `export const releaseMarker = "${sourceMarker}";`,
      "export const providerCredentialsRequired = false;",
      "",
    ].join("\n");
    await sourceRepository.writeFile("main", sourcePath, sourceContents);
    agentBranch = `agent/investigate-${randomUUID().replaceAll("-", "").slice(0, 8)}`;
    protectedAgentBranch = `protected/${randomUUID().replaceAll("-", "").slice(0, 8)}`;
    await systemClient.json(`${sourceRepository.path}/branch-protection`, {
      method: "POST",
      body: {
        rulesets: [{
          ref_pattern: `refs/heads/${protectedAgentBranch}`,
          required_contexts: [],
          required_approvals: 1,
          require_codeowner_review: false,
          require_conversation_resolution: false,
          allow_force_push: false,
        }],
      },
      expectedStatus: 200,
    });
    proposedSourceContents = sourceContents.replace(
      "providerCredentialsRequired = false",
      "providerCredentialsRequired = false as const",
    );
  }, 120_000);

  test("a resumed collaborator reads that context through Myelin alone", async () => {
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
    resumedRun = workAfterResume.body as unknown as AgentRunEnvelope;
    const issuePage = await askAgent(resumedRun, 3, "issues.list", {
      key: createdProject.project.issue_prefix,
      limit: 10,
    });
    const agentVisibleIssue = array(issuePage.items, "agent-visible issues")
      .map((issue, index) => record(issue, `agent-visible issue ${index}`))
      .find((issue) => issue.key === issueKey);
    expect(agentVisibleIssue).toMatchObject({
      id: issueId,
      key: issueKey,
      title: contextualIssueTitle,
      ref: contextualIssueRef,
    });
    expect(issuePage.page).toMatchObject({ limit: 10 });

    const agentVisibleIssueRef = string(agentVisibleIssue?.ref, "agent-visible issue ref");
    const viewedIssue = await askAgent(resumedRun, 4, "issues.view", {
      issue_ref: agentVisibleIssueRef,
    });
    expect(viewedIssue).toMatchObject({
      id: issueId,
      key: issueKey,
      title: contextualIssueTitle,
      project_id: createdProject.project.id,
      ref: `myelin://${systemTestConfig.tenant}/issue/issue/${issueKey}`,
    });

    const pageList = await askAgent(resumedRun, 5, "knowledge.list_pages", { limit: 10 });
    const agentVisiblePage = array(pageList.items, "agent-visible knowledge pages")
      .map((page, index) => record(page, `agent-visible knowledge page ${index}`))
      .find((page) => page.id === knowledgePageId);
    expect(agentVisiblePage).toMatchObject({
      id: knowledgePageId,
      title: knowledgeTitle,
      ref: knowledgePageRef,
    });
    expect(pageList.page).toMatchObject({ limit: 10 });

    agentVisiblePageRef = string(agentVisiblePage?.ref, "agent-visible Knowledge ref");
    const readPage = await askAgent(resumedRun, 6, "knowledge.read_page", {
      page_ref: agentVisiblePageRef,
    });
    expect(readPage).toMatchObject({
      id: knowledgePageId,
      title: knowledgeTitle,
      ref: agentVisiblePageRef,
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

    sourceFile = await askAgent(resumedRun, 11, "git.read_file", {
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
  }, 120_000);

  test("the collaborator authors reviewed work through the governed Git surface", async () => {
    // Protection is repository policy, not a convention attached only to `main`. A selected
    // write tool can propose work on an ordinary branch, but cannot sidestep a custom protected
    // ref through the convenient file-edit door.
    const protectedWrite = await askAgentToBeDenied(
      resumedRun,
      12,
      "git.write_file",
      {
        repo: sourceRepository.slug,
        ref: protectedAgentBranch,
        path: sourcePath,
        contents: proposedSourceContents,
        base_oid: string(sourceFile.base_oid, "agent-visible source blob OID"),
        start_ref: "main",
      },
    );
    expect(protectedWrite).toContain("branch protection refused");

    // A rejected write is absent, not merely unreferenced. Even a caller who knows the exact
    // Git object ID cannot recover rejected secret bytes through the object-addressed diff API.
    const rejectedSecret = ["AK", "IA", randomUUID().replaceAll("-", "")].join("");
    await askAgentToBeDenied(
      resumedRun,
      13,
      "git.write_file",
      {
        repo: sourceRepository.slug,
        ref: `agent/rejected-${randomUUID().replaceAll("-", "").slice(0, 8)}`,
        path: sourcePath,
        contents: rejectedSecret,
        base_oid: string(sourceFile.base_oid, "agent-visible source blob OID"),
        start_ref: "main",
      },
    );
    const rejectedObject = await systemClient.json(
      `${sourceRepository.path}/file-lines/${gitBlobOid(rejectedSecret)}`
        + `?path=${encodeURIComponent(sourcePath)}&start=1&end=1`,
      { expectedStatus: 200 },
    );
    expect(rejectedObject.body).toEqual({ lines: [] });
    const aliasedLine = await systemClient.json(
      `${sourceRepository.path}/file-lines/${gitBlobOid(rejectedSecret)}`
        + `?path=${encodeURIComponent(sourcePath)}&start=01&end=1`,
      { expectedStatus: 400 },
    );
    expect(aliasedLine.body).toMatchObject({ error: { code: "bad_request" } });

    // The collaborator authors the proposed change through the governed Git surface. The
    // optimistic blob OID prevents lost updates, while start_ref creates a review branch without
    // granting the agent a reusable Git credential.
    const agentWriteKey = `agent-write-${randomUUID()}`;
    const writtenByAgent = await askAgentToAct(
      resumedRun,
      14,
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
      15,
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
    agentPullRequestTitle = uniqueName("Make credentialless intent explicit");
    const agentPullRequestKey = `agent-pr-${randomUUID()}`;
    const openedByAgent = await askAgentToAct(
      resumedRun,
      16,
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
      17,
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
    agentPullRequestRef = string(openedByAgent.ref, "agent pull request ref");
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
  }, 120_000);

  test("agent-created work stays pseudonymous and approval-gated", async () => {
    // The collaborator can discover the founder's projects through the same permission boundary
    // instead of depending on an operator to hide a UUID in its prompt. That visible project
    // metadata supplies the issue type and prefix for durable team work.
    const projectPage = await askAgent(resumedRun, 18, "projects.list", { limit: 100 });
    const agentVisibleProject = array(projectPage.items, "agent-visible projects")
      .map((project, index) => record(project, `agent-visible project ${index}`))
      .find((project) => project.name === cliProjectName);
    expect(agentVisibleProject).toMatchObject({
      id: createdProject.project.id,
      ref: createdProject.project.ref,
      name: cliProjectName,
      issue_prefix: createdProject.project.issue_prefix,
      default_issue_type_id: createdProject.project.default_issue_type_id,
    });
    const agentVisibleProjectRef = string(
      agentVisibleProject?.ref,
      "agent-visible project ref",
    );

    // The human's live project access bounds the write, and a lost MCP response can be retried
    // without creating a second ticket.
    agentIssueTitle = uniqueName("Investigate the credentialless release failure");
    const agentIssueKey = `agent-issue-${randomUUID()}`;
    const createdByAgent = await askAgentToAct(
      resumedRun,
      19,
      "issues.create",
      {
        project_ref: agentVisibleProjectRef,
        title: agentIssueTitle,
      },
      agentIssueKey,
    );
    const replayedAgentIssue = await askAgentToAct(
      resumedRun,
      20,
      "issues.create",
      {
        project_ref: agentVisibleProjectRef,
        title: agentIssueTitle,
      },
      agentIssueKey,
    );
    expect(replayedAgentIssue).toEqual(createdByAgent);
    agentIssueRef = string(createdByAgent.ref, "agent-created issue ref");
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
      creator_kind: "agent",
      key: expect.stringMatching(new RegExp(`^${createdProject.project.issue_prefix}-\\d+$`)),
    });
    const publicIssueAuthor = string(
      humanVisibleAgentIssue.created_by,
      "human-visible agent issue author",
    );
    expect(publicIssueAuthor.startsWith("issue-author-")).toBe(true);
    expect(publicIssueAuthor.endsWith(`@${systemTestConfig.tenant}.noreply`)).toBe(true);
    expect(publicIssueAuthor).not.toContain(activated.agent.principal_id);
    expect(JSON.stringify(humanVisibleAgentIssue)).not.toContain(activated.agent.principal_id);
    expect(agentIssueRef).toBe(
      `myelin://${systemTestConfig.tenant}/issue/issue/${string(
        humanVisibleAgentIssue.key,
        "human-visible agent issue key",
      )}`,
    );

    // Closing shared work is consequential, so the collaborator can propose the exact effect
    // but cannot apply it. The creator sees one approval card addressed to the canonical issue;
    // another human cannot decide it, and the issue remains open until the creator approves.
    const agentIssueId = string(humanVisibleAgentIssue.id, "human-visible agent issue id");
    const agentIssueCloseKey = `agent-issue-close-${randomUUID()}`;
    const closeArguments = { issue_ref: agentIssueRef };
    const issueCloseGateId = await askAgentToRequestApproval(
      resumedRun,
      30,
      "issues.close",
      closeArguments,
      agentIssueCloseKey,
    );

    const issueBeforeApproval = await runCli(
      configDirectory,
      "--json",
      "issue",
      "view",
      agentIssueId,
    );
    expect(issueBeforeApproval.exitCode, issueBeforeApproval.stderr).toBe(0);
    expect(JSON.parse(issueBeforeApproval.stdout)).toMatchObject({
      id: agentIssueId,
      state_category: "unstarted",
    });

    const issueApprovalNotice = await eventually<JsonRecord>(
      async () => {
        const inbox = await systemClient.json("/v1/notif/inbox?view=all&limit=100", {
          token: browserSession,
          tokenScheme: "session",
        });
        return array(record(inbox.body, "founder inbox").items, "founder inbox items")
          .map((item, index) => record(item, `founder inbox item ${index}`))
          .find(
            (item) =>
              item.action !== null &&
              record(item.action, "approval action").gate_id === issueCloseGateId,
          );
      },
      { description: "the issue close approval to appear on the canonical issue" },
    );
    expect(issueApprovalNotice).toMatchObject({
      subject: agentIssueRef,
      action: {
        kind: "agent_effect_approval",
        gate_id: issueCloseGateId,
        run_id: resumedRun.run.id,
      },
    });

    await reviewerClient.json(
      `/v1/agent-approvals/${encodeURIComponent(issueCloseGateId)}/decision`,
      {
        method: "POST",
        body: { decision: "approve" },
        idempotencyKey: `reviewer-issue-close-${randomUUID()}`,
        expectedStatus: 403,
      },
    );
    const approvedIssueClose = await systemClient.json(
      `/v1/agent-approvals/${encodeURIComponent(issueCloseGateId)}/decision`,
      {
        method: "POST",
        body: { decision: "approve" },
        token: browserSession,
        tokenScheme: "session",
        idempotencyKey: `founder-issue-close-${randomUUID()}`,
        expectedStatus: 200,
      },
    );
    expect(approvedIssueClose.body).toMatchObject({
      gate_id: issueCloseGateId,
      run_id: resumedRun.run.id,
      state: "approved",
      changed: true,
    });

    const closedByAgent = await askAgentToAct(
      resumedRun,
      31,
      "issues.close",
      closeArguments,
      agentIssueCloseKey,
      issueCloseGateId,
    );
    const replayedAgentClose = await askAgentToAct(
      resumedRun,
      32,
      "issues.close",
      closeArguments,
      agentIssueCloseKey,
      issueCloseGateId,
    );
    expect(replayedAgentClose).toEqual(closedByAgent);
    expect(closedByAgent).toMatchObject({
      ref: agentIssueRef,
      data: {
        id: agentIssueId,
        key: humanVisibleAgentIssue.key,
        state: "Done",
        state_category: "completed",
      },
    });

    const issueAfterApproval = await runCli(
      configDirectory,
      "--json",
      "issue",
      "view",
      agentIssueId,
    );
    expect(issueAfterApproval.exitCode, issueAfterApproval.stderr).toBe(0);
    expect(JSON.parse(issueAfterApproval.stdout)).toMatchObject({
      id: agentIssueId,
      state: "Done",
      state_category: "completed",
      creator_kind: "agent",
      created_by: publicIssueAuthor,
    });
  }, 120_000);

  test("agent context enriches the living spec, the room, and the graph", async () => {
    // The collaborator can add context to the human-owned living spec through the same
    // governed surface. Myelin records an agent-authored encrypted block, while the founder's
    // live ownership remains the resource authority; neither identity is flattened into the
    // other and retrying the logical effect cannot append a duplicate.
    const agentKnowledgeLinkKey = `agent-knowledge-link-${randomUUID()}`;
    const linkedByAgent = await askAgentToAct(
      resumedRun,
      21,
      "knowledge.link_work",
      {
        page_ref: agentVisiblePageRef,
        reference: agentPullRequestRef,
        note: "Implementation is reviewed in",
      },
      agentKnowledgeLinkKey,
    );
    const replayedAgentKnowledgeLink = await askAgentToAct(
      resumedRun,
      22,
      "knowledge.link_work",
      {
        page_ref: agentVisiblePageRef,
        reference: agentPullRequestRef,
        note: "Implementation is reviewed in",
      },
      agentKnowledgeLinkKey,
    );
    const linkedByAgentData = record(
      linkedByAgent.data,
      "agent Knowledge link receipt data",
    );
    expect(replayedAgentKnowledgeLink).toEqual({
      ...linkedByAgent,
      data: { ...linkedByAgentData, linked: false },
    });
    expect(linkedByAgent.ref).toMatch(
      new RegExp(`^${knowledgePageRef}#b[0-9A-HJKMNP-TV-Z]{26}$`),
    );
    expect(linkedByAgentData).toMatchObject({
      page_id: knowledgePageId,
      page_ref: knowledgePageRef,
      version: 3,
      linked: true,
    });

    const pageAfterAgentLink = await runCli(
      configDirectory,
      "--json",
      "doc",
      "page",
      "get",
      knowledgePageId,
    );
    expect(pageAfterAgentLink.exitCode, pageAfterAgentLink.stderr).toBe(0);
    const collaborativeBlocks = array(
      record(
        record(JSON.parse(pageAfterAgentLink.stdout), "collaborative Knowledge envelope").page,
        "collaborative Knowledge page",
      ).blocks,
      "collaborative Knowledge blocks",
    ).map((block, index) => record(block, `collaborative Knowledge block ${index}`));
    expect(
      collaborativeBlocks.filter((block) =>
        array(block.references, "collaborative Knowledge references").includes(
          agentPullRequestRef,
        )
      ),
    ).toEqual([
      expect.objectContaining({
        markdown: "Implementation is reviewed in \u{FFFC}",
        references: [agentPullRequestRef],
        is_you: false,
      }),
    ]);

    const livingSpecContext = await eventually<JsonRecord>(
      async () => {
        const listed = await runCli(
          configDirectory,
          "--json",
          "ref",
          "links",
          knowledgePageRef,
        );
        expect(listed.exitCode, listed.stderr).toBe(0);
        const response = record(JSON.parse(listed.stdout), "living spec outgoing context");
        const targets = array(response.items, "living spec context targets")
          .map((item, index) => record(item, `living spec context target ${index}`));
        return targets.some((item) => item.target_root_ref === contextualIssueRef)
            && targets.some((item) => item.target_root_ref === agentPullRequestRef)
          ? response
          : undefined;
      },
      { description: "the living spec to lead to both human and agent delivery context" },
    );
    expect(array(livingSpecContext.items, "living spec outgoing context items")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          source_root_ref: knowledgePageRef,
          target_root_ref: contextualIssueRef,
          relation: "links",
        }),
        expect.objectContaining({
          source_root_ref: knowledgePageRef,
          target_root_ref: agentPullRequestRef,
          relation: "links",
        }),
      ]),
    );

    // Closing is a terminal-state target, not a create operation. Humans can express the intent
    // directly, and a repeated command observes the same durable issue without extra ceremony.
    const closedContextualIssue = await runCli(
      configDirectory,
      "--json",
      "issue",
      "close",
      issueKey,
    );
    expect(closedContextualIssue.exitCode, closedContextualIssue.stderr).toBe(0);
    expect(JSON.parse(closedContextualIssue.stdout)).toMatchObject({
      id: issueId,
      key: issueKey,
      state: "Done",
      state_category: "completed",
    });

    const repeatedClose = await runCli(
      configDirectory,
      "--json",
      "issue",
      "close",
      issueKey,
    );
    expect(repeatedClose.exitCode, repeatedClose.stderr).toBe(0);
    expect(JSON.parse(repeatedClose.stdout)).toEqual(JSON.parse(closedContextualIssue.stdout));

    const agentChatMessage = `${uniqueName("Agent verified the release context")} \u{FFFC} \u{FFFC}`;
    const agentPostKey = `agent-chat-${randomUUID()}`;
    const postedByAgent = await askAgentToAct(
      resumedRun,
      23,
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
      24,
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
    const renderedHumanHistory = await runCli(
      configDirectory,
      "chat",
      "history",
      conversationId,
      "--limit",
      "100",
    );
    expect(renderedHumanHistory.exitCode, renderedHumanHistory.stderr).toBe(0);
    expect(renderedHumanHistory.stdout).toContain(agentIssueTitle);
    expect(renderedHumanHistory.stdout).toContain(`<${agentIssueRef}>`);
    expect(renderedHumanHistory.stdout).toContain(agentPullRequestTitle);
    expect(renderedHumanHistory.stdout).toContain(`<${agentPullRequestRef}>`);

    // References are not merely syntax inside the message. Once projected, either artifact can
    // lead the developer back to the same visible piece of agent-authored context. The graph is
    // read through the CLI under the developer's session, so a hidden source could never appear
    // here just because its edge exists.
    for (const linkedArtifact of [agentIssueRef, agentPullRequestRef]) {
      const backlink = await eventually<JsonRecord>(
        async () => {
          const listed = await runCli(
            configDirectory,
            "--json",
            "ref",
            "backlinks",
            linkedArtifact,
          );
          expect(listed.exitCode, listed.stderr).toBe(0);
          const response = record(JSON.parse(listed.stdout), "permission-filtered backlinks");
          const links = array(
            response.items,
            "visible backlink items",
          )
            .map((item, index) => record(item, `visible backlink ${index}`))
            .filter((item) => item.root_ref === postedByAgent.ref);
          return links.length === 1 ? links[0] : undefined;
        },
        { description: `${linkedArtifact} to lead back to the agent's context message` },
      );
      expect(backlink).toMatchObject({
        root_ref: postedByAgent.ref,
        target_ref: linkedArtifact,
        relation: "links",
        relation_class: "reference",
      });
    }

    const outgoingContext = await eventually<JsonRecord>(
      async () => {
        const listed = await runCli(
          configDirectory,
          "--json",
          "ref",
          "links",
          string(postedByAgent.ref, "agent-authored context reference"),
        );
        expect(listed.exitCode, listed.stderr).toBe(0);
        const response = record(JSON.parse(listed.stdout), "permission-filtered outgoing links");
        const targets = array(response.items, "visible outgoing link items")
          .map((item, index) => record(item, `visible outgoing link ${index}`))
          .filter((item) => item.source_root_ref === postedByAgent.ref);
        return targets.length === 2 ? response : undefined;
      },
      { description: "the agent's context message to lead to both pieces of delivery work" },
    );
    expect(array(outgoingContext.items, "outgoing context targets")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ target_root_ref: agentIssueRef, relation: "links" }),
        expect.objectContaining({ target_root_ref: agentPullRequestRef, relation: "links" }),
      ]),
    );
  }, 120_000);

  test("the reference graph is not an existence oracle", async () => {
    // The target is gated too. Knowing (or guessing) a private canonical ref cannot turn the
    // graph into an existence oracle for another developer.
    const privatePage = await runCli(
      configDirectory,
      "--json",
      "--idempotency-key",
      `private-ref-target-${randomUUID()}`,
      "doc",
      "page",
      "create",
      "--title",
      uniqueName("Private release notes"),
      "--template",
      "blank",
      "--visibility",
      "private",
    );
    expect(privatePage.exitCode, privatePage.stderr).toBe(0);
    const privatePageRef = string(
      record(record(JSON.parse(privatePage.stdout), "private page envelope").page, "private page")
        .ref,
      "private page ref",
    );
    const ownerBacklinks = await runCli(
      configDirectory,
      "--json",
      "ref",
      "backlinks",
      privatePageRef,
    );
    expect(ownerBacklinks.exitCode, ownerBacklinks.stderr).toBe(0);
    expect(array(record(JSON.parse(ownerBacklinks.stdout), "owner backlinks").items)).toEqual([]);
    await reviewerClient.json(
      `/v1/refs/backlinks?ref=${encodeURIComponent(privatePageRef)}`,
      { expectedStatus: 404 },
    );
  }, 60_000);

  test("retirement is irreversible", async () => {
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
  }, 60_000);

  test("issue migration rides the same browser session", async () => {
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
    );
    expect(resumedImport.exitCode, resumedImport.stderr).toBe(0);
    expect(resumedImport.stdout).toContain("0 created, 1 resumed");
  }, 60_000);

  test("Git speaks Myelin through the credential helper until expiry and logout", async () => {
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
  }, 120_000);
});
