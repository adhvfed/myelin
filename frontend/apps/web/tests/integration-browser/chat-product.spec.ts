import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";
import { navigateToApp, signIn, waitForAppHydration } from "./session";

function requiredEnvironment(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required; run this test with fed test:integration`);
  return value;
}

const edgeUrl = requiredEnvironment("MYELIN_INTEGRATION_EDGE_URL").replace(/\/$/, "");
const token = requiredEnvironment("MYELIN_BROWSER_EDGE_TOKEN");
const tenant = requiredEnvironment("MYELIN_BROWSER_TENANT");

test("a signed-in engineer creates and resumes an encrypted durable Chat topic", async ({
  page,
  request,
}) => {
  const suffix = `${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const channel = `delivery-${suffix}`;
  const topic = `release-${suffix}`;
  const message = `Gate ${suffix} is green; continue the EU rollout.`;
  const issueTitle = `Coordinate the browser rollout ${suffix}`;
  const headers = {
    authorization: `Bearer ${token}`,
    "x-myelin-token-scheme": "agent",
  };
  const projectResponse = await request.post(`${edgeUrl}/v1/projects`, {
    headers: { ...headers, "idempotency-key": randomUUID() },
    data: {
      name: `Browser reference cards ${suffix}`,
      issue_prefix: `B${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`,
    },
  });
  const projectText = await projectResponse.text();
  expect(projectResponse.status(), projectText).toBe(201);
  const project = (JSON.parse(projectText) as { project: { id: string } }).project;
  const issueResponse = await request.post(`${edgeUrl}/v1/issues`, {
    headers: { ...headers, "idempotency-key": randomUUID() },
    data: { project_id: project.id, title: issueTitle },
  });
  const issueText = await issueResponse.text();
  expect(issueResponse.status(), issueText).toBe(202);
  const requestEventId = (JSON.parse(issueText) as {
    authorization: { request_event_id: string };
  }).authorization.request_event_id;
  let issue: { ref: string; state: string; title: string } | undefined;
  await expect.poll(async () => {
    const status = await request.get(
      `${edgeUrl}/v1/issues/authorization-requests/${requestEventId}`,
      { headers },
    );
    if (status.status() === 200) {
      issue = (JSON.parse(await status.text()) as {
        issue: { ref: string; state: string; title: string };
      }).issue;
    }
    return status.status();
  }, { message: "the browser's referenced issue should become active" }).toBe(200);
  expect(issue).toMatchObject({ title: issueTitle });
  const issueKey = issue!.ref.split("/").at(-1)!;
  await expect.poll(async () => {
    const visible = await request.get(
      `${edgeUrl}/v1/issues?state=all&key=${encodeURIComponent(issueKey)}`,
      { headers, failOnStatusCode: false },
    );
    if (visible.status() !== 200) return false;
    const body = JSON.parse(await visible.text()) as { items: Array<{ ref: string }> };
    return body.items.some((candidate) => candidate.ref === issue!.ref);
  }, { message: "the referenced issue should enter the effective viewer projection" }).toBe(true);
  const relatedTopic = `incident-notes-${suffix}`;
  const relatedResponse = await request.post(`${edgeUrl}/v1/chat/conversations`, {
    headers: { ...headers, "idempotency-key": randomUUID() },
    data: {
      project_id: project.id,
      channel: `private-context-${suffix}`,
      topic: relatedTopic,
    },
  });
  const relatedText = await relatedResponse.text();
  expect(relatedResponse.status(), relatedText).toBe(201);
  const relatedConversation = (JSON.parse(relatedText) as {
    conversation: { id: string; ref: string };
  }).conversation;
  const repository = `chat-handoff-${suffix}`;
  const repositoryResponse = await request.post(`${edgeUrl}/v1/git/repos`, {
    headers: { ...headers, "idempotency-key": randomUUID() },
    data: { slug: repository },
  });
  const repositoryText = await repositoryResponse.text();
  expect(repositoryResponse.status(), repositoryText).toBe(201);
  const commitTitle = `Preserve the handoff ${suffix}`;
  const commitResponse = await request.post(
    `${edgeUrl}/v1/git/repos/${encodeURIComponent(repository)}/blob/main/README.md`,
    {
      headers: { ...headers, "idempotency-key": randomUUID() },
      data: {
        base_oid: "",
        contents: `# ${repository}\n`,
        message: commitTitle,
      },
    },
  );
  const commitText = await commitResponse.text();
  expect(commitResponse.status(), commitText).toBe(200);
  const commitOid = (JSON.parse(commitText) as { applied: { new_oid: string } }).applied.new_oid;
  expect(commitOid).toMatch(/^[0-9a-f]{40}$/);
  const commitRef = `myelin://${tenant}/git/commit/${repository}:${commitOid}`;
  const blobRef =
    `myelin://${tenant}/git/blob/${repository}:refs%2Fheads%2Fmain:README%2Emd#L1-L1`;

  await signIn(page);
  await navigateToApp(page, "/chat");

  await page.getByRole("button", { name: "Create a topic" }).click();
  await page.getByRole("textbox", { name: "Channel", exact: true }).fill(channel);
  await page.getByRole("textbox", { name: "Topic", exact: true }).fill(topic);
  await page.getByRole("button", { name: "Create topic" }).click();
  await page.waitForURL(/\/chat\?conversation=[0-9A-HJKMNP-TV-Z]{26}$/);
  await expect(page.getByRole("heading", { name: topic })).toBeVisible();

  await page.getByLabel(`Message ${topic}`).fill(message);
  await page.getByRole("button", { name: "Link work" }).click();
  await page.getByRole("textbox", { name: "Canonical Myelin reference" }).fill(issue!.ref);
  await page.getByRole("button", { name: "Add reference" }).click();
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page.getByText(message)).toBeVisible();
  await expect(page.getByRole("link", { name: `${issueTitle}, ${issue!.state}` }))
    .toHaveAttribute("href", `/issues?state=all&key=${issueKey}`);
  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByText(message)).toBeVisible();
  await expect(page.getByRole("link", { name: `${issueTitle}, ${issue!.state}` })).toBeVisible();

  await page.getByLabel(`Message ${topic}`).fill("Resume from this exact revision.");
  await page.getByRole("button", { name: "Link work" }).click();
  await page.getByRole("textbox", { name: "Canonical Myelin reference" }).fill(commitRef);
  await page.getByRole("button", { name: "Add reference" }).click();
  await page.getByRole("button", { name: "Send" }).click();
  const commitCard = page.getByRole("link", { name: `${commitTitle}, committed` });
  await expect(commitCard)
    .toHaveAttribute("href", `/git/repos/${repository}/commit/${commitOid}`);
  await commitCard.click();
  await expect(page).toHaveURL(new RegExp(`/git/repos/${repository}/commit/${commitOid}$`));
  await expect(page.getByRole("heading", { level: 1, name: commitTitle })).toBeVisible();
  await page.goBack();
  await expect(page.getByRole("heading", { name: topic })).toBeVisible();

  await page.getByLabel(`Message ${topic}`).fill("Open the exact file discussed here.");
  await page.getByRole("button", { name: "Link work" }).click();
  await page.getByRole("textbox", { name: "Canonical Myelin reference" }).fill(blobRef);
  await page.getByRole("button", { name: "Add reference" }).click();
  await page.getByRole("button", { name: "Send" }).click();
  const blobCard = page.getByRole("link", { name: `${repository} · README.md, file` });
  await expect(blobCard)
    .toHaveAttribute(
      "href",
      `/git/repos/${repository}/blob/refs%2Fheads%2Fmain/README.md#L1-L1`,
    );
  await blobCard.click();
  await expect(page).toHaveURL(
    new RegExp(`/git/repos/${repository}/blob/refs%2Fheads%2Fmain/README.md#L1-L1$`),
  );
  await expect(page.getByRole("heading", { level: 1, name: "README.md" })).toBeVisible();
  await expect(page.locator("#L1")).toContainText(`# ${repository}`);
  await page.goBack();
  await expect(page.getByRole("heading", { name: topic })).toBeVisible();

  await page.getByLabel(`Message ${topic}`).fill("Continue the sensitive investigation here.");
  await page.getByRole("button", { name: "Link work" }).click();
  await page.getByRole("textbox", { name: "Canonical Myelin reference" })
    .fill(relatedConversation.ref);
  await page.getByRole("button", { name: "Add reference" }).click();
  await page.getByRole("button", { name: "Send" }).click();
  const relatedCard = page.getByRole("link", { name: `${relatedTopic}, active` });
  await expect(relatedCard)
    .toHaveAttribute("href", `/chat?conversation=${relatedConversation.id}`);
  await relatedCard.click();
  await expect(page).toHaveURL(new RegExp(`/chat\\?conversation=${relatedConversation.id}$`));
  await expect(page.getByRole("heading", { name: relatedTopic })).toBeVisible();

  const conversations = await request.get(`${edgeUrl}/v1/chat/conversations?limit=100`, {
    headers,
  });
  const conversationText = await conversations.text();
  expect(conversations.status(), conversationText).toBe(200);
  const envelope = JSON.parse(conversationText) as {
    items: Array<{ id: string; channel: string; topic: string }>;
  };
  const created = envelope.items.find((row) => row.channel === channel && row.topic === topic);
  expect(created?.id).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);

  const messages = await request.get(
    `${edgeUrl}/v1/chat/conversations/${created!.id}/messages?limit=100`,
    {
      headers,
    },
  );
  const messageText = await messages.text();
  expect(messages.status(), messageText).toBe(200);
  expect(JSON.parse(messageText).items).toEqual(expect.arrayContaining([
    expect.objectContaining({
      content: `${message} \uFFFC`,
      is_you: true,
      state: "active",
      nodes: [expect.objectContaining({
        ref: issue!.ref,
        card: expect.objectContaining({ kind: "projection", title: issueTitle }),
      })],
    }),
  ]));
});

test("a message posted in one session arrives live in a second session without a reload", async ({
  page,
  browser,
}) => {
  const suffix = `${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const channel = `live-${suffix}`;
  const topic = `standup-${suffix}`;
  const message = `Deploy ${suffix} is rolling; watch the error budget.`;

  await signIn(page);
  await navigateToApp(page, "/chat");
  await page.getByRole("button", { name: "Create a topic" }).click();
  await page.getByRole("textbox", { name: "Channel", exact: true }).fill(channel);
  await page.getByRole("textbox", { name: "Topic", exact: true }).fill(topic);
  await page.getByRole("button", { name: "Create topic" }).click();
  await page.waitForURL(/\/chat\?conversation=[0-9A-HJKMNP-TV-Z]{26}$/);
  const conversationPath = new URL(page.url()).pathname + new URL(page.url()).search;

  const secondSession = await browser.newContext();
  try {
    const pageB = await secondSession.newPage();
    await signIn(pageB);
    await navigateToApp(pageB, conversationPath);
    await expect(pageB.getByRole("heading", { name: topic })).toBeVisible();

    await page.getByLabel(`Message ${topic}`).fill(message);
    await page.getByRole("button", { name: "Send" }).click();
    await expect(page.getByText(message)).toBeVisible();

    // the second session must receive the message pushed over SSE - well inside
    // the 8s bound and far below the 30s fallback poll, so only live delivery
    // can explain it. no reload happens here.
    await expect(pageB.getByText(message)).toBeVisible({ timeout: 8_000 });
  } finally {
    await secondSession.close();
  }
});
