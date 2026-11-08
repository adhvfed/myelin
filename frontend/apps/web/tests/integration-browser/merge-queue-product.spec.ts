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

const runnerImage =
  "myelin.local/linux-small-v1-rootfs@sha256:65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";

test("an approved pull request scheduled to merge when ready lands once CI passes", async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  const slug = `merge-queue-${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const owner = { token: await browserApprovedSession(request), scheme: "session" };
  const repoPath = `/v1/git/repos/${encodeURIComponent(slug)}`;

  await signIn(page);
  await page.getByRole("button", { name: "New repository" }).click();
  const repository = page.getByRole("dialog", { name: "New repository" });
  await repository.getByLabel("Name or namespace/name").fill(slug);
  await repository.getByRole("button", { name: "Create repository" }).click();
  await expect(page.getByRole("button", { name: "Create first file" })).toBeVisible();

  await postProductJson(request, `${repoPath}/branch-protection`, {
    rulesets: [{
      ref_pattern: "refs/heads/main",
      required_contexts: ["ci/contract"],
      required_approvals: 1,
      require_codeowner_review: false,
      require_conversation_resolution: false,
      allow_force_push: false,
    }],
  }, { ...owner, status: 200 });
  const write = async (branch: string, path: string, contents: string, startRef?: string) =>
    String((await postProductJson(request, `${repoPath}/blob/${encodeURIComponent(branch)}/${path}`, {
      base_oid: "",
      contents,
      ...(startRef ? { start_ref: startRef } : {}),
    }, { ...owner, status: 200 }) as { applied: JsonObject }).applied.new_oid);
  await write("main", "README.md", "# queue\n");
  await write("main", ".myelin/ci.toml", `schema_version = 2
on = "pull_request"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "contract"
image = "${runnerImage}"
command = ["sh", "-c", "sleep 8"]
`);
  const head = await write("feature/queued", "queued.txt", "queued\n", "main");
  const opened = await postProductJson(request, `${repoPath}/prs`, {
    title: "Queue me",
    base_ref: "refs/heads/main",
    head_ref: "refs/heads/feature/queued",
    head_oid: head,
    reviewers: [integrationReviewerPrincipal],
  }, { ...owner, status: 201 }) as { applied: { pr: { number: number } } };
  const number = opened.applied.pr.number;
  await postProductJson(request, `${repoPath}/prs/${number}/reviews`, { verdict: "approve" }, {
    token: integrationReviewerToken,
    scheme: "agent",
    status: 200,
  });

  // The required check is still running, so the pull request cannot merge yet.
  await navigateToApp(page, `/git/repos/${encodeURIComponent(slug)}/prs/${number}`);
  await page.getByRole("button", { name: "Merge when ready" }).click();
  await expect(page.getByText("Will merge automatically when ready")).toBeVisible();
  await expect(page.getByTestId("merge-terminal"))
    .toContainText("This pull request is merged.", { timeout: 150_000 });
});
