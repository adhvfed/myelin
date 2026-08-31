import { randomBytes, randomUUID } from "node:crypto";
import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import {
  browserApprovedSession,
  integrationEdgeUrl,
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

test("an engineer keeps a cloud branch and its environment with one agent", async ({
  page,
  request,
}) => {
  const suffix = `${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const agentName = `checkout-companion-${suffix}`;
  const threadName = `Investigate checkout race ${suffix}`;
  const problem = `The final reader for ${suffix} still owns its checkout lease.`;
  const repositorySlug = `cloud-workspace-${suffix}`;
  const sessionToken = await browserApprovedSession(request);
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
  const activation = page.getByRole("dialog", { name: "Activate private-work agent" });
  await activation.getByRole("textbox", { name: "Agent name" }).fill(agentName);
  await activation.getByRole("checkbox", { name: "Allow bounded workspace commands" }).check();
  await activation.getByRole("button", { name: "Activate agent" }).click();

  const dialog = page.getByRole("dialog", { name: "Start private agent thread" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("textbox", { name: "Problem name" }).fill(threadName);
  const agentChoice = dialog.getByRole("combobox", { name: "Agent" });
  await agentChoice.selectOption({ label: agentName });
  const agentId = await agentChoice.inputValue();
  expect(agentId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
  await dialog.getByRole("combobox", { name: "Keep workspace for" }).selectOption("3");
  await dialog.getByRole("textbox", { name: "Repository slug" }).fill(repositorySlug);
  await dialog.getByRole("button", { name: "Start thread" }).click();

  await page.waitForURL(/\/agents\?thread=[0-9a-f-]{36}$/);
  await expect(page.getByRole("heading", { level: 2, name: threadName })).toBeVisible();
  await expect(page.getByText(`Private with ${agentName}`)).toBeVisible();
  await page.getByRole("button", { name: "Agent workspace" }).click();
  const workspace = page.getByRole("dialog", { name: "Agent workspace" });
  await expect(workspace.getByText("Generation 1", { exact: true })).toBeVisible();
  await expect(workspace.getByText(repositorySlug, { exact: true })).toBeVisible();
  await expect(workspace.getByTestId("workspace-environment"))
    .toContainText("Service Federation · declared");
  await expect(workspace.getByText("fed.yaml", { exact: true })).toBeVisible();
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
  await page.keyboard.press("Escape");
  await expect(workspace).toBeHidden();

  await postProductJson(
    request,
    `/v1/agents/${encodeURIComponent(agentId)}/suspend`,
    {},
    { token: sessionToken, scheme: "session", status: 200 },
  );
  try {
    await page.reload();
    await waitForAppHydration(page);
    await page.getByRole("button", { name: "Agent workspace" }).click();
    await expect(workspace.getByTestId("agent-connect-unavailable"))
      .toContainText(`${agentName} is suspended`);
    await expect(workspace.getByTestId("agent-connect-command")).toHaveCount(0);
    await expect(workspace.getByTestId("agent-workspace-command")).toHaveCount(0);
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
  } finally {
    await postProductJson(
      request,
      `/v1/agents/${encodeURIComponent(agentId)}/resume`,
      {},
      { token: sessionToken, scheme: "session", status: 200 },
    );
  }

  await page.getByLabel(`Message ${threadName}`).fill(problem);
  await page.getByRole("button", { name: "Send privately" }).click();
  await expect(page.getByText(problem)).toBeVisible();
  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByText(problem)).toBeVisible();
  await expect(page.getByText(`Private with ${agentName}`)).toBeVisible();

  const threadId = new URL(page.url()).searchParams.get("thread")!;
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
  const threadRef = (JSON.parse(threadText) as { thread: { ref: string } }).thread.ref;

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

  const accessibility = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(accessibility.violations).toEqual([]);
});
