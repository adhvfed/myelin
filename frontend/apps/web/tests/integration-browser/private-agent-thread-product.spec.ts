import { randomBytes, randomUUID } from "node:crypto";
import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import type { APIRequestContext } from "@playwright/test";

import {
  browserApprovedSession,
  integrationEdgeUrl,
  integrationProjectId,
  integrationReviewerPrincipal,
  integrationReviewerToken,
  postProductJson,
  type JsonObject,
} from "./product-api";
import { signIn, waitForAppHydration } from "./session";

function ephemeralEd25519PublicKey(): string {
  const algorithm = Buffer.from("ssh-ed25519");
  const key = randomBytes(32);
  const length = (value: Buffer) => {
    const encoded = Buffer.alloc(4);
    encoded.writeUInt32BE(value.length);
    return encoded;
  };
  const blob = Buffer.concat([length(algorithm), algorithm, length(key), key]);
  return `ssh-ed25519 ${blob.toString("base64")}`;
}

async function commitWorkspaceReview(
  request: APIRequestContext,
  sessionToken: string,
  threadId: string,
  change: "initial" | "follow-up" | "hidden-credential" | "rewrite-hidden-credential" = "initial",
): Promise<void> {
  const envelope = await postProductJson(
    request,
    `/v1/agent-threads/${encodeURIComponent(threadId)}/runs`,
    {},
    { token: sessionToken, scheme: "session", status: 201 },
  );
  const run = envelope.run as JsonObject;
  const credential = envelope.credential as JsonObject;
  const runId = String(run.id);
  const runToken = String(credential.token);
  const executed = await request.post(
    `${integrationEdgeUrl}/v1/agent-runs/${encodeURIComponent(runId)}/mcp`,
    {
      headers: {
        authorization: `Bearer ${runToken}`,
        "x-myelin-token-scheme": "agent",
      },
      data: {
        jsonrpc: "2.0",
        id: 1,
        method: "tools/call",
        params: {
          name: "workspace.exec",
          arguments: {
            command: change === "initial"
              ? [
                  "git config user.name 'Myelin browser agent'",
                  "git config user.email 'browser-agent@myelin.invalid'",
                  "printf 'export const browserReview = true;\\n' > browser-review.ts",
                  "git add browser-review.ts",
                  "git commit -m 'Prepare browser cloud branch for review'",
                ].join(" && ")
              : change === "follow-up"
                ? [
                  "printf 'export const browserFollowUp = true;\\n' > browser-follow-up.ts",
                  "git add browser-follow-up.ts",
                  "git commit -m 'Follow up on browser cloud review'",
                ].join(" && ")
                : change === "hidden-credential"
                  ? [
                      "printf 'GITHUB_TOKEN=gh%saB3dE5fG7hJ9kL2mN4pQ6rS8tV0wX1yZ\\n' 'p_' > browser-credential.env",
                      "git add browser-credential.env",
                      "git commit -m 'Accidentally commit browser credential'",
                      "git rm browser-credential.env",
                      "git commit -m 'Delete browser credential from visible tree'",
                    ].join(" && ")
                  : "git reset --hard HEAD~2 && test ! -e browser-credential.env",
            timeout_seconds: 10,
          },
          _meta: { "com.myelin/idempotencyKey": `browser-review-${change}-${randomUUID()}` },
        },
      },
      failOnStatusCode: false,
    },
  );
  const executedText = await executed.text();
  expect(executed.status(), executedText).toBe(200);
  expect(JSON.parse(executedText)).toMatchObject({
    result: { isError: false, structuredContent: { data: { exit_code: 0 } } },
  });
  await postProductJson(
    request,
    `/v1/agent-runs/${encodeURIComponent(runId)}/close`,
    {},
    { token: runToken, scheme: "agent", status: 200 },
  );
}

test("an engineer keeps a cloud branch and its environment with one agent", async ({
  page,
  request,
}) => {
  const suffix = `${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const agentName = `checkout-companion-${suffix}`;
  const renamedAgentName = `checkout-continuity-companion-${suffix}`;
  const threadName = `Investigate checkout race ${suffix}`;
  const problem = `The final reader for ${suffix} still owns its checkout lease.`;
  const personalInstructions = `# My workflow\n\nKeep the ${suffix} diagnosis private until it is ready.`;
  const agentDefaults = `# Checkout companion defaults\n\nExplain the ${suffix} tradeoff first.`;
  const revisedAgentDefaults = `${agentDefaults}\nRun the focused browser journey.`;
  const agentContextPath = ".myelin/companion-defaults.md";
  const agentContextContents = `# Companion defaults\n\nTrace ${suffix} from its durable receipt.\n`;
  const revisedAgentContextContents = `${agentContextContents}Keep the reusable context concise.\n`;
  const contextFilePath = ".myelin/browser-context.md";
  const contextFileContents = `# Browser context\n\nKeep ${suffix} private until review.\n`;
  const repositorySlug = `cloud-workspace-${suffix}`;
  const malformedRepositorySlug = `malformed-cloud-workspace-${suffix}`;
  const sessionToken = await browserApprovedSession(request);
  await postProductJson(
    request,
    "/v1/git/repos",
    { slug: malformedRepositorySlug },
    { token: sessionToken, scheme: "session", status: 201 },
  );
  await postProductJson(
    request,
    `/v1/git/repos/${encodeURIComponent(malformedRepositorySlug)}/blob/main/fed.yaml`,
    { base_oid: "", contents: "services:\n  app: [unterminated\n" },
    { token: sessionToken, scheme: "session", status: 200 },
  );
  await postProductJson(
    request,
    "/v1/git/repos",
    { slug: repositorySlug },
    { token: sessionToken, scheme: "session", status: 201 },
  );
  await postProductJson(
    request,
    `/v1/git/repos/${encodeURIComponent(repositorySlug)}/blob/main/README.md`,
    { base_oid: "", contents: "# Browser-owned cloud workspace\n" },
    { token: sessionToken, scheme: "session", status: 200 },
  );
  await postProductJson(
    request,
    `/v1/git/repos/${encodeURIComponent(repositorySlug)}/blob/main/fed.yaml`,
    {
      base_oid: "",
      contents: "services:\n  app:\n    process: exec sleep 300\nentrypoint: app\n",
    },
    { token: sessionToken, scheme: "session", status: 200 },
  );

  await signIn(page);
  await page.getByRole("link", { name: "Agents" }).click();
  await page.waitForURL("**/agents");
  await page.getByRole("button", { name: "Activate an agent" }).click();
  const activation = page.getByRole("dialog", { name: "Activate named agent" });
  await expect(activation.getByRole("combobox", { name: "Runtime" })).toHaveValue("external");
  await activation.getByRole("textbox", { name: "Agent name" }).fill(agentName);
  await activation.getByRole("textbox", { name: "Default workspace instructions" })
    .fill(agentDefaults);
  await activation.getByRole("button", { name: "Add context file" }).click();
  await activation.getByRole("textbox", { name: "File 1 path" }).fill(agentContextPath);
  await activation.getByRole("textbox", { name: "File 1 contents" }).fill(agentContextContents);
  await activation.getByRole("checkbox", { name: "Run bounded commands" }).check();
  await activation.getByRole("button", { name: "Activate agent" }).click();

  const initialThreadDialog = page.getByRole("dialog", { name: "Start private agent thread" });
  await expect(initialThreadDialog).toBeVisible();
  await initialThreadDialog.getByRole("button", { name: "Cancel" }).click();

  await page.getByRole("button", { name: "Manage my agents" }).click();
  const roster = page.getByRole("dialog", { name: "My named agents" });
  const agentRow = roster.getByRole("listitem").filter({ hasText: agentName });
  await expect(agentRow).toContainText("Workspace defaults · revision 1");
  await expect(agentRow).toContainText("6 selected tools");
  await agentRow.getByRole("button", { name: "Rename" }).click();
  const rename = page.getByRole("dialog", { name: `Rename ${agentName}` });
  await expect(rename.getByRole("textbox", { name: "Agent name" })).toHaveValue(agentName);
  await rename.getByRole("textbox", { name: "Agent name" }).fill(renamedAgentName);
  await rename.getByRole("button", { name: "Rename agent" }).click();
  await expect(rename).toBeHidden();

  await page.getByRole("button", { name: "Manage my agents" }).click();
  const renamedRoster = page.getByRole("dialog", { name: "My named agents" });
  const renamedAgentRow = renamedRoster.getByRole("listitem").filter({ hasText: renamedAgentName });
  await expect(renamedAgentRow).toContainText("Workspace defaults · revision 1");
  await expect(renamedRoster.getByRole("listitem").filter({ hasText: agentName })).toHaveCount(0);
  await renamedAgentRow.getByRole("button", { name: "Tools" }).click();
  const tools = page.getByRole("dialog", { name: `${renamedAgentName} · tools` });
  await expect(tools.getByRole("checkbox", { name: "Run bounded commands" })).toBeChecked();
  await tools.getByRole("checkbox", { name: "Reply in threads" }).uncheck();
  await tools.getByRole("button", { name: "Save tools" }).click();
  await expect(tools).toBeHidden();

  await page.getByRole("button", { name: "Manage my agents" }).click();
  const revisedRoster = page.getByRole("dialog", { name: "My named agents" });
  const revisedAgentRow = revisedRoster.getByRole("listitem").filter({ hasText: renamedAgentName });
  await expect(revisedAgentRow).toContainText("5 selected tools");
  await revisedAgentRow.getByRole("button", { name: "Workspace" }).click();
  const defaults = page.getByRole("dialog", { name: `${renamedAgentName} · workspace defaults` });
  const defaultsEditor = defaults.getByRole("textbox", {
    name: "Default AGENTS.md instructions",
  });
  await expect(defaultsEditor).toHaveValue(agentDefaults);
  await expect(defaults.getByRole("textbox", { name: "File 1 path" }))
    .toHaveValue(agentContextPath);
  await defaults.getByRole("textbox", { name: "File 1 contents" })
    .fill(revisedAgentContextContents);
  await defaultsEditor.fill(revisedAgentDefaults);
  await defaults.getByRole("button", { name: "Save defaults" }).click();
  await expect(defaults).toBeHidden();

  await page.getByRole("button", { name: "New private thread" }).click();
  const malformedDialog = page.getByRole("dialog", { name: "Start private agent thread" });
  const malformedThreadName = `Diagnose invalid environment ${suffix}`;
  await malformedDialog.getByRole("textbox", { name: "Problem name" })
    .fill(malformedThreadName);
  await malformedDialog.getByRole("combobox", { name: "Agent" })
    .selectOption({ label: renamedAgentName });
  await malformedDialog.getByRole("textbox", { name: "Repository slug" })
    .fill(malformedRepositorySlug);
  await malformedDialog.getByRole("button", { name: "Start thread" }).click();
  await page.waitForURL(/\/agents\?thread=[0-9a-f-]{36}$/);
  await expect(page.getByRole("heading", { level: 2, name: malformedThreadName })).toBeVisible();
  await page.getByRole("button", { name: "Agent workspace" }).click();
  const malformedWorkspace = page.getByRole("dialog", { name: "Agent workspace" });
  await expect(malformedWorkspace.getByTestId("workspace-environment"))
    .toContainText("Service Federation · invalid");
  await expect(malformedWorkspace.getByTestId("workspace-environment-state"))
    .toContainText("Correct the source and start new work from that commit.");
  await page.keyboard.press("Escape");
  await expect(malformedWorkspace).toBeHidden();

  await page.getByRole("button", { name: "New private thread" }).click();
  const dialog = page.getByRole("dialog", { name: "Start private agent thread" });
  await dialog.getByRole("textbox", { name: "Problem name" }).fill(threadName);
  const agentChoice = dialog.getByRole("combobox", { name: "Agent" });
  await agentChoice.selectOption({ label: renamedAgentName });
  const agentId = await agentChoice.inputValue();
  expect(agentId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
  const reviewProject = dialog.getByRole("combobox", { name: "Project for eventual review" });
  await expect(reviewProject).toHaveValue("");
  await reviewProject.selectOption(integrationProjectId);
  await expect(dialog.getByText("This agent carries private workspace defaults at revision 2."))
    .toBeVisible();
  await dialog.getByRole("combobox", { name: "Keep workspace for" }).selectOption("3");
  await dialog.getByRole("textbox", { name: "Repository slug" }).fill(repositorySlug);
  await dialog.getByRole("textbox", { name: "Personal agent instructions" })
    .fill(personalInstructions);
  await dialog.getByRole("button", { name: "Add context file" }).click();
  await dialog.getByRole("textbox", { name: "File 1 path" }).fill(contextFilePath);
  await dialog.getByRole("textbox", { name: "File 1 contents" }).fill(contextFileContents);
  await dialog.getByRole("button", { name: "Start thread" }).click();

  await page.waitForURL(/\/agents\?thread=[0-9a-f-]{36}$/);
  await expect(page.getByRole("heading", { level: 2, name: threadName })).toBeVisible();
  await expect(page.getByText(`Private with ${renamedAgentName}`)).toBeVisible();
  await page.getByRole("button", { name: "Agent workspace" }).click();
  const workspace = page.getByRole("dialog", { name: "Agent workspace" });
  await expect(workspace.getByText("Generation 1", { exact: true })).toBeVisible();
  await expect(workspace.getByText(repositorySlug, { exact: true })).toBeVisible();
  await expect(workspace.getByTestId("workspace-environment"))
    .toContainText("Service Federation · declared");
  await expect(workspace.getByText("fed.yaml", { exact: true })).toBeVisible();
  await expect(workspace.getByTestId("workspace-personalization"))
    .toContainText("Declarative workspace overlay");
  await expect(workspace.getByTestId("workspace-environment-state"))
    .toContainText("pinned to this source commit");
  await expect(workspace.getByText("No workspace entries yet.")).toBeVisible();
  await expect(workspace.getByTestId("agent-command-scope"))
    .toContainText("Bounded commands authorized");
  await expect(workspace.getByTestId("agent-connect-command"))
    .toContainText(`myelin mcp serve --as ${agentId} --thread ${
      new URL(page.url()).searchParams.get("thread")!
    }`);
  const sshCommand = workspace.getByTestId("agent-workspace-command");
  await expect(sshCommand).toContainText("myelin agent thread ssh");
  await expect(sshCommand).toContainText(new URL(page.url()).searchParams.get("thread")!);
  await expect(workspace.getByTestId("agent-workspace-file-command"))
    .toContainText(`myelin agent thread ssh ${
      new URL(page.url()).searchParams.get("thread")!
    } --sftp`);
  await expect(workspace.getByTestId("agent-workspace-editor-command"))
    .toContainText(`myelin agent thread ssh-config ${
      new URL(page.url()).searchParams.get("thread")!
    } --identity /path/to/ssh_identity`);
  const workspaceDeadline = workspace.locator("time");
  const initialDeadline = await workspaceDeadline.getAttribute("datetime");
  expect(initialDeadline).not.toBeNull();
  await workspace.getByRole("combobox", { name: "Keep from today" }).selectOption("30");
  await workspace.getByRole("button", { name: "Keep workspace" }).click();
  await expect.poll(async () => workspaceDeadline.getAttribute("datetime"))
    .not.toBe(initialDeadline);
  const retainedDeadline = await workspaceDeadline.getAttribute("datetime");
  expect(Date.parse(retainedDeadline!)).toBeGreaterThan(Date.parse(initialDeadline!));
  await expect(page.getByRole("link", { name: `${threadName} ready · 30 days` })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(workspace).toBeHidden();

  let resumedThroughProduct = false;
  try {
    await page.getByRole("button", { name: "Manage my agents" }).click();
    const lifecycleRoster = page.getByRole("dialog", { name: "My named agents" });
    const lifecycleAgent = lifecycleRoster.getByRole("listitem").filter({ hasText: renamedAgentName });
    await lifecycleAgent.getByRole("button", { name: "Suspend" }).click();
    await expect(lifecycleAgent).toContainText("suspended · external MCP");
    await lifecycleRoster.getByRole("button", { name: "Close", exact: true }).click();

    await page.reload();
    await waitForAppHydration(page);
    await page.getByRole("button", { name: "Agent workspace" }).click();
    await expect(workspace.locator("time")).toHaveAttribute("datetime", retainedDeadline!);
    await expect(workspace.getByTestId("agent-connect-unavailable"))
      .toContainText(`${renamedAgentName} is suspended`);
    await expect(workspace.getByTestId("agent-connect-command")).toHaveCount(0);
    await expect(workspace.getByTestId("agent-workspace-command")).toHaveCount(0);
    await expect(workspace.getByTestId("agent-workspace-file-command")).toHaveCount(0);
    await expect(workspace.getByTestId("workspace-ssh-unavailable"))
      .toContainText("until the selected agent identity is active");
    await postProductJson(
      request,
      `/v1/agent-threads/${encodeURIComponent(
        new URL(page.url()).searchParams.get("thread")!,
      )}/ssh-access`,
      { public_key: ephemeralEd25519PublicKey() },
      { token: sessionToken, scheme: "session", status: 409 },
    );
    await page.keyboard.press("Escape");

    await page.getByRole("button", { name: "Manage my agents" }).click();
    const resumeRoster = page.getByRole("dialog", { name: "My named agents" });
    const suspendedAgent = resumeRoster.getByRole("listitem").filter({ hasText: renamedAgentName });
    await suspendedAgent.getByRole("button", { name: "Resume" }).click();
    await expect(suspendedAgent).toContainText("active · external MCP");
    resumedThroughProduct = true;
    await resumeRoster.getByRole("button", { name: "Close", exact: true }).click();
  } finally {
    if (!resumedThroughProduct) {
      await postProductJson(
        request,
        `/v1/agents/${encodeURIComponent(agentId)}/resume`,
        {},
        { token: sessionToken, scheme: "session", status: 200 },
      );
    }
  }

  await page.getByLabel(`Message ${threadName}`).fill(problem);
  await page.getByRole("button", { name: "Send privately" }).click();
  await expect(page.getByText(problem)).toBeVisible();
  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByText(problem)).toBeVisible();
  await expect(page.getByText(`Private with ${renamedAgentName}`)).toBeVisible();

  const threadId = new URL(page.url()).searchParams.get("thread")!;
  await commitWorkspaceReview(request, sessionToken, threadId);
  const agentResponse = await request.get(
    `${integrationEdgeUrl}/v1/agents/${encodeURIComponent(agentId)}`,
    {
      headers: {
        authorization: `Bearer ${sessionToken}`,
        "x-myelin-token-scheme": "session",
      },
    },
  );
  const agentText = await agentResponse.text();
  expect(agentResponse.status(), agentText).toBe(200);
  const durableAgent = (JSON.parse(agentText) as {
    agent: { selected_tools: Array<{ name: string }>; effective_tools: Array<{ name: string }> };
  }).agent;
  expect(durableAgent.selected_tools).toEqual(expect.arrayContaining([
    expect.objectContaining({ name: "workspace.exec" }),
  ]));
  expect(durableAgent.selected_tools).not.toEqual(expect.arrayContaining([
    expect.objectContaining({ name: "chat.reply" }),
  ]));
  expect(durableAgent.effective_tools).toEqual(expect.arrayContaining([
    expect.objectContaining({ name: "workspace.exec" }),
  ]));
  const durable = await request.get(
    `${integrationEdgeUrl}/v1/agent-threads/${encodeURIComponent(threadId)}/messages?limit=100`,
    {
      headers: {
        authorization: `Bearer ${sessionToken}`,
        "x-myelin-token-scheme": "session",
      },
    },
  );
  const durableText = await durable.text();
  expect(durable.status(), durableText).toBe(200);
  expect((JSON.parse(durableText) as JsonObject).items).toEqual(expect.arrayContaining([
    expect.objectContaining({ content: problem, is_you: true, state: "active" }),
  ]));
  expect((JSON.parse(durableText) as JsonObject).conversation).toEqual(expect.objectContaining({
    retention_days: 30,
  }));

  const threadResponse = await request.get(
    `${integrationEdgeUrl}/v1/agent-threads/${encodeURIComponent(threadId)}`,
    {
      headers: {
        authorization: `Bearer ${sessionToken}`,
        "x-myelin-token-scheme": "session",
      },
    },
  );
  const threadText = await threadResponse.text();
  expect(threadResponse.status(), threadText).toBe(200);
  const durableThread = JSON.parse(threadText) as {
    thread: { ref: string; project_id: string };
  };
  const threadRef = durableThread.thread.ref;
  const projectId = durableThread.thread.project_id;

  await page.getByRole("link", { name: "Chat" }).click();
  await page.waitForURL("**/chat");
  await page.getByRole("button", { name: "Create a topic" }).click();
  await page.getByRole("textbox", { name: "Channel", exact: true }).fill(`private-work-${suffix}`);
  const handoffTopic = `Fresh-context handoff ${suffix}`;
  await page.getByRole("textbox", { name: "Topic", exact: true }).fill(handoffTopic);
  await page.getByRole("button", { name: "Create topic" }).click();
  await page.getByLabel(`Message ${handoffTopic}`).fill("Resume the investigation where it lives.");
  await page.getByRole("button", { name: "Link work" }).click();
  await page.getByRole("textbox", { name: "Canonical Myelin reference" }).fill(threadRef);
  await page.getByRole("button", { name: "Add reference" }).click();
  await page.getByRole("button", { name: "Send" }).click();
  const threadCard = page.getByRole("link", { name: `${threadName}, ready` });
  await expect(threadCard).toHaveAttribute("href", `/agents?thread=${threadId}`);
  await threadCard.click();
  await expect(page).toHaveURL(new RegExp(`/agents\\?thread=${threadId}$`));
  await expect(page.getByRole("heading", { level: 2, name: threadName })).toBeVisible();

  await expect(page.getByText("Owner and agent only", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Ready for review" }).click();
  await expect(page.getByText("Project collaborators can review", { exact: true })).toBeVisible();
  await expect(page.getByText(`Project review with ${renamedAgentName}`, { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Send to review" })).toBeVisible();
  const collaboratorResponse = await request.get(
    `${integrationEdgeUrl}/v1/projects/${encodeURIComponent(projectId)}/collaborators?limit=100`,
    {
      headers: {
        authorization: `Bearer ${sessionToken}`,
        "x-myelin-token-scheme": "session",
      },
      failOnStatusCode: false,
    },
  );
  const collaboratorText = await collaboratorResponse.text();
  expect(collaboratorResponse.status(), collaboratorText).toBe(200);
  expect((JSON.parse(collaboratorText) as { items: Array<{ principal_id: string }> }).items)
    .toEqual(expect.arrayContaining([
      expect.objectContaining({ principal_id: integrationReviewerPrincipal }),
    ]));
  await page.getByRole("button", { name: "Agent workspace" }).click();
  await expect(workspace.getByRole("button", { name: "Open pull request" })).toBeVisible();
  const reviewers = workspace.getByRole("group", { name: "Request review from" });
  const reviewer = reviewers.getByRole("checkbox", {
    name: new RegExp(integrationReviewerPrincipal),
  });
  await expect(reviewer).toBeVisible();
  await reviewer.check();
  await commitWorkspaceReview(request, sessionToken, threadId, "hidden-credential");
  await workspace.getByRole("button", { name: "Open pull request" }).click();
  await expect(workspace.getByRole("alert")).toContainText(
    "review history contains a possible secret at `browser-credential.env`; rewrite it out of the branch",
  );
  await commitWorkspaceReview(request, sessionToken, threadId, "rewrite-hidden-credential");
  await workspace.getByRole("button", { name: "Open pull request" }).click();
  const pullRequestLink = workspace.getByRole("link", { name: /^Pull request #\d+$/ });
  await expect(pullRequestLink).toBeVisible();
  const pullRequestHref = await pullRequestLink.getAttribute("href");
  expect(pullRequestHref).toMatch(
    new RegExp(`/git/repos/${encodeURIComponent(repositorySlug)}/prs/\\d+$`),
  );
  const reviewerPullRequest = await request.get(`${integrationEdgeUrl}/v1${pullRequestHref}`, {
    headers: {
      authorization: `Bearer ${integrationReviewerToken}`,
      "x-myelin-token-scheme": "agent",
    },
    failOnStatusCode: false,
  });
  const reviewerPullRequestText = await reviewerPullRequest.text();
  expect(reviewerPullRequest.status(), reviewerPullRequestText).toBe(200);
  const originalHead = String((JSON.parse(reviewerPullRequestText) as JsonObject).head_oid);
  await commitWorkspaceReview(request, sessionToken, threadId, "follow-up");
  await expect(workspace.getByRole("button", { name: "Update pull request" })).toBeVisible();
  await workspace.getByRole("button", { name: "Update pull request" }).click();
  await expect.poll(async () => {
    const response = await request.get(`${integrationEdgeUrl}/v1${pullRequestHref}`, {
      headers: {
        authorization: `Bearer ${integrationReviewerToken}`,
        "x-myelin-token-scheme": "agent",
      },
    });
    if (response.status() !== 200) return null;
    return String(((await response.json()) as JsonObject).head_oid);
  }).not.toBe(originalHead);
  await expect(workspace.getByRole("link", { name: /^Pull request #\d+$/ }))
    .toHaveAttribute("href", pullRequestHref!);
  await page.keyboard.press("Escape");
  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByText("Project collaborators can review", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Agent workspace" }).click();
  await expect(workspace.getByRole("link", { name: /^Pull request #\d+$/ }))
    .toHaveAttribute("href", pullRequestHref!);
  await workspace.getByRole("button", { name: "Keep workspace" }).click();
  await expect.poll(async () => workspace.locator("time").getAttribute("datetime"))
    .not.toBe(retainedDeadline);
  await page.keyboard.press("Escape");
  await expect(page.getByText("Project collaborators can review", { exact: true })).toBeVisible();

  const accessibility = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(accessibility.violations).toEqual([]);

  await page.getByRole("button", { name: "Manage my agents" }).click();
  const retirementRoster = page.getByRole("dialog", { name: "My named agents" });
  const retiringAgent = retirementRoster.getByRole("listitem").filter({ hasText: renamedAgentName });
  await retiringAgent.getByRole("button", { name: "Retire" }).click();
  const confirmation = page.getByRole("alertdialog", { name: `Retire ${renamedAgentName}?` });
  await expect(confirmation).toContainText("cannot be undone");
  await confirmation.getByRole("button", { name: "Retire agent" }).click();
  await expect(retiringAgent).toContainText("Retired permanently");
  await retirementRoster.getByRole("button", { name: "Close", exact: true }).click();

  await page.reload();
  await waitForAppHydration(page);
  await page.getByRole("button", { name: "Manage my agents" }).click();
  const finalRoster = page.getByRole("dialog", { name: "My named agents" });
  await expect(finalRoster.getByRole("listitem").filter({ hasText: renamedAgentName }))
    .toContainText("disabled · external MCP");

  await finalRoster.getByRole("button", { name: "Activate agent" }).click();
  const hostedActivation = page.getByRole("dialog", { name: "Activate named agent" });
  const hostedAgentName = `hosted-triage-${suffix}`;
  await hostedActivation.getByRole("textbox", { name: "Agent name" }).fill(hostedAgentName);
  await hostedActivation.getByRole("combobox", { name: "Runtime" }).selectOption("hosted");
  await expect(hostedActivation.getByRole("textbox", { name: "Default workspace instructions" }))
    .toHaveCount(0);
  await hostedActivation.getByRole("button", { name: "Activate agent" }).click();
  await expect(hostedActivation).toBeHidden();
  const hostedRoster = page.getByRole("dialog", { name: "My named agents" });
  const hostedAgent = hostedRoster.getByRole("listitem").filter({ hasText: hostedAgentName });
  await expect(hostedAgent).toContainText("active · hosted");
  await expect(page.getByRole("dialog", { name: "Start private agent thread" })).toHaveCount(0);
  await hostedRoster.getByRole("button", { name: "Close", exact: true }).click();

  await page.getByRole("link", { name: "Automations" }).click();
  await page.waitForURL("**/automations");
  await page.getByRole("button", { name: "New automation" }).click();
  const automation = page.getByRole("dialog", { name: "Create governed automation" });
  const automationTask = `Read the failed ${suffix} run and open one focused issue.`;
  await automation.getByRole("combobox", { name: "Run as hosted agent" })
    .selectOption({ label: hostedAgentName });
  await expect(automation.getByRole("combobox", { name: "Platform event" }))
    .toHaveValue("ci.run.failed");
  await automation.getByRole("textbox", { name: "Agent task" }).fill(automationTask);
  await automation.getByRole("spinbutton", { name: "Budget per firing · minor units" })
    .fill("250000");
  await automation.getByRole("spinbutton", { name: "Maximum firings" }).fill("2");
  await expect(automation.getByRole("checkbox", { name: "Require a human approval" }))
    .toBeChecked();
  await automation.getByRole("button", { name: "Create automation" }).click();
  await page.waitForURL(/\/automations\/[0-9a-f-]{36}$/);
  await expect(page.getByRole("heading", { level: 1, name: automationTask })).toBeVisible();
  await expect(page.getByText("250,000 minor-units", { exact: true })).toBeVisible();
  await expect(page.getByText("0 of 2", { exact: true })).toBeVisible();
  await expect(page.getByText("Required", { exact: true })).toBeVisible();
  await expect(page.getByText("Refused", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Disable" }).click();
  await page.getByRole("alertdialog", { name: "Disable this automation?" })
    .getByRole("button", { name: "Disable automation" }).click();
  await expect(page.getByText("Disabled", { exact: true })).toBeVisible();

  await page.getByRole("link", { name: "Agents" }).click();
  await page.getByRole("button", { name: "Manage my agents" }).click();
  const cleanupRoster = page.getByRole("dialog", { name: "My named agents" });
  const cleanupAgent = cleanupRoster.getByRole("listitem").filter({ hasText: hostedAgentName });
  await cleanupAgent.getByRole("button", { name: "Retire" }).click();
  await page.getByRole("alertdialog", { name: `Retire ${hostedAgentName}?` })
    .getByRole("button", { name: "Retire agent" }).click();
  await expect(cleanupAgent).toContainText("Retired permanently");
});
