import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";

import { browserApprovedSession, postProductJson, type JsonObject } from "./product-api";
import { navigateToApp, signIn } from "./session";

test("an author readies a draft, closes it, and reopens it from the pull request page", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  const slug = `pr-lifecycle-${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const owner = { token: await browserApprovedSession(request), scheme: "session" };
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
  await write("main", "README.md");
  const head = await write("draft/notes", "notes.txt", "main");
  const opened = await postProductJson(request, `${repoPath}/prs`, {
    title: "Sketch the release notes",
    base_ref: "refs/heads/main",
    head_ref: "refs/heads/draft/notes",
    head_oid: head,
    draft: true,
  }, { ...owner, status: 201 }) as { applied: { pr: { number: number } } };
  const number = opened.applied.pr.number;

  await navigateToApp(page, `/git/repos/${encodeURIComponent(slug)}/prs/${number}`);
  const card = page.getByTestId("merge-card");
  await expect(card.getByTestId("draft-notice")).toContainText("This is a draft.");
  await expect(card.getByRole("button", { name: "Merge pull request" })).toHaveCount(0);

  await card.getByRole("button", { name: "Ready for review" }).click();
  await expect(card.getByTestId("draft-notice")).toHaveCount(0);
  await expect(card.getByRole("button", { name: "Close pull request" })).toBeVisible();

  await card.getByRole("button", { name: "Close pull request" }).click();
  await expect(page.getByText("Pull request closed")).toBeVisible();
  await expect(card.getByTestId("merge-terminal")).toContainText("This pull request is closed. Closed by");
  await expect(card.getByRole("button", { name: "Merge pull request" })).toHaveCount(0);

  await card.getByRole("button", { name: "Reopen pull request" }).click();
  await expect(page.getByText("Pull request reopened")).toBeVisible();
  await expect(card.getByTestId("merge-terminal")).toHaveCount(0);
  await expect(card.getByRole("button", { name: "Close pull request" })).toBeVisible();
});
