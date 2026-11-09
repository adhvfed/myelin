import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { systemTestConfig } from "../src/config.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { ciRunsMatching } from "../src/journeys/ci-runs.js";
import { findInboxItemMatching, pullRequestSubject } from "../src/journeys/inbox.js";
import { proposeChange, type OpenedPullRequest } from "../src/journeys/pull-requests.js";
import { array, record, string, type JsonRecord } from "../src/json.js";

const runnerImage =
  "myelin.local/linux-small-v1-rootfs@sha256:65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";

const pipeline = (command: string) => `schema_version = 2
on = "pull_request"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "contract"
image = "${runnerImage}"
command = ${command}
`;

async function queueWithRequiredCheck(prefix: string, command: string): Promise<GitProject> {
  const project = new GitProject(uniqueName(prefix), systemClient);
  await project.create();
  await systemClient.json(`${project.path}/branch-protection`, {
    method: "POST",
    body: {
      rulesets: [{
        ref_pattern: "refs/heads/main",
        required_contexts: ["ci/contract"],
        required_approvals: 1,
        require_codeowner_review: false,
        require_conversation_resolution: false,
        allow_force_push: false,
      }],
    },
  });
  await project.writeFile("main", "README.md", "# queue\n");
  await project.writeFile("main", ".myelin/ci.toml", pipeline(command));
  return project;
}

async function approvedChange(
  project: GitProject,
  branch: string,
  path: string,
): Promise<OpenedPullRequest> {
  const change = await proposeChange(systemClient, project, {
    branch,
    path,
    contents: `${path}\n`,
    title: `Add ${path}`,
    commitMessage: `Add ${path}`,
    reviewers: [systemTestConfig.reviewerPrincipal],
  });
  await reviewerClient.json(`${project.path}/prs/${change.number}/reviews`, {
    method: "POST",
    body: { verdict: "approve" },
  });
  return change;
}

async function pullRequest(project: GitProject, number: number): Promise<JsonRecord> {
  return record((await systemClient.json(`${project.path}/prs/${number}`)).body, "pull request");
}

describe("the merge queue", () => {
  test("lands approved pull requests one at a time, each tested on top of the last", async () => {
    const project = await queueWithRequiredCheck("merge-queue", `["true"]`);
    const first = await approvedChange(project, "feature/first", "first.txt");
    const second = await approvedChange(project, "feature/second", "second.txt");

    for (const change of [first, second]) {
      const queued = await systemClient.json(`${project.path}/prs/${change.number}/auto-merge`, {
        method: "POST",
        body: {},
      });
      expect(queued.body).toMatchObject({
        applied: {
          action: "git.pr.auto_merge.request",
          auto_merge: { requested_by: `${systemTestConfig.principal}@${systemTestConfig.tenant}.noreply` },
        },
      });
    }

    const merged = await eventually(async () => {
      const states = await Promise.all([first, second].map((change) =>
        pullRequest(project, change.number)));
      return states.every((state) => state.pr_state === "merged") ? states : undefined;
    }, { description: "both queued pull requests to merge", timeoutMs: 150_000 });

    const [, secondMerged] = merged;
    const finalHead = string(secondMerged?.head_oid, "second merged head");
    expect(finalHead).not.toBe(second.headOid);
    const commits = await systemClient.json(
      `${project.path}/commits/${encodeURIComponent("feature/second")}?limit=1`,
    );
    expect(array(commits.body.items, "queued branch commits")).toEqual([
      expect.objectContaining({ oid: finalHead, parents: [second.headOid, first.headOid] }),
    ]);
    const repoRef = `myelin://${systemTestConfig.tenant}/git/repo/${project.slug}`;
    expect(await ciRunsMatching(systemClient, (run) =>
      run.repo_ref === repoRef && run.commit_oid === finalHead && run.state === "succeeded"))
      .toHaveLength(1);
    expect(await project.readFile("main", "second.txt")).toMatchObject({ contents: "second.txt\n" });
    expect(await project.readFile("main", "first.txt")).toMatchObject({ contents: "first.txt\n" });
  }, 180_000);

  test("takes a pull request out of the queue when its required check fails", async () => {
    const project = await queueWithRequiredCheck("merge-queue-red", `["false"]`);
    const change = await approvedChange(project, "feature/red", "red.txt");
    await systemClient.json(`${project.path}/prs/${change.number}/auto-merge`, {
      method: "POST",
      body: {},
    });

    const stopped = await eventually(async () => {
      const state = await pullRequest(project, change.number);
      return state.auto_merge_stopped ? state : undefined;
    }, { description: "the failed check to stop the automatic merge", timeoutMs: 120_000 });
    expect(stopped).toMatchObject({
      pr_state: "open",
      auto_merge: null,
      auto_merge_stopped: { reason: "a required check failed: ci/contract" },
    });
    const subject = pullRequestSubject(project.slug, change.number);
    const notice = await eventually(
      () => findInboxItemMatching(systemClient, (item) =>
        item.subject === subject && item.reason === "blocked"),
      { description: "the person who scheduled the merge to hear that it stopped" },
    );
    expect(notice).toMatchObject({ reason: "blocked", class: "watching" });

    const again = await systemClient.json(`${project.path}/prs/${change.number}/auto-merge`, {
      method: "POST",
      body: {},
    });
    expect(again.body).toMatchObject({ applied: { auto_merge: expect.any(Object) } });
    const cancelled = await systemClient.json(`${project.path}/prs/${change.number}/auto-merge`, {
      method: "DELETE",
    });
    expect(cancelled.body).toMatchObject({
      applied: { action: "git.pr.auto_merge.cancel", auto_merge: null },
    });
    expect(await pullRequest(project, change.number)).toMatchObject({
      auto_merge: null,
      auto_merge_stopped: null,
    });
    await reviewerClient.json(`${project.path}/prs/${change.number}/auto-merge`, {
      method: "POST",
      body: {},
      expectedStatus: [403, 404],
    });
  }, 150_000);

  test("squash merges leave one commit per pull request on main, queued or not", async () => {
    const project = new GitProject(uniqueName("squash-merge"), systemClient);
    await project.create();
    await project.writeFile("main", "README.md", "# squash\n");
    const startCommits = await systemClient.json(`${project.path}/commits/main?limit=1`);
    const start = string(
      record(array(startCommits.body.items, "start commits")[0], "start commit").oid,
      "start oid",
    );
    const first = await approvedChange(project, "feature/first", "first.txt");
    const second = await approvedChange(project, "feature/second", "second.txt");

    const merged = await systemClient.json(`${project.path}/prs/${first.number}/merge`, {
      method: "POST",
      body: { method: "squash" },
    });
    const squashed = string(record(merged.body.applied, "squash merge").new_oid, "squash oid");
    expect(squashed).not.toBe(first.headOid);
    expect(await pullRequest(project, first.number)).toMatchObject({
      pr_state: "merged",
      merged_oid: squashed,
    });

    await systemClient.json(`${project.path}/prs/${second.number}/auto-merge`, {
      method: "POST",
      body: { method: "squash" },
    });
    await eventually(async () =>
      (await pullRequest(project, second.number)).pr_state === "merged" ? true : undefined, {
      description: "the queued pull request to squash onto the first",
      timeoutMs: 60_000,
    });
    const history = array((await systemClient.json(
      `${project.path}/commits/main?limit=3`,
    )).body.items, "squashed history").map((item) => record(item, "commit"));
    expect(history).toEqual([
      expect.objectContaining({
        summary: `Add second.txt (#${second.number})`,
        parents: [squashed],
      }),
      expect.objectContaining({
        oid: squashed,
        summary: `Add first.txt (#${first.number})`,
        parents: [start],
      }),
      expect.objectContaining({ oid: start }),
    ]);
    expect(await project.readFile("main", "second.txt")).toMatchObject({ contents: "second.txt\n" });
  }, 120_000);
});
