import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;

async function devLogin(page: Page) {
  await page.goto("/login");
  await page.waitForLoadState("networkidle");
  await page.getByTestId("dev-login").click();
  await page.waitForURL("**/git/repos");
}

async function expectAccessible(page: Page, context: string) {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(results.violations, `${context}: ${JSON.stringify(results.violations, null, 2)}`)
    .toEqual([]);
}

test.describe("Chat workspace", () => {
  test.afterEach(async ({ request }) => {
    const response = await request.post(`${EDGE}/__test/config`, {
      data: { resetChat: true, forceUnauthorized: false },
    });
    expect(response.ok()).toBe(true);
  });

  test("organizes channel topics into a durable, accessible conversation timeline", async ({ page }) => {
    await devLogin(page);
    await page.getByRole("navigation", { name: "Primary" }).getByRole("link", { name: "Chat" }).click();
    await page.waitForURL("**/chat");

    await expect(page.getByTestId("chat-screen")).toBeVisible();
    await expect(page.getByText("Under construction", { exact: true })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "engineering" })).toBeVisible();
    await page.getByTestId("chat-topic-link").filter({ hasText: "release readiness" }).click();
    await expect(page.getByRole("heading", { name: "release readiness", level: 2 })).toBeVisible();
    await expect(page.getByText("The canary is healthy.", { exact: false })).toBeVisible();
    await expect(page.getByRole("link", { name: "Issue MYL-204, open", exact: true }))
      .toHaveAttribute("href", "/issues?state=all&key=MYL-204");
    await expect(page.getByText("I’m watching error rate", { exact: false })).toBeVisible();
    await expect(page.getByText("Agent · 543210", { exact: true })).toBeVisible();
    await expectAccessible(page, "seeded Chat timeline");
  });

  test("keeps every unsent draft with the topic where it was written", async ({ page }) => {
    await devLogin(page);
    await page.goto("/chat");

    await page.getByTestId("chat-topic-link").filter({ hasText: "release readiness" }).click();
    const releaseDraft = "Hold this thought with the release.";
    await page.getByLabel("Message release readiness").fill(releaseDraft);

    await page.getByTestId("chat-topic-link").filter({ hasText: "agent operations" }).click();
    await expect(page.getByLabel("Message agent operations")).toHaveText("");
    const agentDraft = "Keep this one with agent operations.";
    await page.getByLabel("Message agent operations").fill(agentDraft);

    await page.getByTestId("chat-topic-link").filter({ hasText: "release readiness" }).click();
    await expect(page.getByLabel("Message release readiness")).toHaveText(releaseDraft);
    await page.getByTestId("chat-topic-link").filter({ hasText: "agent operations" }).click();
    await expect(page.getByLabel("Message agent operations")).toHaveText(agentDraft);
  });

  test("sends a structured work reference that remains navigable after reload", async ({ page }) => {
    await devLogin(page);
    await page.goto("/chat");
    await page.getByTestId("chat-topic-link").filter({ hasText: "release readiness" }).click();

    const message = "Review the linked issue before continuing the rollout.";
    await page.getByLabel("Message release readiness").fill(message);
    await page.getByRole("button", { name: "Link work" }).click();
    await page.getByRole("textbox", { name: "Canonical Myelin reference" })
      .fill("myelin://acme/issue/issue/MYL-777");
    await expectAccessible(page, "Chat reference composer");
    await page.getByRole("button", { name: "Add reference" }).click();
    await expect(page.getByRole("link", { name: "Reference: MYL-777" }))
      .toHaveAttribute("href", "/issues?state=all&key=MYL-777");

    await page.getByRole("button", { name: "Send" }).click();
    const posted = page.locator(".chat-message-you").filter({ hasText: message });
    await expect(posted).toHaveCount(1);
    await expect(posted.getByRole("link", { name: "Issue MYL-777, open" }))
      .toHaveAttribute("href", "/issues?state=all&key=MYL-777");

    await page.reload();
    const durable = page.locator(".chat-message-you").filter({ hasText: message });
    await expect(durable).toHaveCount(1);
    await expect(durable.getByRole("link", { name: "Issue MYL-777, open" })).toBeVisible();
  });

  test("a first user can create a topic, send with Enter, and reload without losing it", async ({ page, request }) => {
    const empty = await request.post(`${EDGE}/__test/config`, { data: { emptyChat: true } });
    expect(empty.ok()).toBe(true);
    await devLogin(page);
    await page.goto("/chat");

    await expect(page.getByText("No conversations yet")).toBeVisible();
    await page.getByRole("button", { name: "Create the first topic" }).click();
    await page.getByRole("textbox", { name: "Channel", exact: true }).fill("platform");
    await page.getByRole("textbox", { name: "Topic", exact: true }).fill("incident follow-up");
    await page.getByRole("button", { name: "Create topic" }).click();

    await page.waitForURL(/\/chat\?conversation=[0-9A-HJKMNP-TV-Z]{26}$/);
    await expect(page.getByRole("heading", { name: "incident follow-up" })).toBeVisible();
    await expect(page.getByText("Start the conversation")).toBeVisible();

    const composer = page.getByLabel("Message incident follow-up");
    await composer.fill("We fixed the timeout and captured the follow-up in MYL-204.");
    await composer.press("Enter");
    await expect(page.getByText("We fixed the timeout and captured the follow-up in MYL-204.")).toBeVisible();
    await expect(page.getByText("You", { exact: true })).toBeVisible();

    await page.reload();
    await expect(page.getByRole("heading", { name: "incident follow-up" })).toBeVisible();
    await expect(page.getByText("We fixed the timeout and captured the follow-up in MYL-204.")).toBeVisible();
    await expectAccessible(page, "created Chat topic");
  });

  test("a response-lost retry keeps one durable message", async ({ page, request }) => {
    const configured = await request.post(`${EDGE}/__test/config`, {
      data: { emptyChat: true, chatPostResponseLosses: 1 },
    });
    expect(configured.ok()).toBe(true);
    await devLogin(page);
    await page.goto("/chat");

    await page.getByRole("button", { name: "Create the first topic" }).click();
    await page.getByRole("textbox", { name: "Channel", exact: true }).fill("reliability");
    await page.getByRole("textbox", { name: "Topic", exact: true }).fill("retry identity");
    await page.getByRole("button", { name: "Create topic" }).click();

    const message = "Commit once even when the acknowledgement disappears.";
    await page.getByLabel("Message retry identity").fill(message);
    await page.getByRole("button", { name: "Send" }).click();
    await expect(page.getByRole("alert")).toContainText("retrying this draft is safe");

    await page.getByRole("button", { name: "Send" }).click();
    await expect(page.getByText(message, { exact: true })).toHaveCount(1);
    await page.reload();
    await expect(page.getByText(message, { exact: true })).toHaveCount(1);
  });

  test("a lost topic acknowledgement can be retried without creating a second topic", async ({ page, request }) => {
    const configured = await request.post(`${EDGE}/__test/config`, {
      data: { emptyChat: true, chatConversationResponseLosses: 1 },
    });
    expect(configured.ok()).toBe(true);
    await devLogin(page);
    await page.goto("/chat");

    await page.getByRole("button", { name: "Create the first topic" }).click();
    await page.getByRole("textbox", { name: "Channel", exact: true }).fill("reliability");
    await page.getByRole("textbox", { name: "Topic", exact: true }).fill("durable retries");
    await page.getByRole("button", { name: "Create topic" }).click();
    await expect(page.getByRole("alert")).toContainText("Retrying this unchanged topic is safe");

    await page.getByRole("button", { name: "Create topic" }).click();
    await expect(page.getByRole("heading", { name: "durable retries", level: 2 })).toBeVisible();
    await page.reload();
    await expect(page.getByTestId("chat-topic-link").filter({ hasText: "durable retries" }))
      .toHaveCount(1);
  });

  test("a late topic creation cannot reclaim navigation after its dialog was left", async ({ page, request }) => {
    const configured = await request.post(`${EDGE}/__test/config`, {
      data: { emptyChat: true, chatConversationResponseDelaysMs: [1_500] },
    });
    expect(configured.ok()).toBe(true);
    await devLogin(page);
    await page.goto("/chat");

    await page.getByRole("button", { name: "Create the first topic" }).click();
    await page.getByRole("textbox", { name: "Channel", exact: true }).fill("reliability");
    await page.getByRole("textbox", { name: "Topic", exact: true }).fill("created after leaving");
    await page.getByRole("button", { name: "Create topic" }).click();
    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.chatConversationCreateRequests;
    }).toBe(1);

    await page.evaluate(() => {
      window.history.pushState({}, "", "/chat");
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    await expect(page.getByRole("dialog", { name: "New topic" })).toHaveCount(0);

    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.chatConversationCreateResponses;
    }).toBe(1);
    await page.waitForTimeout(500);
    expect(new URL(page.url()).searchParams.has("conversation")).toBe(false);
    await expect(page.getByText("Topic created in reliability", { exact: true })).toHaveCount(0);
  });

  test("editing an uncertain topic starts a distinct creation", async ({ page, request }) => {
    const configured = await request.post(`${EDGE}/__test/config`, {
      data: { emptyChat: true, chatConversationResponseLosses: 1 },
    });
    expect(configured.ok()).toBe(true);
    await devLogin(page);
    await page.goto("/chat");

    await page.getByRole("button", { name: "Create the first topic" }).click();
    await page.getByRole("textbox", { name: "Channel", exact: true }).fill("reliability");
    const topic = page.getByRole("textbox", { name: "Topic", exact: true });
    await topic.fill("first attempt");
    await page.getByRole("button", { name: "Create topic" }).click();
    await expect(page.getByRole("alert")).toContainText("Retrying this unchanged topic is safe");

    await topic.fill("revised attempt");
    await page.getByRole("button", { name: "Create topic" }).click();
    await expect(page.getByRole("heading", { name: "revised attempt", level: 2 })).toBeVisible();
    await page.reload();
    await expect(page.getByTestId("chat-topic-link").filter({ hasText: "first attempt" }))
      .toHaveCount(1);
    await expect(page.getByTestId("chat-topic-link").filter({ hasText: "revised attempt" }))
      .toHaveCount(1);
  });

  test("the narrow layout moves cleanly between topic navigation and the composer", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 760 });
    await devLogin(page);
    await page.goto("/chat");
    await expect(page.getByRole("heading", { name: "Chat", level: 1 })).toBeVisible();

    await page.getByTestId("chat-topic-link").filter({ hasText: "agent operations" }).click();
    await expect(page.getByRole("heading", { name: "agent operations", level: 2 })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Chat", level: 1 })).toBeHidden();
    await page.getByRole("link", { name: "Back to topics" }).click();
    await expect(page.getByRole("heading", { name: "Chat", level: 1 })).toBeVisible();
    await expectAccessible(page, "narrow Chat navigation");
  });

  test("an invalid conversation link explains itself and leads back to the topic list", async ({ page }) => {
    await devLogin(page);
    await page.setViewportSize({ width: 375, height: 760 });
    await page.goto("/chat?conversation=not-a-conversation");

    await expect(page.getByRole("heading", { name: "Conversation unavailable" })).toBeVisible();
    await expect(page.getByText("That conversation address is invalid.")).toBeVisible();
    const back = page.getByRole("link", { name: "Back to topics" });
    await expect(back).toHaveAttribute("href", "/chat");
    await expectAccessible(page, "invalid Chat deep link");

    await back.click();
    await expect(page.getByRole("heading", { name: "Chat", level: 1 })).toBeVisible();
  });

  test("a topic created while pagination is loading restarts the catalogue snapshot", async ({ page, request }) => {
    const configured = await request.post(`${EDGE}/__test/config`, {
      data: { chatConversationCount: 101, chatConversationCursorDelaysMs: [1_500] },
    });
    expect(configured.ok()).toBe(true);
    await devLogin(page);
    await page.goto("/chat");

    await page.getByRole("button", { name: "More topics" }).click();
    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.chatConversationCursorRequests;
    }).toBe(1);

    await page.getByRole("button", { name: "Create a topic" }).click();
    await page.getByRole("textbox", { name: "Channel", exact: true }).fill("reliability");
    await page.getByRole("textbox", { name: "Topic", exact: true }).fill("one coherent catalogue");
    await page.getByRole("button", { name: "Create topic" }).click();
    await expect(page.getByTestId("chat-topic-link").filter({ hasText: "one coherent catalogue" }))
      .toBeVisible();

    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.chatConversationCursorResponses;
    }).toBe(1);
    await expect(page.getByRole("button", { name: "More topics" })).toBeVisible();
    await expect(page.getByTestId("chat-topic-link").filter({ hasText: "agent operations" }))
      .toHaveCount(0);

    await page.getByRole("button", { name: "More topics" }).click();
    await expect(page.getByTestId("chat-topic-link").filter({ hasText: "agent operations" }))
      .toBeVisible();
    await expect(page.getByTestId("chat-topic-link").filter({ hasText: "release readiness" }))
      .toBeVisible();
  });

  test("an earlier page can never arrive in the topic opened while it was loading", async ({ page, request }) => {
    const configured = await request.post(`${EDGE}/__test/config`, {
      data: { chatPaginatedMessages: true, chatMessageCursorDelaysMs: [1_500] },
    });
    expect(configured.ok()).toBe(true);
    await devLogin(page);
    await page.goto("/chat");

    await page.getByTestId("chat-topic-link").filter({ hasText: "release readiness" }).click();
    await page.getByRole("button", { name: "Load earlier messages" }).click();
    await page.getByTestId("chat-topic-link").filter({ hasText: "agent operations" }).click();
    await expect(page.getByRole("heading", { name: "agent operations", level: 2 })).toBeVisible();

    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.chatMessageCursorResponses;
    }).toBe(1);
    await expect(page.getByText("The canary is healthy.", { exact: false })).toHaveCount(0);
    await expect(page.getByText("Start the conversation")).toBeVisible();
  });
});
