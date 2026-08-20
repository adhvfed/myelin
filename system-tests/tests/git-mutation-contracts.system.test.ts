import { describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { GitProject } from "../src/git-project.js";
import { array, integer, record } from "../src/json.js";

async function expectBadRequest(
  path: string,
  body: Record<string, unknown>,
  client = systemClient,
): Promise<void> {
  const response = await client.json(path, { method: "POST", body, expectedStatus: 400 });
  expect(response.body).toMatchObject({ error: { code: "bad_request" } });
}

describe("Git mutation contracts", () => {
  test("ambiguous instructions never become durable work", async () => {
    const project = new GitProject(uniqueName("strict-git"), systemClient);

    await expectBadRequest("/v1/git/repos", { slug: project.slug, visibility: "private" });
    await systemClient.json(project.path, { expectedStatus: 404 });

    await project.create();
    await expectBadRequest(`${project.path}/blob/main/README.md`, {
      base_oid: "",
      contents: "# This commit must not exist.\n",
      commit_message: "A misspelled field Myelin must not ignore",
    });
    expect((await systemClient.json(project.path)).body).toMatchObject({ state: "empty" });

    await project.writeFile("main", "README.md", "# Unambiguous work\n", {
      message: "Explain the first durable change",
    });
    const blame = await systemClient.json(`${project.path}/blame/main/README.md`);
    const firstHunk = record(
      array(blame.body.hunks, "README blame hunks")[0],
      "README blame hunk",
    );
    expect(record(firstHunk.commit, "README blame commit")).toMatchObject({
      summary: "Explain the first durable change",
    });
    const feature = await project.writeFile(
      "strict-input",
      "src/strict.ts",
      "export const intent = 'explicit';\n",
      { startRef: "main" },
    );
    const pullRequestsPath = `${project.path}/prs`;
    const proposal = {
      title: "Keep mutation intent explicit",
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/strict-input",
      head_oid: feature.commitOid,
      reviewers: [systemTestConfig.reviewerPrincipal],
    };

    await expectBadRequest(pullRequestsPath, { ...proposal, draft: "false" });
    expect(
      array((await systemClient.json(pullRequestsPath)).body.items, "pull requests"),
    ).toEqual([]);

    const opened = await systemClient.json(pullRequestsPath, {
      method: "POST",
      body: { ...proposal, draft: false },
      expectedStatus: 201,
    });
    const pullRequest = record(record(opened.body.applied, "open receipt").pr, "pull request");
    const pullRequestNumber = integer(pullRequest.number, "pull request number");
    const pullRequestPath = `${pullRequestsPath}/${pullRequestNumber}`;

    await expectBadRequest(
      `${pullRequestPath}/reviews`,
      { verdict: "approve", publish: true },
      reviewerClient,
    );
    expect((await systemClient.json(`${pullRequestPath}/checks`)).body).toMatchObject({
      current_approvals: 0,
    });

    await expectBadRequest(`${pullRequestPath}/merge`, { force: true });
    expect((await systemClient.json(pullRequestPath)).body).toMatchObject({ pr_state: "open" });
  });
});
