import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { systemTestConfig } from "../src/config.js";
import { GitProject } from "../src/git-project.js";
import { awaitActiveIssue } from "../src/journeys/issues.js";
import { record, string } from "../src/json.js";

describe("merging closes the issues a pull request names", () => {
  test("a Closes line in the description closes the issue when the pull request merges", async () => {
    const issue = await awaitActiveIssue(systemClient, uniqueName("Fix the onboarding typo"));
    const key = string(issue.key, "issue key");
    const viewIssue = async () =>
      record((await systemClient.json(`/v1/issues/${encodeURIComponent(key)}`)).body, "issue");
    expect(await viewIssue()).toMatchObject({ state_category: expect.not.stringMatching("completed") });

    const project = new GitProject(uniqueName("closes-issue"), systemClient);
    await project.create();
    await project.writeFile("main", "README.md", "# Closes\n");
    const head = (await project.writeFile("fix/typo", "typo.txt", "fixed\n", {
      startRef: "main",
    })).commitOid;
    const opened = await systemClient.json(`${project.path}/prs`, {
      method: "POST",
      body: {
        title: "Fix the onboarding typo",
        body_md: `The typo is gone.\n\nCloses ${key}`,
        base_ref: "refs/heads/main",
        head_ref: "refs/heads/fix/typo",
        head_oid: head,
        reviewers: [systemTestConfig.reviewerPrincipal],
      },
      expectedStatus: 201,
    });
    const number = Number(record(record(opened.body.applied, "opened").pr, "pr").number);
    await reviewerClient.json(`${project.path}/prs/${number}/reviews`, {
      method: "POST",
      body: { verdict: "approve" },
    });
    expect(await viewIssue()).toMatchObject({ state_category: expect.not.stringMatching("completed") });

    await systemClient.json(`${project.path}/prs/${number}/merge`, { method: "POST", body: {} });
    expect(await viewIssue()).toMatchObject({ key, state_category: "completed" });
  }, 120_000);
});
