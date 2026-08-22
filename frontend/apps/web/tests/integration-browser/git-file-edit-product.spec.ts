import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";

import {
  browserApprovedSession,
  integrationEdgeUrl,
  postProductJson,
  type JsonObject,
} from "./product-api";
import { signIn, waitForAppHydration } from "./session";

test("an engineer keeps their draft when a teammate edits the file first", async ({
  page,
  request,
}) => {
  const slug = `concurrent-edit-${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const sessionToken = await browserApprovedSession(request);
  const owner = { token: sessionToken, scheme: "session" };
  const filePath = `/v1/git/repos/${encodeURIComponent(slug)}/blob/main/README.md`;

  await signIn(page);
  await page.getByRole("button", { name: "New repository" }).click();
  const repository = page.getByRole("dialog", { name: "New repository" });
  await repository.getByLabel("Name or namespace/name").fill(slug);
  await repository.getByRole("button", { name: "Create repository" }).click();

  await page.getByRole("button", { name: "Create first file" }).click();
  const firstFile = page.getByRole("dialog", { name: "Create file" });
  await firstFile.getByRole("textbox", { name: "File path" }).fill("README.md");
  await firstFile.getByRole("textbox", { name: "File contents" }).fill("# Shared plan\n");
  await firstFile.getByRole("textbox", { name: "Commit message" }).fill("Start the shared plan");
  await firstFile.getByRole("button", { name: "Commit file" }).click();
  await expect(page.getByRole("heading", { name: "README.md" })).toBeVisible();

  await page.getByRole("button", { name: "Edit file" }).click();
  const editor = page.getByRole("dialog", { name: "Edit README.md" });
  const draft = "# Shared plan\n\nOwner draft that must not be lost.\n";
  await editor.getByRole("textbox", { name: "File contents" }).fill(draft);
  await editor.getByRole("textbox", { name: "Commit message" }).fill("Finish the shared plan");

  const current = await request.get(`${integrationEdgeUrl}${filePath}`, {
    headers: {
      authorization: `Bearer ${sessionToken}`,
      "x-myelin-token-scheme": "session",
    },
  });
  const currentText = await current.text();
  expect(current.status(), currentText).toBe(200);
  const baseOid = String((JSON.parse(currentText) as JsonObject).base_oid);
  expect(baseOid).toMatch(/^[0-9a-f]{40}$/);

  const teammateVersion = "# Shared plan\n\nA teammate committed first.\n";
  await postProductJson(request, filePath, {
    base_oid: baseOid,
    contents: teammateVersion,
    message: "Record the teammate decision",
  }, { ...owner, status: 200 });

  await editor.getByRole("button", { name: "Commit changes" }).click();
  await expect(editor.getByRole("alert"))
    .toHaveText("Another edit landed first. Reload the file before editing it again.");
  await expect(editor.getByRole("textbox", { name: "File contents" })).toHaveValue(draft);

  await editor.getByRole("button", { name: "Cancel" }).click();
  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByTestId("blob-contents")).toContainText("A teammate committed first.");
  await expect(page.getByTestId("blob-contents")).not.toContainText("Owner draft");
});
