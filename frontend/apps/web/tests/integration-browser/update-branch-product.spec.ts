import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";

import {
  browserApprovedSession,
  integrationReviewerPrincipal,
  integrationReviewerToken,
  postProductJson,
  type JsonObject,
} from "./product-api";
import { navigateToApp, signIn } from "./session";

test("a reviewer's approval survives updating a branch another merge left behind", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  const slug = `update-branch-${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const owner = { token: await browserApprovedSession(request), scheme: "session" };
  const reviewer = { token: integrationReviewerToken, scheme: "agent" };
  const repoPath = `/v1/git/repos/${encodeURIComponent(slug)}`;

  await signIn(page);
  await page.getByRole("button", { name: "New repository" }).click();
  const repository = page.getByRole("dialog", { name: "New repository" });
  await repository.getByLabel("Name or namespace/name").fill(slug);
  await repository.getByRole("button", { name: "Create repository" }).click();
  await expect(page.getByRole("button", { name: "Create first file" })).toBeVisible();

  const write = async (branch: string, path: string, startRef?: string) =>
    String((await postProductJson(request, `${repoPath}/blob/${encodeURIComponent(branch)}/${path}`, {
      base_oid: "",
      contents: `${path}\n`,
      ...(startRef ? { start_ref: startRef } : {}),
    }, { ...owner, status: 200 }) as { applied: JsonObject }).applied.new_oid);
  const propose = async (branch: string, path: string) => {
    const head = await write(branch, path, "main");
    const opened = await postProductJson(request, `${repoPath}/prs`, {
      title: `Add ${path}`,
      base_ref: "refs/heads/main",
      head_ref: `refs/heads/${branch}`,
      head_oid: head,
      reviewers: [integrationReviewerPrincipal],
    }, { ...owner, status: 201 }) as { applied: { pr: { number: number } } };
    const number = opened.applied.pr.number;
    await postProductJson(request, `${repoPath}/prs/${number}/reviews`, { verdict: "approve" }, {
      ...reviewer,
      status: 200,
    });
    return number;
  };
  await write("main", "README.md");
  const first = await propose("feature/first", "first.txt");
  const second = await propose("feature/second", "second.txt");
  await postProductJson(request, `${repoPath}/prs/${first}/merge`, {}, { ...owner, status: 200 });

  await navigateToApp(page, `/git/repos/${encodeURIComponent(slug)}/prs/${second}`);
  const behind = page.getByTestId("branch-behind");
  await expect(behind).toContainText("main has moved on.");
  await expect(page.getByRole("button", { name: "Merge pull request" })).toHaveCount(0);

  await behind.getByRole("button", { name: "Update branch" }).click();
  await expect(page.getByText("Merged main into this branch")).toBeVisible();
  await expect(behind).toHaveCount(0);
  // Main requires one approval, so the gate opening proves the reviewer's approval survived.
  await expect(page.getByTestId("merge-ready")).toBeVisible();
  await expect(page.getByTestId("merge-blocked")).toHaveCount(0);
  await page.getByRole("button", { name: "Merge pull request" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Merge" }).click();
  await expect(page.getByTestId("merge-terminal")).toContainText("This pull request is merged.");
});
