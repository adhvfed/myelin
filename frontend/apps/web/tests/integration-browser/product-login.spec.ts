import { randomUUID } from "node:crypto";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type APIRequestContext } from "@playwright/test";
import { navigateToApp, signIn } from "./session";

type JsonObject = Record<string, unknown>;

function requiredEnvironment(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required; run this test with fed test:integration`);
  return value;
}

function object(value: unknown): JsonObject {
  expect(typeof value).toBe("object");
  expect(value).not.toBeNull();
  expect(Array.isArray(value)).toBe(false);
  return value as JsonObject;
}

const edgeUrl = requiredEnvironment("MYELIN_INTEGRATION_EDGE_URL").replace(/\/$/, "");
const token = requiredEnvironment("MYELIN_BROWSER_EDGE_TOKEN");
const tenant = requiredEnvironment("MYELIN_BROWSER_TENANT");
const principal = requiredEnvironment("MYELIN_BROWSER_PRINCIPAL");
const runnerImage =
  "myelin.local/linux-small-v1-rootfs@sha256:65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";

async function edgeRequest(
  request: APIRequestContext,
  method: "GET" | "POST",
  path: string,
  expectedStatus: number | readonly number[],
  body?: unknown,
): Promise<JsonObject> {
  const response = await request.fetch(`${edgeUrl}${path}`, {
    method,
    headers: {
      accept: "application/json",
      authorization: `Bearer ${token}`,
      "x-myelin-token-scheme": "agent",
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...(method === "POST" ? { "idempotency-key": randomUUID() } : {}),
    },
    data: body,
    failOnStatusCode: false,
  });
  const text = await response.text();
  const acceptedStatuses = Array.isArray(expectedStatus) ? expectedStatus : [expectedStatus];
  expect(
    acceptedStatuses,
    `${method} ${path} returned ${response.status()}: ${text}`,
  ).toContain(
    response.status(),
  );
  return object(JSON.parse(text));
}

test("durable product data is available and mutable after browser login", async ({
  page,
  request,
}) => {
  test.setTimeout(90_000);
  const slug = `browser-${Date.now().toString(36)}-${randomUUID().slice(0, 8)}`;
  const repoPath = `/v1/git/repos/${encodeURIComponent(slug)}`;

  await signIn(page);
  await expect(page).toHaveTitle("Code · Myelin");
  await expect(page.getByRole("heading", { name: "Repositories" })).toBeVisible();
  await expect(page.getByText("Myelin Developer", { exact: true })).toBeVisible();
  await expect(page.getByText("fr-par", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.cookie)).not.toContain("myelin_session");

  await page.getByRole("button", { name: "New repository" }).click();
  const createDialog = page.getByRole("dialog", { name: "New repository" });
  await createDialog.getByLabel("Name or namespace/name").fill(slug);
  await createDialog.getByRole("button", { name: "Create repository" }).click();
  await page.waitForURL(`**/git/repos/${slug}`);
  await expect(page.getByRole("heading", { name: `${tenant}/${slug}` })).toBeVisible();
  await expect(page.getByRole("button", { name: "Copy reference" }))
    .toHaveAttribute("title", `myelin://${tenant}/git/repo/${slug}`);
  const gitSetup = page.getByTestId("git-setup");
  await gitSetup.getByText("Set up Git").click();
  await expect(gitSetup).toContainText(`${principal}@${tenant}.noreply`);
  await expect(gitSetup.getByTestId("git-setup-commands"))
    .toContainText(`myelin --edge '${edgeUrl}' auth login`);
  await expect(gitSetup.getByTestId("git-setup-commands")).toContainText("myelin auth configure-git");
  await expect(gitSetup.getByTestId("git-setup-commands")).toContainText("git push -u origin 'main'");

  await page.getByRole("button", { name: "Create first file" }).click();
  const firstFile = page.getByRole("dialog", { name: "Create file" });
  await firstFile.getByRole("textbox", { name: "File path" }).fill("README.md");
  await firstFile.getByRole("textbox", { name: "File contents" })
    .fill(`# ${slug}\n\nCreated through the running product.\n`);
  await firstFile.getByRole("textbox", { name: "Commit message" }).fill("Start the repository");
  const editorAccessibility = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(editorAccessibility.violations).toEqual([]);
  await firstFile.getByRole("button", { name: "Commit file" }).click();
  await expect(page.getByRole("heading", { name: "README.md" })).toBeVisible();
  await expect(page.getByLabel("File contents"))
    .toContainText("Created through the running product.");

  await page.getByRole("button", { name: "Edit file" }).click();
  const readmeEdit = page.getByRole("dialog", { name: "Edit README.md" });
  await expect(readmeEdit.getByRole("textbox", { name: "File path" })).toHaveValue("README.md");
  await readmeEdit.getByRole("textbox", { name: "File contents" })
    .fill(`# ${slug}\n\nCreated and edited through the running product.\n`);
  await readmeEdit.getByRole("textbox", { name: "Commit message" }).fill("Clarify the README");
  await readmeEdit.getByRole("button", { name: "Commit changes" }).click();
  await expect(readmeEdit).toBeHidden();
  await expect(page.getByTestId("blob-contents"))
    .toContainText("Created and edited through the running product.");

  await navigateToApp(page, `/git/repos/${slug}`);
  await page.getByRole("button", { name: "New file" }).click();
  const pipelineFile = page.getByRole("dialog", { name: "Create file" });
  await pipelineFile.getByRole("textbox", { name: "File path" }).fill(".myelin/ci.toml");
  await pipelineFile.getByRole("textbox", { name: "File contents" }).fill(`schema_version = 2
on = "push"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "test"
image = "${runnerImage}"
command = ["true"]
`);
  await pipelineFile.getByRole("textbox", { name: "Commit message" }).fill("Run the first check");
  await pipelineFile.getByRole("button", { name: "Commit file" }).click();
  await expect(page.getByRole("heading", { name: ".myelin/ci.toml" })).toBeVisible();

  const featureCommit = await edgeRequest(
    request,
    "POST",
    `${repoPath}/blob/feature/app.txt`,
    200,
    { base_oid: "", contents: "export const ready = true;\n" },
  );
  const featureOid = object(featureCommit.applied).new_oid;
  expect(featureOid).toMatch(/^[0-9a-f]{40}$/);
  await edgeRequest(request, "POST", `${repoPath}/prs`, 201, {
    title: "Ship the browser journey",
    base_ref: "refs/heads/main",
    head_ref: "refs/heads/feature",
    head_oid: featureOid,
  });

  await navigateToApp(page, "/git/repos");
  await page.getByRole("button", { name: /Search or run a command/ }).click();
  await page.getByRole("combobox", { name: /Search or run a command/ }).fill(slug);
  await page.keyboard.press("Enter");
  await page.waitForURL(/\/git\/search\?q=/);
  const searchResult = page.getByTestId("code-search-results").getByRole("link").first();
  await expect(searchResult).toContainText(slug);
  await expect(searchResult).toContainText("README.md:1");
  await expect(searchResult).toContainText(`# ${slug}`);
  await searchResult.click();
  await expect(page).toHaveURL(/\/blob\/refs%2Fheads%2Fmain\/README\.md#L1$/);
  await expect(page.locator("#L1")).toContainText(`# ${slug}`);

  await navigateToApp(page, "/git/repos");
  await page.getByRole("link", { name: new RegExp(`${tenant}/${slug}`) }).click();
  await expect(page.getByRole("heading", { name: `${tenant}/${slug}` })).toBeVisible();
  await expect(page.getByText("Created and edited through the running product.")).toBeVisible();

  await page.getByRole("link", { name: "Pull requests" }).click();
  const review = page.getByTestId("pr-row").filter({ hasText: "Ship the browser journey" });
  await expect(review).toBeVisible();
  await expect(review).toContainText("feature");
  await expect(review).toContainText("main");
  await review.click();

  await expect(page.getByRole("heading", { name: "Ship the browser journey #1" })).toBeVisible();
  await expect(page.getByText("refs/heads/feature")).toBeVisible();
  await expect(page.getByText("refs/heads/main")).toBeVisible();

  await navigateToApp(page, "/ci");
  const ciRow = page.getByTestId("ci-run-row").filter({ hasText: slug });
  await expect(ciRow).toContainText("Succeeded", { timeout: 60_000 });
  await expect(ciRow).toContainText("push");
  const ciRunId = (await ciRow.getAttribute("href"))?.split("/").at(-1) ?? "";
  expect(ciRunId).toMatch(/^[0-9a-f-]{36}$/);
  await ciRow.click();
  await expect(page).toHaveURL(new RegExp(`/ci/runs/${ciRunId}$`));
  await expect(page.getByRole("heading", { name: `Run ${ciRunId.slice(0, 8)}` })).toBeVisible();
  await expect(page.getByText(/^[0-9a-f]{40}$/, { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { level: 3, name: "test" })).toBeVisible();
  await expect(page.getByTestId("ci-job-result")).toContainText("Workload passed");

  await navigateToApp(page, "/issues");
  const issuePrefix = `B${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`;
  const issueTitle = `Track ${slug}`;
  await page.getByRole("button", { name: "New issue" }).click();
  const issueDialog = page.getByRole("dialog");
  const firstProjectSetup = issueDialog.getByText("Issues live in projects");
  if (!await firstProjectSetup.isVisible().catch(() => false)) {
    await issueDialog.getByRole("button", { name: "New project" }).click();
  }
  await issueDialog.getByLabel("Project name").fill(`Browser ${slug}`);
  await issueDialog.getByLabel("Issue key").fill(issuePrefix);
  await issueDialog.getByRole("button", { name: "Create project" }).click();
  await expect(issueDialog.getByLabel("Project")).toHaveValue(/^[0-9a-f-]{36}$/);
  await issueDialog.getByLabel("Title").fill(issueTitle);
  await issueDialog.getByRole("button", { name: "Create issue" }).click();

  const issueRow = page.getByTestId("issue-row").filter({ hasText: issueTitle });
  await expect(issueRow).toBeVisible({ timeout: 20_000 });
  const issueKey = (await issueRow.locator("code").textContent())?.trim();
  expect(issueKey).toMatch(new RegExp(`^${issuePrefix}-\\d+$`));
  await issueRow.click();
  await expect(page.getByRole("heading", { name: issueTitle })).toBeVisible();
  const issueId = decodeURIComponent(new URL(page.url()).pathname.split("/").at(-1) ?? "");
  expect(issueId).toMatch(/^[0-9a-f-]{36}$/);

  await page.getByRole("button", { name: "Close issue" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Close issue" }).click();
  await expect(page.getByRole("button", { name: "Close issue" })).toHaveCount(0);
  await expect(page.getByText(`${issueKey} closed`)).toBeVisible();

  const closedIssue = await edgeRequest(
    request,
    "GET",
    `/v1/issues/${encodeURIComponent(issueId)}`,
    200,
  );
  expect(closedIssue).toMatchObject({ id: issueId, key: issueKey, state_category: "completed" });
});
