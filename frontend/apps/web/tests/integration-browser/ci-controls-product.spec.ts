import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";

import { browserApprovedSession, integrationEdgeUrl, postProductJson } from "./product-api";
import { navigateToApp, signIn } from "./session";

const runnerImage =
  "myelin.local/linux-small-v1-rootfs@sha256:65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";

test("an engineer stops a long CI run and re-runs it from the run page", async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  const slug = `ci-controls-${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const owner = { token: await browserApprovedSession(request), scheme: "session" };
  const repoPath = `/v1/git/repos/${encodeURIComponent(slug)}`;

  await signIn(page);
  await page.getByRole("button", { name: "New repository" }).click();
  const repository = page.getByRole("dialog", { name: "New repository" });
  await repository.getByLabel("Name or namespace/name").fill(slug);
  await repository.getByRole("button", { name: "Create repository" }).click();
  await expect(page.getByRole("button", { name: "Create first file" })).toBeVisible();

  await postProductJson(request, `${repoPath}/blob/main/.myelin/ci.toml`, {
    base_oid: "",
    contents: `schema_version = 2
on = "push"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "soak"
image = "${runnerImage}"
command = ["sh", "-c", "printf 'soaking\\\\n'; sleep 120"]
`,
    message: "Start a long soak test",
  }, { ...owner, status: 200 });

  await navigateToApp(page, "/ci");
  const row = page.getByTestId("ci-run-row").filter({ hasText: slug });
  await expect(row).toContainText("Running", { timeout: 60_000 });
  const runId = (await row.getAttribute("href"))?.split("/").at(-1) ?? "";
  expect(runId).toMatch(/^[0-9a-f-]{36}$/);
  await row.click();
  await expect(page).toHaveURL(new RegExp(`/ci/runs/${runId}$`));

  await expect(page.getByRole("button", { name: "Re-run" })).toHaveCount(0);
  await page.getByRole("button", { name: "Cancel run" }).click();
  const confirm = page.getByRole("alertdialog");
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "Keep running" }).click();
  await expect(confirm).toBeHidden();

  await page.getByRole("button", { name: "Cancel run" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: /^Cancel run$/ }).click();
  await expect(page.getByRole("button", { name: "Re-run" })).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole("button", { name: "Cancel run" })).toHaveCount(0);

  const detail = await request.get(`${integrationEdgeUrl}/v1/ci/runs/${runId}`, {
    headers: { authorization: `Bearer ${owner.token}`, "x-myelin-token-scheme": owner.scheme },
  });
  expect(detail.status()).toBe(200);
  const body = await detail.json() as { run: { state: string }; controls: unknown };
  expect(body.run.state).toBe("cancelled");
  expect(body.controls).toEqual({ retry: true, cancel: false });

  await page.getByRole("button", { name: "Re-run" }).click();
  await expect(page).not.toHaveURL(new RegExp(`/ci/runs/${runId}$`), { timeout: 30_000 });
  const retriedId = page.url().split("/").at(-1) ?? "";
  expect(retriedId).toMatch(/^[0-9a-f-]{36}$/);
  await expect(page.getByRole("heading", { name: `Run ${retriedId.slice(0, 8)}` })).toBeVisible();
  await expect(page.getByText("manual", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Cancel run" }).click({ timeout: 60_000 });
  await page.getByRole("alertdialog").getByRole("button", { name: /^Cancel run$/ }).click();
  await expect(page.getByRole("button", { name: "Re-run" })).toBeVisible({ timeout: 30_000 });
});
