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

test("a signed-in engineer creates and resumes an encrypted durable Chat topic", async ({
  page,
  request,
}) => {
  const suffix = `${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const channel = `delivery-${suffix}`;
  const topic = `release-${suffix}`;
  const message = `Gate ${suffix} is green; continue the EU rollout.`;

  await signIn(page);
  await navigateToApp(page, "/chat");

  await page.getByRole("button", { name: "Create a topic" }).click();
  await page.getByRole("textbox", { name: "Channel", exact: true }).fill(channel);
  await page.getByRole("textbox", { name: "Topic", exact: true }).fill(topic);
  await page.getByRole("button", { name: "Create topic" }).click();
  await page.waitForURL(/\/chat\?conversation=[0-9A-HJKMNP-TV-Z]{26}$/);
  await expect(page.getByRole("heading", { name: topic })).toBeVisible();

  await page.getByLabel(`Message ${topic}`).fill(message);
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page.getByText(message)).toBeVisible();
  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByText(message)).toBeVisible();

  const conversations = await request.get(`${edgeUrl}/v1/chat/conversations?limit=100`, {
    headers: {
      authorization: `Bearer ${token}`,
      "x-myelin-token-scheme": "agent",
    },
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
      headers: {
        authorization: `Bearer ${token}`,
        "x-myelin-token-scheme": "agent",
      },
    },
  );
  const messageText = await messages.text();
  expect(messages.status(), messageText).toBe(200);
  expect(JSON.parse(messageText).items).toEqual(expect.arrayContaining([
    expect.objectContaining({ content: message, is_you: true, state: "active" }),
  ]));
});
