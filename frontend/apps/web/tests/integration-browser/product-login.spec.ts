import { randomUUID } from "node:crypto";
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

async function waitForCiRun(
  request: APIRequestContext,
  repoRef: string,
  commitOid: unknown,
): Promise<JsonObject> {
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    const runs = await edgeRequest(request, "GET", "/v1/ci/runs?state=all&limit=50", 200);
    expect(Array.isArray(runs.items)).toBe(true);
    const match = (runs.items as unknown[]).find((value) => {
      if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
      const row = value as JsonObject;
      return row.repo_ref === repoRef && row.commit_oid === commitOid;
    });
    if (match !== undefined) return object(match);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`CI run for ${repoRef} at ${String(commitOid)} did not appear within 15 seconds`);
}

test("durable product data is available and mutable after browser login", async ({
  page,
  request,
}) => {
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

  await edgeRequest(request, "POST", `${repoPath}/blob/main/README.md`, 200, {
    base_oid: "",
    contents: `# ${slug}\n\nCreated through the running product.\n`,
  });
  const pipelineCommit = await edgeRequest(
    request,
    "POST",
    `${repoPath}/blob/main/.myelin/ci.toml`,
    200,
    {
      base_oid: "",
      contents: `on = "push"

[[jobs]]
name = "test"
image = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000"
command = ["true"]
`,
    },
  );
  const pipelineOid = object(pipelineCommit.applied).new_oid;
  expect(pipelineOid).toMatch(/^[0-9a-f]{40}$/);
  const ciRun = await waitForCiRun(
    request,
    `myelin://${tenant}/git/repo/${slug}`,
    pipelineOid,
  );
  const ciRunId = String(ciRun.run_id);
  expect(ciRun).toMatchObject({ trigger_kind: "push", state: "queued" });
  expect(ciRunId).toMatch(/^[0-9a-f-]{36}$/);

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
  await expect(page.getByText("Created through the running product.")).toBeVisible();

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
  await expect(ciRow).toContainText("Queued");
  await expect(ciRow).toContainText("push");
  await ciRow.click();
  await expect(page).toHaveURL(new RegExp(`/ci/runs/${ciRunId}$`));
  await expect(page.getByRole("heading", { name: `Run ${ciRunId.slice(0, 8)}` })).toBeVisible();
  await expect(page.getByText(String(pipelineOid), { exact: true })).toBeVisible();
  await expect(page.getByTestId("ci-jobs-empty")).toBeVisible();

  await navigateToApp(page, "/issues");
  const issueTitle = `Track ${slug}`;
  await page.getByRole("button", { name: "New issue" }).click();
  const issueDialog = page.getByRole("dialog", { name: "New issue" });
  await issueDialog.getByLabel("Title").fill(issueTitle);
  await issueDialog.getByRole("button", { name: "Create issue" }).click();

  const issueRow = page.getByTestId("issue-row").filter({ hasText: issueTitle });
  await expect(issueRow).toBeVisible({ timeout: 20_000 });
  const issueKey = (await issueRow.locator("code").textContent())?.trim();
  expect(issueKey).toMatch(/^MYL-\d+$/);
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
