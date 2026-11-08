import { randomUUID } from "node:crypto";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, test } from "vitest";

import { systemClient, uniqueName } from "../src/context.js";
import { git } from "../src/git-cli.js";
import { GitProject } from "../src/git-project.js";
import { awaitTheOnlyCiRun } from "../src/journeys/ci-runs.js";
import { integer, record } from "../src/json.js";
import { gitRepositoryUrl, systemTestConfig } from "../src/config.js";

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

describe("pull requests follow their branch", () => {
  test("review fixes from the editor and from git push become the reviewed head", async () => {
    const slug = uniqueName("pr-follows-branch");
    const project = new GitProject(slug, systemClient);
    await project.create();
    await project.writeFile("main", "README.md", `# ${slug}\n`);
    await project.writeFile("main", ".myelin/ci.toml", pullRequestPipeline);
    const first = (await project.writeFile(
      "feature/follow",
      "first.txt",
      "first\n",
      { startRef: "main" },
    )).commitOid;
    const opened = await systemClient.json(`${project.path}/prs`, {
      method: "POST",
      body: {
        title: "Follow the branch",
        base_ref: "refs/heads/main",
        head_ref: "refs/heads/feature/follow",
        head_oid: first,
        reviewers: [],
      },
      idempotencyKey: `follow-branch-${randomUUID()}`,
      expectedStatus: 201,
    });
    const number = integer(
      record(record(opened.body.applied, "opened PR").pr, "pull request").number,
      "pull request number",
    );
    const pullRequest = async () =>
      record((await systemClient.json(`${project.path}/prs/${number}`)).body, "pull request");
    const repoRef = `myelin://${systemTestConfig.tenant}/git/repo/${slug}`;
    await awaitTheOnlyCiRun(
      systemClient,
      (run) => run.repo_ref === repoRef && run.commit_oid === first,
      "CI for the opened head",
    );

    const second = (await project.writeFile("feature/follow", "second.txt", "second\n"))
      .commitOid;
    expect(await pullRequest()).toMatchObject({ head_oid: second, commits_count: 2 });

    const root = await mkdtemp(join(tmpdir(), "myelin-pr-follow-"));
    try {
      const working = join(root, "work");
      await git(["clone", "--branch", "feature/follow", gitRepositoryUrl(slug), working], {
        token: systemTestConfig.token,
      });
      const pseudonym = `${systemTestConfig.principal}@${systemTestConfig.tenant}.noreply`;
      await git(["config", "user.name", pseudonym], { cwd: working });
      await git(["config", "user.email", pseudonym], { cwd: working });
      await writeFile(join(working, "third.txt"), "third\n", "utf8");
      await git(["add", "third.txt"], { cwd: working });
      await git(["commit", "-m", "fix: address review"], { cwd: working });
      const third = (await git(["rev-parse", "HEAD"], { cwd: working })).stdout.trim();
      await git(["push", "origin", "HEAD:refs/heads/feature/follow"], {
        cwd: working,
        token: systemTestConfig.token,
      });

      expect(await pullRequest()).toMatchObject({ head_oid: third, commits_count: 3 });
      const followed = await awaitTheOnlyCiRun(
        systemClient,
        (run) => run.repo_ref === repoRef && run.commit_oid === third,
        "CI to test the pushed review fix",
      );
      expect(followed).toMatchObject({ trigger_kind: "pull_request", commit_oid: third });

      await git(["commit", "--amend", "-m", "fix: address review, squashed"], { cwd: working });
      const rewritten = (await git(["rev-parse", "HEAD"], { cwd: working })).stdout.trim();
      await git(["push", "--force", "origin", "HEAD:refs/heads/feature/follow"], {
        cwd: working,
        token: systemTestConfig.token,
      });
      expect(await pullRequest()).toMatchObject({ head_oid: rewritten, commits_count: 3 });
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  }, 120_000);

  test("a pull request opened with a short branch name follows it too", async () => {
    const slug = uniqueName("pr-follows-short-ref");
    const project = new GitProject(slug, systemClient);
    await project.create();
    await project.writeFile("main", "README.md", `# ${slug}\n`);
    const first = (await project.writeFile("topic", "one.txt", "one\n", { startRef: "main" }))
      .commitOid;
    const opened = await systemClient.json(`${project.path}/prs`, {
      method: "POST",
      body: {
        title: "Short head",
        base_ref: "main",
        head_ref: "topic",
        head_oid: first,
        reviewers: [],
      },
      idempotencyKey: `follow-short-${randomUUID()}`,
      expectedStatus: 201,
    });
    const number = integer(
      record(record(opened.body.applied, "opened PR").pr, "pull request").number,
      "pull request number",
    );
    const second = (await project.writeFile("topic", "two.txt", "two\n")).commitOid;
    expect(
      record((await systemClient.json(`${project.path}/prs/${number}`)).body, "pull request"),
    ).toMatchObject({ head_oid: second, commits_count: 2 });
  }, 60_000);
});
