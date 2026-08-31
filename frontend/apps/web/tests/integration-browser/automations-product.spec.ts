import { randomUUID } from "node:crypto";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import {
  browserApprovedSession,
  integrationEdgeUrl,
  postProductJson as post,
  type JsonObject,
} from "./product-api";
import { signIn, waitForAppHydration } from "./session";

async function expectAccessible(page: Page, context: string): Promise<void> {
  const result = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(
    result.violations,
    `${context}: ${JSON.stringify(result.violations, null, 2)}`,
  ).toEqual([]);
}

test("an automation owner governs durable agent work without configuring an integration key", async ({
  page,
  request,
}) => {
  test.setTimeout(90_000);
  const suffix = `${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const task = `Review ownership changes for ${suffix} and leave a concise handoff.`;
  const sessionToken = await browserApprovedSession(request);
  const owner = { token: sessionToken, scheme: "session" };
  const createdProject = await post(request, "/v1/projects", {
    name: `Automation approvals ${suffix}`,
    issue_prefix: `A${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`,
  }, { ...owner, status: 201 });
  const project = createdProject.project as JsonObject;
  const projectId = String(project.id);
  expect(projectId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
  const agentName = `ownership-companion-${suffix}`;

  await signIn(page);
  await page.goto("/agents?activate=1&runtime=hosted");
  await waitForAppHydration(page);
  const activation = page.getByRole("dialog", { name: "Activate named agent" });
  await expect(activation.getByRole("combobox", { name: "Runtime" })).toHaveValue("hosted");
  await activation.getByRole("textbox", { name: "Agent name" }).fill(agentName);
  await activation.getByRole("button", { name: "Activate agent" }).click();
  await expect(activation).toBeHidden();
  const activatedRoster = page.getByRole("dialog", { name: "My named agents" });
  await expect(activatedRoster.getByRole("listitem").filter({ hasText: agentName }))
    .toContainText("active · hosted");
  await activatedRoster.getByRole("button", { name: "Close", exact: true }).click();

  await page.getByLabel("Primary").getByRole("link", { name: "Automations" }).click();
  await page.waitForURL("**/automations");
  const firstAutomation = await createAutomationInBrowser(page, agentName, task);
  const automationId = firstAutomation.id;
  const automationRef = firstAutomation.ref;
  const secondTask = `Summarise a separate ownership change for ${suffix}.`;
  await page.getByLabel("Primary").getByRole("link", { name: "Automations" }).click();
  await page.waitForURL("**/automations");
  const secondAutomation = await createAutomationInBrowser(page, agentName, secondTask);
  const secondAutomationId = secondAutomation.id;
  const secondAutomationRef = secondAutomation.ref;

  await page.getByLabel("Primary").getByRole("link", { name: "Automations" }).click();
  await page.waitForURL("**/automations");
  const row = page.getByTestId("automation-row").filter({ hasText: task });
  await expect(row).toBeVisible();
  await expect(row).toContainText("Active");
  await expect(row).toContainText("0 / 8 firings");
  await expectAccessible(page, "automation list");

  await row.click();
  await page.waitForURL(`**/automations/${automationId}`);
  await expect(page.getByRole("heading", { level: 1, name: task })).toBeVisible();
  await expect(page.getByRole("button", { name: "Copy reference" }))
    .toHaveAttribute("title", automationRef);
  await expect(page.getByText("Required", { exact: true })).toBeVisible();
  await expect(page.getByText("Refused", { exact: true })).toBeVisible();
  await expect(page.getByText("No matching events have reserved work yet.")).toBeVisible();

  await createIssueEvent(request, owner, projectId, `Approve hosted work ${suffix}`);
  const awaitingApproval = page.getByTestId("automation-firing-row").filter({
    has: page.getByRole("button", { name: "Approve" }),
  });
  await expect(awaitingApproval).toBeVisible({ timeout: 30_000 });
  await expect(awaitingApproval).toContainText("Waiting for its owner’s decision");
  const approvedEventId = (await awaitingApproval.locator("code").textContent())?.trim() ?? "";
  expect(approvedEventId).not.toBe("");
  const approvedRow = page.getByTestId("automation-firing-row").filter({
    hasText: approvedEventId,
  });
  await awaitingApproval.getByRole("button", { name: "Approve" }).click();
  await expect(approvedRow).toContainText("approved by");
  await expect(approvedRow.getByRole("button", { name: "Approve" })).toHaveCount(0);

  await createIssueEvent(request, owner, projectId, `Reject hosted work ${suffix}`);
  const awaitingRejection = page.getByTestId("automation-firing-row").filter({
    has: page.getByRole("button", { name: "Reject" }),
  });
  await expect(awaitingRejection).toBeVisible({ timeout: 30_000 });
  const rejectedEventId = (await awaitingRejection.locator("code").textContent())?.trim() ?? "";
  expect(rejectedEventId).not.toBe("");
  const rejectedRow = page.getByTestId("automation-firing-row").filter({
    hasText: rejectedEventId,
  });
  await awaitingRejection.getByRole("button", { name: "Reject" }).click();
  const rejection = page.getByRole("alertdialog", { name: "Reject this automation firing?" });
  await expect(rejection).toContainText("end without starting");
  await rejection.getByRole("button", { name: "Reject firing" }).click();
  await expect(rejectedRow).toContainText("rejected by");
  await expect(rejectedRow.getByRole("button", { name: "Reject" })).toHaveCount(0);
  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByTestId("automation-firing-row").filter({ hasText: approvedEventId }))
    .toContainText("approved by");
  await expect(page.getByTestId("automation-firing-row").filter({ hasText: rejectedEventId }))
    .toContainText("rejected by");

  await page.getByRole("button", { name: "Pause" }).click();
  await expect(page.getByTitle("State: Paused")).toBeVisible();
  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByTitle("State: Paused")).toBeVisible();

  await page.getByRole("button", { name: "Resume" }).click();
  await expect(page.getByTitle("State: Active")).toBeVisible();
  await page.getByRole("button", { name: "Disable" }).click();
  const dialog = page.getByRole("alertdialog", { name: "Disable this automation?" });
  await expect(dialog).toContainText("irreversible");
  await dialog.getByRole("button", { name: "Disable automation" }).click();
  await expect(page.getByTitle("State: Disabled")).toBeVisible();
  await expect(page.getByRole("button", { name: "Resume" })).toHaveCount(0);

  await page.evaluate((id) => {
    window.history.pushState({}, "", `/automations/${id}`);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, secondAutomationId);
  await page.waitForURL(`**/automations/${secondAutomationId}`);
  await expect(page.getByRole("heading", { level: 1, name: secondTask })).toBeVisible();
  await expect(page.getByTitle("State: Active")).toBeVisible();
  await expect(page.getByRole("button", { name: "Pause" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Copy reference" }))
    .toHaveAttribute("title", secondAutomationRef);
  await page.getByRole("button", { name: "Disable" }).click();
  await page.getByRole("alertdialog", { name: "Disable this automation?" })
    .getByRole("button", { name: "Disable automation" }).click();
  await expect(page.getByTitle("State: Disabled")).toBeVisible();

  await page.goBack();
  await page.waitForURL(`**/automations/${automationId}`);
  await expect(page.getByRole("heading", { level: 1, name: task })).toBeVisible();
  await expect(page.getByTitle("State: Disabled")).toBeVisible();

  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByTitle("State: Disabled")).toBeVisible();
  await expectAccessible(page, "disabled automation detail");

  await page.getByRole("link", { name: "Agents" }).click();
  await page.waitForURL("**/agents");
  await page.getByRole("button", { name: "Manage my agents" }).click();
  const cleanupRoster = page.getByRole("dialog", { name: "My named agents" });
  const cleanupAgent = cleanupRoster.getByRole("listitem").filter({ hasText: agentName });
  await cleanupAgent.getByRole("button", { name: "Retire" }).click();
  await page.getByRole("alertdialog", { name: `Retire ${agentName}?` })
    .getByRole("button", { name: "Retire agent" }).click();
  await expect(cleanupAgent).toContainText("Retired permanently");
});

async function createAutomationInBrowser(
  page: Page,
  agentName: string,
  task: string,
): Promise<{ id: string; ref: string }> {
  await page.getByRole("button", { name: "New automation" }).click();
  const dialog = page.getByRole("dialog", { name: "Create governed automation" });
  await dialog.getByRole("combobox", { name: "Run as hosted agent" })
    .selectOption({ label: agentName });
  await dialog.getByRole("combobox", { name: "Platform event" }).fill("issue.issue.created");
  await dialog.getByRole("textbox", { name: "Agent task" }).fill(task);
  await dialog.getByRole("spinbutton", { name: "Budget per firing · minor units" }).fill("125000");
  await dialog.getByRole("spinbutton", { name: "Maximum firings" }).fill("8");
  await expect(dialog.getByRole("checkbox", { name: "Require a human approval" })).toBeChecked();
  await dialog.getByRole("button", { name: "Create automation" }).click();
  await page.waitForURL(/\/automations\/[0-9a-f-]{36}$/);
  const id = new URL(page.url()).pathname.split("/").at(-1) ?? "";
  expect(id).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
  await expect(page.getByRole("heading", { level: 1, name: task })).toBeVisible();
  const ref = await page.getByRole("button", { name: "Copy reference" }).getAttribute("title");
  expect(ref).toMatch(/^myelin:\/\//);
  return { id, ref: ref! };
}

async function createIssueEvent(
  request: Parameters<typeof post>[0],
  owner: { token: string; scheme: string },
  projectId: string,
  title: string,
): Promise<void> {
  const requested = await post(request, "/v1/issues", {
    project_id: projectId,
    title,
  }, { ...owner, status: 202 });
  const authorization = requested.authorization as JsonObject;
  const requestEventId = String(authorization.request_event_id);
  expect(requestEventId).not.toBe("");
  await expect.poll(async () => {
    const response = await request.get(
      `${integrationEdgeUrl}/v1/issues/authorization-requests/${encodeURIComponent(requestEventId)}`,
      {
        headers: {
          authorization: `Bearer ${owner.token}`,
          "x-myelin-token-scheme": owner.scheme,
        },
        failOnStatusCode: false,
      },
    );
    return response.status();
  }, { message: `issue event for ${title} should become durable`, timeout: 20_000 }).toBe(200);
}
