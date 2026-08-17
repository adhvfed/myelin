import { expect, request as pwRequest, test, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { DEV_ACCESS_TOKEN, DEV_ISSUE_TARGET } from "../../dev-edge/dev-contract.mjs";

const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;
const OPEN_ID = "00000000-0000-4000-8000-000000000102";
const SECOND_OPEN_ID = "00000000-0000-4000-8000-000000000101";

async function setEdgeConfig(cfg: {
  resetIssues?: boolean;
  emptyIssues?: boolean;
  onlyClosedIssues?: boolean;
  issuesUnavailable?: boolean;
  issueActivationPolls?: number;
  issueActivationUnavailable?: boolean;
  issueCreateUnavailable?: boolean;
  issueCreateResponseLosses?: number;
  issueCloseUnavailable?: boolean;
  issueCloseResponseDelaysMs?: number[];
  emptyProjects?: boolean;
  projectsUnavailable?: boolean;
  projectCreateUnavailable?: boolean;
  projectCreateResponseLosses?: number;
  projectCount?: number;
  projectListCursorDelaysMs?: number[];
  issueListFirstPageHolds?: number;
  releaseIssueListFirstPages?: boolean;
  issueListFirstPageDelaysMs?: number[];
  issueListCursorDelaysMs?: number[];
}) {
  const context = await pwRequest.newContext();
  const response = await context.post(`${EDGE}/__test/config`, { data: cfg });
  expect(response.ok(), "dev-edge Issues config must be accepted").toBeTruthy();
  await context.dispose();
}

async function edgeIssueState(): Promise<{
  issueListCursorRequests: number;
  issueListCursorResponses: number;
  issueListFirstPageDelayedRequests: number;
  issueListFirstPageDelayedResponses: number;
  issueListCursorRequestsByState: { open: number; closed: number; all: number };
}> {
  const context = await pwRequest.newContext();
  const response = await context.post(`${EDGE}/__test/config`, { data: {} });
  expect(response.ok()).toBeTruthy();
  const body = await response.json();
  await context.dispose();
  return body.state;
}

async function devLogin(page: Page) {
  await page.goto("/login");
  await page.waitForLoadState("networkidle");
  await page.getByTestId("dev-login").click();
  await page.waitForURL("**/git/repos");
}

async function waitForInteractiveShell(page: Page) {
  await expect(page.locator('.app-shell[data-shortcuts-ready="true"]')).toBeVisible();
}

async function gotoInteractive(page: Page, path: string) {
  await page.goto(path);
  await waitForInteractiveShell(page);
}

async function expectNoAxeViolations(page: Page, context: string) {
  const result = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(
    result.violations,
    `axe violations on ${context}: ${JSON.stringify(result.violations, null, 2)}`,
  ).toEqual([]);
}

test.beforeEach(async () => setEdgeConfig({ resetIssues: true }));
test.afterEach(async () => setEdgeConfig({ resetIssues: true }));

test.describe("issue workflows", () => {
  test("the direct API refuses issue titles people cannot reliably read", async ({ request }) => {
    for (const [name, title] of [
      ["surrounding whitespace", " padded title "],
      ["line breaks", "line\nbreak"],
      ["control characters", "hidden\u0085control"],
    ] as const) {
      const response = await request.post(`${EDGE}/v1/issues`, {
        headers: {
          authorization: `Bearer ${DEV_ACCESS_TOKEN}`,
          "idempotency-key": `invalid-title-${name.replaceAll(" ", "-")}`,
        },
        data: { project_id: DEV_ISSUE_TARGET.project_id, title },
      });
      expect(response.status(), name).toBe(400);
    }
  });

  test("unauthenticated issue detail redirects to login", async ({ page }) => {
    await page.goto(`/issues/${OPEN_ID}`);
    await page.waitForURL("**/login");
    await expect(page.getByTestId("dev-login")).toBeVisible();
  });

  test("list, key search, state tabs, and roving rows stay authoritative and keyboard-operable", async ({ page }) => {
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await expect(page.getByRole("heading", { level: 1, name: "Issues" })).toBeVisible();
    await expect(page.getByText("Close the collaboration feedback loop")).toBeVisible();
    await expect(page.getByTitle("State: Todo").first()).toBeVisible();
    await expectNoAxeViolations(page, "Issues list");

    const rows = page.getByTestId("issue-row");
    await expect(rows.first()).toHaveAttribute("tabindex", "0");
    await rows.first().press("ArrowDown");
    await expect(rows.nth(1)).toBeFocused();
    await rows.nth(1).press("j");
    await expect(rows.nth(2)).toBeFocused();

    await page.getByRole("tab", { name: "Closed" }).click();
    await page.waitForURL("**/issues?state=closed");
    await expect(page.getByText("Consolidate issue navigation")).toBeVisible();
    await expect(page.getByTestId("issue-row").first()).toHaveAttribute("tabindex", "0");

    await page.getByLabel("Find by issue key").fill("myl-101");
    await page.getByRole("button", { name: "Find" }).click();
    await page.waitForURL("**/issues?state=closed&key=MYL-101");
    await expect(page.getByTestId("issues-no-results")).toBeVisible();

    await page.getByRole("tab", { name: "All" }).click();
    await page.waitForURL("**/issues?state=all&key=MYL-101");
    await expect(page.getByText("Verify encrypted issue titles")).toBeVisible();
    await expect(page.getByText("Close the collaboration feedback loop")).toHaveCount(0);
  });

  test("opaque load-more appends the final authorized row without totals", async ({ page }) => {
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await expect(page.getByTestId("issue-row")).toHaveCount(50);
    await page.getByRole("button", { name: "Load more" }).click();
    await expect(page.getByTestId("issue-row")).toHaveCount(51);
    await expect(page.getByText(/of \d+/)).toHaveCount(0);
  });

  test("a delayed old cursor page cannot mix into a new filter generation", async ({ page }) => {
    await setEdgeConfig({ issueListCursorDelaysMs: [750, 2_000] });
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await expect(page.getByTestId("issue-row")).toHaveCount(50);

    await page.getByRole("button", { name: "Load more" }).click();
    await expect.poll(async () => (await edgeIssueState()).issueListCursorRequestsByState.open).toBe(1);
    await page.getByRole("tab", { name: "Closed" }).click();
    await page.waitForURL("**/issues?state=closed");
    await expect(page.getByTestId("issue-row")).toHaveCount(50);
    await page.getByRole("button", { name: "Load more" }).click();
    await expect.poll(async () => (await edgeIssueState()).issueListCursorRequests).toBe(2);
    await expect.poll(async () => (await edgeIssueState()).issueListCursorResponses).toBe(1);

    await expect(page.getByRole("button", { name: "Loading…" })).toBeDisabled();
    await expect(page.getByTestId("issue-row")).toHaveCount(50);

    await expect(page.getByTestId("issue-row")).toHaveCount(51);
    await expect(page.getByText("Consolidate issue navigation")).toBeVisible();
    await expect(page.getByTitle("State: Done")).toHaveCount(51);
    await expect(page.getByTitle("State: Todo")).toHaveCount(0);
  });

  test("quick capture is focus-safe from both the button and command palette", async ({ page }) => {
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await page.waitForLoadState("networkidle");
    const trigger = page.getByRole("button", { name: "New issue" });
    await trigger.focus();
    await trigger.press("Enter");
    const dialog = page.getByRole("dialog", { name: "New issue" });
    await expect(dialog).toBeVisible();
    await expect(page.getByLabel("Title")).toBeFocused();
    await expectNoAxeViolations(page, "New issue dialog");
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(trigger).toBeFocused();

    await page.keyboard.press("ControlOrMeta+k");
    const command = page.getByRole("combobox", { name: /Search or run a command/ });
    await command.fill("Create issue");
    await command.press("Enter");
    await page.waitForURL("**/issues?new=1");
    await expect(page.getByRole("dialog", { name: "New issue" })).toBeVisible();
    await expect(page.getByLabel("Title")).toBeFocused();
  });

  test("a fresh organization creates its first project before capturing its first issue", async ({ page }) => {
    await setEdgeConfig({ emptyProjects: true, emptyIssues: true });
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await page.getByRole("button", { name: "New issue" }).click();

    const dialog = page.getByRole("dialog");
    await expect(dialog).toHaveAccessibleName("Set up issue tracking");
    await expect(dialog.getByLabel("Project name")).toBeFocused();
    await dialog.getByLabel("Project name").fill("Developer experience");
    await dialog.getByLabel("Issue key").fill("DX");
    await expect(dialog).toContainText("Issues will look like DX-1");
    await dialog.getByRole("button", { name: "Create project" }).click();

    await expect(dialog).toHaveAccessibleName("New issue");
    await expect(dialog.getByLabel("Title")).toBeFocused();
    await expect(dialog.getByLabel("Project")).toContainText("Developer experience — DX");
    await dialog.getByLabel("Title").fill("Make onboarding uneventful");
    await dialog.getByRole("button", { name: "Create issue" }).click();

    await expect(page.getByTestId("pending-issue")).toContainText(/^DX-/);
    await expect(page.getByText("Make onboarding uneventful")).toBeVisible();
    await expectNoAxeViolations(page, "first project and issue journey");
  });

  test("a lost project response can be retried without creating or selecting a duplicate", async ({ page }) => {
    await setEdgeConfig({
      emptyProjects: true,
      emptyIssues: true,
      projectCreateResponseLosses: 1,
      issueActivationPolls: 1,
    });
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await page.getByRole("button", { name: "New issue" }).click();

    const dialog = page.getByRole("dialog", { name: "Set up issue tracking" });
    await dialog.getByLabel("Project name").fill("Reliability");
    await dialog.getByLabel("Issue key").fill("REL");
    await dialog.getByRole("button", { name: "Create project" }).click();

    await expect(dialog.getByRole("alert")).toContainText("Retrying this unchanged draft is safe");
    await expect(dialog.getByLabel("Project name")).toHaveValue("Reliability");
    await dialog.getByRole("button", { name: "Create project" }).click();

    const issueDialog = page.getByRole("dialog", { name: "New issue" });
    await expect(issueDialog.getByLabel("Project").locator("option")).toHaveCount(1);
    await expect(issueDialog.getByLabel("Project")).toContainText("Reliability — REL");
    await issueDialog.getByLabel("Title").fill("Make uncertain delivery uneventful");
    await issueDialog.getByRole("button", { name: "Create issue" }).click();
    await expect(page.getByTestId("issue-row").filter({ hasText: "Make uncertain delivery uneventful" }))
      .toHaveCount(1);

    await page.reload();
    await waitForInteractiveShell(page);
    await page.getByRole("button", { name: "New issue" }).click();
    await expect(page.getByRole("dialog", { name: "New issue" }).getByLabel("Project").locator("option"))
      .toHaveCount(1);
  });

  test("a project created while pagination is loading restarts the shared catalogue", async ({ page, request }) => {
    await setEdgeConfig({ projectCount: 51, projectListCursorDelaysMs: [1_500] });
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await page.getByRole("button", { name: "New issue" }).click();

    const dialog = page.getByRole("dialog");
    await expect(dialog).toHaveAccessibleName("New issue");
    const projects = dialog.getByLabel("Project");
    await expect(projects.locator("option")).toHaveCount(50);
    await dialog.getByRole("button", { name: "Load more projects" }).click();
    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.projectListCursorRequests;
    }).toBe(1);

    await dialog.getByRole("button", { name: "New project" }).click();
    await expect(dialog).toHaveAccessibleName("New project");
    await dialog.getByLabel("Project name").fill("Catalogue integrity");
    await dialog.getByLabel("Issue key").fill("CAT");
    await dialog.getByRole("button", { name: "Create project" }).click();
    await expect(dialog).toHaveAccessibleName("New issue");
    await expect(projects).toContainText("Catalogue integrity — CAT");

    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.projectListCursorResponses;
    }).toBe(1);
    await expect(dialog.getByRole("button", { name: "Load more projects" })).toBeVisible();
    await expect(projects.locator("option").filter({ hasText: "Project 001" })).toHaveCount(0);

    await dialog.getByRole("button", { name: "Load more projects" }).click();
    await expect(projects.locator("option").filter({ hasText: "Project 001" })).toHaveCount(1);
    await expect(projects.locator("option").filter({ hasText: "Myelin — MYL" })).toHaveCount(1);
  });

  test("a lost issue response can be retried without creating a duplicate issue", async ({ page }) => {
    await setEdgeConfig({ emptyIssues: true, issueCreateResponseLosses: 1, issueActivationPolls: 1 });
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await page.getByRole("button", { name: "New issue" }).click();

    const dialog = page.getByRole("dialog", { name: "New issue" });
    await dialog.getByLabel("Title").fill("Retry one durable issue");
    await dialog.getByRole("button", { name: "Create issue" }).click();

    await expect(dialog.getByRole("alert")).toContainText("Retrying this unchanged draft is safe");
    await expect(dialog.getByLabel("Title")).toHaveValue("Retry one durable issue");
    await dialog.getByRole("button", { name: "Create issue" }).click();
    await expect(page.getByTestId("issue-row").filter({ hasText: "Retry one durable issue" })).toHaveCount(1);

    await page.reload();
    await waitForInteractiveShell(page);
    await expect(page.getByTestId("issue-row").filter({ hasText: "Retry one durable issue" })).toHaveCount(1);
  });

  test("project discovery failure never guesses an issue destination", async ({ page }) => {
    await setEdgeConfig({ projectsUnavailable: true });
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await page.getByRole("button", { name: "New issue" }).click();

    const dialog = page.getByRole("dialog", { name: "New issue" });
    await expect(dialog.getByRole("alert")).toContainText("No issue destination has been guessed");
    await expect(dialog.getByRole("button", { name: "Create issue" })).toHaveCount(0);
    await setEdgeConfig({ projectsUnavailable: false });
    await dialog.getByRole("button", { name: "Try again" }).click();
    await expect(dialog.getByLabel("Title")).toBeFocused();
  });

  test("202 create remains visibly pending until fresh polling observes active", async ({ page }) => {
    await setEdgeConfig({ issueActivationPolls: 3 });
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await page.waitForLoadState("networkidle");
    await page.getByRole("button", { name: "Load more" }).click();
    await expect(page.getByTestId("issue-row")).toHaveCount(51);
    await setEdgeConfig({ issueListFirstPageHolds: 1 });
    await page.getByRole("button", { name: "New issue" }).click();
    await page.getByLabel("Title").fill("Capture the browser rough edge");
    await page.getByRole("button", { name: "Create issue" }).click();

    const pending = page.getByTestId("pending-issue");
    await expect(pending).toContainText("Activating access");
    await expect(pending).not.toContainText("Capture the browser rough edge");
    await expect(page.getByText(/accepted — activating access/)).toBeVisible();

    await expect(page.getByText(/is ready/)).toBeVisible();
    await expect.poll(async () => (await edgeIssueState()).issueListFirstPageDelayedRequests).toBe(1);
    await expect(page.getByRole("button", { name: "Load more" })).toHaveCount(0);
    await expect(page.getByTestId("issue-row")).toHaveCount(50);

    await setEdgeConfig({ releaseIssueListFirstPages: true });
    await expect.poll(async () => (await edgeIssueState()).issueListFirstPageDelayedResponses).toBe(1);
    await expect(page.getByText("Capture the browser rough edge")).toBeVisible();
    await expect(pending).toHaveCount(0);
    await expect(page.getByTestId("issue-row")).toHaveCount(50);
    await page.getByRole("button", { name: "Load more" }).click();
    await expect(page.getByTestId("issue-row")).toHaveCount(52);
    const ids = await page.getByTestId("issue-row").evaluateAll((rows) =>
      rows.map((row) => row.getAttribute("href")),
    );
    expect(new Set(ids).size).toBe(52);

    await page.reload();
    await waitForInteractiveShell(page);
    await expect(page.getByTestId("issue-row")).toHaveCount(50);
    await page.getByRole("button", { name: "Load more" }).click();
    await expect(page.getByTestId("issue-row")).toHaveCount(52);
    const freshIds = await page.getByTestId("issue-row").evaluateAll((rows) =>
      rows.map((row) => row.getAttribute("href")),
    );
    expect(new Set(freshIds).size).toBe(52);
    expect(freshIds).toEqual(ids);
  });

  test("an unavailable activation confirmation is announced without inferring failure", async ({ page }) => {
    await setEdgeConfig({ issueActivationUnavailable: true });
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await page.getByRole("button", { name: "New issue" }).click();
    await page.getByLabel("Title").fill("Keep an honest activation status");
    await page.getByRole("button", { name: "Create issue" }).click();

    const status = page.getByTestId("pending-issue").getByRole("status");
    await expect(status).toHaveText(/Activation could not be confirmed/);
    await expect(page.getByTestId("pending-issue")).toContainText("No failure is inferred");
    await expect(page.getByTestId("pending-issue")).not.toContainText("safely pending");
  });

  test("issue create retries stay safe while ambiguous closes require a state check", async ({ page }) => {
    await setEdgeConfig({ issueCreateUnavailable: true });
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await page.getByRole("button", { name: "New issue" }).click();
    await page.getByLabel("Title").fill("Do not overclaim a failed create");
    await page.getByRole("button", { name: "Create issue" }).click();
    await expect(page.getByRole("alert")).toContainText(
      "We couldn't confirm whether the issue was created. Retrying this unchanged draft is safe.",
    );

    await setEdgeConfig({ issueCreateUnavailable: false, issueCloseUnavailable: true });
    await gotoInteractive(page, `/issues/${OPEN_ID}`);
    await page.getByRole("button", { name: "Close issue" }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Close issue" }).click();
    await expect(page.getByRole("alert")).toContainText(
      "We couldn't confirm whether this issue was closed. Refresh and check its current state before retrying.",
    );
    await expect(page.getByRole("alert")).not.toContainText("It has not been changed");
  });

  test("detail closes only after a safe-focus confirmation", async ({ page }) => {
    await devLogin(page);
    await gotoInteractive(page, `/issues/${OPEN_ID}`);
    await expect(page.getByRole("heading", { name: "Close the collaboration feedback loop" })).toBeVisible();
    await expectNoAxeViolations(page, "Issue detail");
    await page.getByRole("button", { name: "Close issue" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    await dialog.getByRole("button", { name: "Close issue" }).click();
    await expect(page.getByTitle("State: Done")).toBeVisible();
    await expect(page.getByRole("button", { name: "Close issue" })).toHaveCount(0);
    await expect(page.getByText("MYL-102 closed")).toBeVisible();
  });

  test("a late close response stays with the issue that was closed", async ({ page, request }) => {
    await setEdgeConfig({ issueCloseResponseDelaysMs: [1_500] });
    await devLogin(page);
    await gotoInteractive(page, `/issues/${OPEN_ID}`);
    await page.getByRole("button", { name: "Close issue" }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Close issue" }).click();
    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.issueCloseRequests;
    }).toBe(1);

    await page.evaluate((id) => {
      window.history.pushState({}, "", `/issues/${id}`);
      window.dispatchEvent(new PopStateEvent("popstate"));
    }, SECOND_OPEN_ID);
    await page.waitForURL(`**/issues/${SECOND_OPEN_ID}`);
    await expect(page.getByRole("heading", { name: "Verify encrypted issue titles" })).toBeVisible();

    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.issueCloseResponses;
    }).toBe(1);
    await expect(page.getByTitle("State: In progress")).toBeVisible();
    await expect(page.getByRole("button", { name: "Close issue" })).toBeVisible();
    await expect(page.getByText("MYL-102 closed")).toHaveCount(0);
  });

  test("empty, projection-unavailable, and leak-free not-available states stay distinct", async ({ page }) => {
    await setEdgeConfig({ emptyIssues: true });
    await devLogin(page);
    await gotoInteractive(page, "/issues?state=all");
    await expect(page.getByTestId("issues-empty")).toContainText("No issues yet");

    await setEdgeConfig({ issuesUnavailable: true });
    await page.reload();
    await expect(page.getByTestId("issues-error")).toContainText("authorization is catching up");

    await setEdgeConfig({ issuesUnavailable: false });
    await gotoInteractive(page, "/issues/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    await expect(page.getByTestId("issue-detail-error")).toContainText("not available to you");
  });

  test("an open-empty view never claims the tenant has no issues when closed work exists", async ({ page }) => {
    await setEdgeConfig({ onlyClosedIssues: true });
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await expect(page.getByTestId("issues-state-empty")).toContainText("No open issues");
    await expect(page.getByText("No issues yet")).toHaveCount(0);

    await page.getByRole("tab", { name: "Closed" }).click();
    await expect(page.getByText("Consolidate issue navigation")).toBeVisible();
  });

  test("375px layout retains key, title, state, and actions without horizontal overflow", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await devLogin(page);
    await gotoInteractive(page, "/issues");
    await expect(page.getByText("MYL-102", { exact: true })).toBeVisible();
    await expect(page.getByText("Close the collaboration feedback loop")).toBeVisible();
    await expect(page.getByTitle("State: Todo").first()).toBeVisible();
    await expect(page.getByRole("button", { name: "New issue" })).toBeVisible();
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(1);
    await expectNoAxeViolations(page, "Issues mobile layout");
  });
});
