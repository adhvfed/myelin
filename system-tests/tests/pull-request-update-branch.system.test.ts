import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import {
  browserApprovedCliClient,
  reviewerClient,
  systemClient,
  uniqueName,
} from "../src/context.js";
import { systemTestConfig } from "../src/config.js";
import { GitProject } from "../src/git-project.js";
import {
  activateExternalAgent,
  askAgentToAct,
  askAgentToBeDenied,
  beginAgentRun,
  closeAgentRun,
} from "../src/journeys/agents.js";
import { awaitTheOnlyCiRun } from "../src/journeys/ci-runs.js";
import { proposeChange } from "../src/journeys/pull-requests.js";
import { array, record, string } from "../src/json.js";

const runnerImage =
  "myelin.local/linux-small-v1-rootfs@sha256:65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";

const pullRequestPipeline = `schema_version = 2
on = "pull_request"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "contract"
image = "${runnerImage}"
command = ["true"]
`;

describe("updating a pull request's branch", () => {
  test("brings a branch that another merge left behind up to date and lets it merge", async () => {
    const slug = uniqueName("pr-update-branch");
    const project = new GitProject(slug, systemClient);
    await project.create();
    await project.writeFile("main", "README.md", `# ${slug}\n`);
    await project.writeFile("main", ".myelin/ci.toml", pullRequestPipeline);
    const reviewers = [systemTestConfig.reviewerPrincipal];
    const first = await proposeChange(systemClient, project, {
      branch: "feature/first",
      path: "first.txt",
      contents: "first\n",
      title: "First change",
      reviewers,
    });
    const second = await proposeChange(systemClient, project, {
      branch: "feature/second",
      path: "second.txt",
      contents: "second\n",
      title: "Second change",
      reviewers,
    });
    for (const change of [first, second]) {
      await reviewerClient.json(`${project.path}/prs/${change.number}/reviews`, {
        method: "POST",
        body: { verdict: "approve" },
      });
    }
    const secondPath = `${project.path}/prs/${second.number}`;

    await systemClient.json(`${project.path}/prs/${first.number}/merge`, {
      method: "POST",
      body: {},
    });
    const stranded = await systemClient.json(`${secondPath}/merge`, {
      method: "POST",
      body: {},
      expectedStatus: 409,
    });
    expect(stranded.body).toMatchObject({ error: { code: "branch_behind_base" } });
    expect((await systemClient.json(secondPath)).body).toMatchObject({ behind_base: true });

    const retryKey = `update-branch-${randomUUID()}`;
    const updated = await systemClient.json(`${secondPath}/update-branch`, {
      method: "POST",
      body: { expected_head_oid: second.headOid },
      idempotencyKey: retryKey,
    });
    const applied = record(updated.body.applied, "branch update receipt.applied");
    expect(applied).toMatchObject({
      action: "git.pr.update_branch",
      updated: true,
      replayed: false,
    });
    const merged = string(applied.head_oid, "updated head");
    expect(merged).not.toBe(second.headOid);
    const replay = await systemClient.json(`${secondPath}/update-branch`, {
      method: "POST",
      body: { expected_head_oid: second.headOid },
      idempotencyKey: retryKey,
    });
    expect(replay.body).toMatchObject({
      applied: { updated: true, head_oid: merged, replayed: true },
    });

    expect((await systemClient.json(secondPath)).body).toMatchObject({
      head_oid: merged,
      commits_count: 2,
      behind_base: false,
    });
    expect((await systemClient.json(`${secondPath}/checks`)).body).toMatchObject({
      current_approvals: 1,
      gate_admitted: true,
    });
    expect(await project.readFile("feature/second", "first.txt")).toMatchObject({
      contents: "first\n",
    });
    const commits = await systemClient.json(
      `${project.path}/commits/${encodeURIComponent("feature/second")}?limit=1`,
    );
    expect(array(commits.body.items, "updated branch commits")).toEqual([
      expect.objectContaining({
        oid: merged,
        summary: "Merge branch 'main' into feature/second",
        parents: [second.headOid, first.headOid],
      }),
    ]);
    const repoRef = `myelin://${systemTestConfig.tenant}/git/repo/${slug}`;
    expect(await awaitTheOnlyCiRun(
      systemClient,
      (run) => run.repo_ref === repoRef && run.commit_oid === merged,
      "CI to test the updated branch",
    )).toMatchObject({ trigger_kind: "pull_request" });

    expect((await systemClient.json(`${secondPath}/update-branch`, {
      method: "POST",
      body: {},
    })).body).toMatchObject({
      applied: { action: "git.pr.update_branch", updated: false, head_oid: merged },
    });
    expect((await systemClient.json(`${secondPath}/merge`, {
      method: "POST",
      body: {},
    })).body).toMatchObject({ applied: { merged: true, new_oid: merged } });
    expect(await project.readFile("main", "second.txt")).toMatchObject({
      contents: "second\n",
    });
  }, 120_000);

  test("reports conflicting paths without touching the branch", async () => {
    const slug = uniqueName("pr-update-conflict");
    const project = new GitProject(slug, systemClient);
    await project.create();
    await project.writeFile("main", "README.md", "# start\n");
    const change = await proposeChange(systemClient, project, {
      branch: "feature/clash",
      path: "notes.txt",
      contents: "notes\n",
      title: "Clashing change",
    });
    const head = (await project.updateFile("feature/clash", "README.md", "# feature\n"))
      .commitOid;
    await project.updateFile("main", "README.md", "# main moved\n");
    const path = `${project.path}/prs/${change.number}`;

    const refused = await systemClient.json(`${path}/update-branch`, {
      method: "POST",
      body: {},
      expectedStatus: 409,
    });
    expect(refused.body).toMatchObject({
      error: { code: "merge_conflict" },
      conflicts: ["README.md"],
    });
    expect((await systemClient.json(path)).body).toMatchObject({ head_oid: head });
    expect(await project.readFile("feature/clash", "README.md")).toMatchObject({
      contents: "# feature\n",
    });

    await systemClient.json(`${path}/update-branch`, {
      method: "POST",
      body: { expected_head_oid: change.headOid },
      expectedStatus: 409,
    });
    await reviewerClient.json(`${path}/update-branch`, {
      method: "POST",
      body: {},
      expectedStatus: [403, 404],
    });
  }, 60_000);

  test("an agent brings its pull request up to date through the governed tool", async () => {
    const founder = await browserApprovedCliClient();
    const slug = uniqueName("agent-update-branch");
    const project = new GitProject(slug, founder);
    await project.create();
    await project.writeFile("main", "README.md", `# ${slug}\n`);
    const change = await proposeChange(founder, project, {
      branch: "feature/agent",
      path: "agent.txt",
      contents: "agent\n",
      title: "Agent change",
    });
    const mainTip = (await project.writeFile("main", "moved.txt", "moved\n")).commitOid;
    const path = `${project.path}/prs/${change.number}`;
    expect((await founder.json(path)).body).toMatchObject({ behind_base: true });

    const agent = await activateExternalAgent(founder, uniqueName("Branch keeper"), [
      "git.update_branch",
    ]);
    const run = await beginAgentRun(founder, agent.agent.id);
    try {
      const request = { repo: slug, number: change.number, expected_head_oid: change.headOid };
      const key = `agent-update-branch-${randomUUID()}`;
      const updated = await askAgentToAct(run, 1, "git.update_branch", request, key);
      const head = string(record(updated.data, "agent branch update").head_oid, "updated head");
      expect(updated).toMatchObject({
        ref: `myelin://${systemTestConfig.tenant}/git/commit/${slug}:${head}`,
        data: { repo: slug, number: change.number, updated: true },
      });
      expect(await askAgentToAct(run, 2, "git.update_branch", request, key)).toEqual(updated);
      expect((await founder.json(path)).body).toMatchObject({
        head_oid: head,
        behind_base: false,
      });
      const commits = await founder.json(
        `${project.path}/commits/${encodeURIComponent("feature/agent")}?limit=1`,
      );
      expect(array(commits.body.items, "agent-updated commits")).toEqual([
        expect.objectContaining({ oid: head, parents: [change.headOid, mainTip] }),
      ]);
      expect(await askAgentToBeDenied(run, 3, "git.update_branch", request)).toContain(
        "branch moved",
      );
    } finally {
      await closeAgentRun(run);
    }
  }, 120_000);
});
