import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";

import { browserApprovedSession, postProductJson, type JsonObject } from "./product-api";
import { navigateToApp, signIn } from "./session";

test("an author retitles a pull request and writes its description on the page", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  const slug = `pr-edit-${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
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
  const head = await write("notes", "notes.txt", "main");
  const opened = await postProductJson(request, `${repoPath}/prs`, {
    title: "Notes",
    base_ref: "refs/heads/main",
    head_ref: "refs/heads/notes",
    head_oid: head,
  }, { ...owner, status: 201 }) as { applied: { pr: { number: number } } };
  const number = opened.applied.pr.number;

  await navigateToApp(page, `/git/repos/${encodeURIComponent(slug)}/prs/${number}`);
  const description = page.getByTestId("pr-description");
  await expect(description).toContainText("No description.");
  await description.getByRole("button", { name: "Edit title and description" }).click();
  const form = page.getByRole("form", { name: "Edit pull request" });
  await form.getByLabel("Title").fill("Release notes for the merge queue");
  await form.getByLabel("Description").fill("Explains **merge when ready** for people and agents.");
  await form.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText("Saved", { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Release notes for the merge queue");
  await expect(description.locator("strong")).toHaveText("merge when ready");

  await page.reload();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Release notes for the merge queue");
  await expect(page.getByTestId("pr-description").locator("strong")).toHaveText("merge when ready");
});
