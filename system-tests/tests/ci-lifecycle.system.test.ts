import { beforeAll, describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { Conversation } from "../src/journeys/chat.js";
import { array, record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";
import {
  awaitTheOnlyCiRun,
  ciRunsMatching,
} from "../src/journeys/ci-runs.js";

const runnerImage =
  "myelin.local/linux-small-v1-rootfs@sha256:65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";
const logMessage = "The sandbox ran this exact commit — café";

const pipeline = `schema_version = 2
on = "push"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "contract"
image = "${runnerImage}"
command = ["sh", "-c", "printf '${logMessage}\\n'"]
`;

const pullRequestPipeline = `schema_version = 2
on = "pull_request"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "contract"
image = "${runnerImage}"
command = ["true"]
`;

const failingPipeline = `schema_version = 2
on = "push"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "contract"
image = "${runnerImage}"
command = ["sh", "-c", "printf 'the test failed for a useful reason\\n'; exit 17"]
`;

describe.sequential("CI delivery lifecycle", () => {
  const slug = uniqueName("system-ci");
  const project = new GitProject(slug, systemClient);
  const repoRef = `myelin://${systemTestConfig.tenant}/git/repo/${slug}`;
  let pipelineCommitOid = "";
  let run: JsonRecord;

  beforeAll(async () => {
    await project.create();
    await project.writeFile("main", "README.md", `# ${slug}\n`);
  });

  test("dispatches exactly one run for a pushed pipeline", async () => {
    pipelineCommitOid = (await project.writeFile("main", ".myelin/ci.toml", pipeline)).commitOid;

    run = await awaitTheOnlyCiRun(
      systemClient,
      (candidate) => candidate.repo_ref === repoRef && candidate.commit_oid === pipelineCommitOid,
      `CI run for ${repoRef} at ${pipelineCommitOid}`,
    );

    expect(run).toMatchObject({
      ref: `myelin://${systemTestConfig.tenant}/ci/run/${run.run_id}`,
      repo_ref: repoRef,
      commit_oid: pipelineCommitOid,
      trigger_kind: "push",
    });
    expect(["queued", "running", "succeeded"]).toContain(run.state);
    expect(run.run_id).toMatch(/^[0-9a-f-]{36}$/);
  });

  test("keeps a pull request tied to the branch its automation protects", async () => {
    const feature = uniqueName("feature/branch-scoped-ci");
    const pullRequestCommit = (await project.updateFile(
      feature,
      ".myelin/ci.toml",
      pullRequestPipeline,
      { startRef: "main" },
    )).commitOid;
    await systemClient.json(`${project.path}/prs`, {
      method: "POST",
      body: {
        title: "Exercise branch-scoped pull request CI",
        base_ref: "refs/heads/main",
        head_ref: `refs/heads/${feature}`,
        head_oid: pullRequestCommit,
        reviewers: [],
      },
      expectedStatus: 201,
    });

    const pullRequestRun = await awaitTheOnlyCiRun(
      systemClient,
      (candidate) => candidate.repo_ref === repoRef && candidate.commit_oid === pullRequestCommit,
      "the pull request CI run to retain its target branch",
    );
    expect(pullRequestRun).toMatchObject({
      repo_ref: repoRef,
      source_ref: "refs/heads/main",
      commit_oid: pullRequestCommit,
      trigger_kind: "pull_request",
    });
    expect(["queued", "running", "succeeded"]).toContain(pullRequestRun.state);
  });

  test("executes the exact pushed commit and preserves its sandbox output", async () => {
    const runId = string(run.run_id, "CI run id");
    const detail = await eventually<JsonRecord>(
      async () => {
        const response = await systemClient.json(`/v1/ci/runs/${encodeURIComponent(runId)}`);
        const body = record(response.body, "CI run detail");
        const current = record(body.run, "CI run");
        if (current.state !== "succeeded") return undefined;
        return body;
      },
      { description: `CI run ${runId} to finish in the sandbox`, timeoutMs: 60_000 },
    );
    expect(detail).toMatchObject({
      run: {
        run_id: runId,
        repo_ref: repoRef,
        commit_oid: pipelineCommitOid,
        state: "succeeded",
        cost_settled: true,
      },
    });

    const jobs = array(detail.jobs, "CI jobs");
    expect(jobs).toHaveLength(1);
    const job = record(jobs[0], "CI job");
    expect(job).toMatchObject({ name: "contract", state: "succeeded", attempt: 1 });
    const jobId = string(job.job_id, "CI job id");

    const steps = array(detail.steps, "CI steps");
    expect(steps).toHaveLength(1);
    const step = record(steps[0], "CI step");
    expect(step).toMatchObject({ job_id: jobId, status: "passed", byte_start: 0 });
    expect(step.byte_end).toBeGreaterThan(0);

    const archived = await systemClient.json(
      `/v1/ci/runs/${encodeURIComponent(runId)}/jobs/${encodeURIComponent(jobId)}/log?start=0&limit=65536`,
    );
    const archive = record(archived.body, "CI log archive");
    expect(archive).toMatchObject({
      run_id: runId,
      job_id: jobId,
      byte_start: 0,
      byte_end: archive.total_end,
      next_offset: null,
      encoding: "base64",
    });
    expect(
      Buffer.from(string(archive.data, "CI log bytes"), "base64").toString("utf8"),
    ).toContain(`${logMessage}\n`);

    const missing = await systemClient.json(
      "/v1/ci/runs/00000000-0000-0000-0000-000000000000",
      { expectedStatus: 404 },
    );
    expect(missing.body).toHaveProperty("error");
  }, 65_000);

  test("turns a failing command into a settled, inspectable run", async () => {
    const failingProject = new GitProject(uniqueName("system-ci-failure"), systemClient);
    await failingProject.create();
    await failingProject.writeFile("main", "README.md", "# A deliberately failing build\n");
    const failedCommit = (await failingProject.writeFile(
      "main",
      ".myelin/ci.toml",
      failingPipeline,
    )).commitOid;

    const failedRun = await awaitTheOnlyCiRun(
      systemClient,
      (candidate) => candidate.commit_oid === failedCommit && candidate.state === "failed",
      "the failing command to become a terminal CI run",
      60_000,
    );

    const failedRunId = string(failedRun.run_id, "failed CI run id");
    const detail = (await systemClient.json(
      `/v1/ci/runs/${encodeURIComponent(failedRunId)}`,
    )).body;
    expect(detail).toMatchObject({
      run: {
        run_id: failedRunId,
        state: "failed",
        cost_settled: true,
      },
      jobs: [{ name: "contract", state: "failed", attempt: 1 }],
    });

    const failedJob = record(array(detail.jobs, "failed CI jobs")[0], "failed CI job");
    const log = (await systemClient.json(
      `/v1/ci/runs/${encodeURIComponent(failedRunId)}/jobs/${encodeURIComponent(string(failedJob.job_id, "failed CI job id"))}/log?start=0&limit=65536`,
    )).body;
    expect(Buffer.from(string(log.data, "failed CI log bytes"), "base64").toString("utf8"))
      .toContain("the test failed for a useful reason\n");
  }, 65_000);

  test("inherits repository visibility instead of exposing runs platform-wide", async () => {
    const runId = string(run.run_id, "CI run id");
    expect(await ciRunsMatching(reviewerClient, (candidate) => candidate.run_id === runId))
      .toEqual([]);

    const hiddenDetail = await reviewerClient.json(
      `/v1/ci/runs/${encodeURIComponent(runId)}`,
      { expectedStatus: 404 },
    );
    expect(hiddenDetail.body).toMatchObject({
      error: { code: "not_found" },
    });
  });

  test("keeps each engineer's CI result legible without exposing the other's repository", async () => {
    const reviewersProject = new GitProject(uniqueName("reviewer-private-ci"), reviewerClient);
    await reviewersProject.create();
    await reviewersProject.writeFile("main", "README.md", `# ${reviewersProject.slug}\n`);
    const reviewersCommit = (await reviewersProject.writeFile(
      "main",
      ".myelin/ci.toml",
      pipeline,
    )).commitOid;
    const reviewersRun = await awaitTheOnlyCiRun(
      reviewerClient,
      (candidate) => candidate.commit_oid === reviewersCommit && candidate.state === "succeeded",
      "the reviewer's private CI run to finish",
      60_000,
    );

    const foundersRunId = string(run.run_id, "founder's CI run id");
    const reviewersRunId = string(reviewersRun.run_id, "reviewer's CI run id");
    const foundersRef = `myelin://${systemTestConfig.tenant}/ci/run/${foundersRunId}`;
    const reviewersRef = `myelin://${systemTestConfig.tenant}/ci/run/${reviewersRunId}`;
    const room = await Conversation.open(systemClient, {
      projectId: systemTestConfig.issues.projectId,
      channel: uniqueName("private-ci-cards"),
      topic: "Compare delivery signals without broadening source access",
    });
    await room.post(systemClient, "My run \uFFFC is green; compare yours \uFFFC.", {
      references: [foundersRef, reviewersRef],
    });

    const foundersHistory = await room.messages(systemClient);
    expect(foundersHistory).toEqual([
      expect.objectContaining({
        nodes: [
          expect.objectContaining({
            ref: foundersRef,
            card: expect.objectContaining({
              kind: "projection",
              title: `${slug} CI`,
              state: "succeeded",
              icon: "ci",
              render_hint: "ci_run",
            }),
          }),
          expect.objectContaining({
            ref: reviewersRef,
            card: { kind: "tombstone" },
          }),
        ],
      }),
    ]);
    expect(JSON.stringify(foundersHistory)).not.toContain(reviewersProject.slug);

    const reviewersHistory = await room.messages(reviewerClient);
    expect(reviewersHistory).toEqual([
      expect.objectContaining({
        nodes: [
          expect.objectContaining({
            ref: foundersRef,
            card: { kind: "tombstone" },
          }),
          expect.objectContaining({
            ref: reviewersRef,
            card: expect.objectContaining({
              kind: "projection",
              title: `${reviewersProject.slug} CI`,
              state: "succeeded",
              icon: "ci",
              render_hint: "ci_run",
            }),
          }),
        ],
      }),
    ]);
    expect(JSON.stringify(reviewersHistory)).not.toContain(slug);
  }, 65_000);
});
