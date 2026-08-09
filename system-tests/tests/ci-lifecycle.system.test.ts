import { beforeAll, describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { array, record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

const pipeline = `on = "push"

[[jobs]]
name = "contract"
image = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000"
command = ["true"]
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

  test("turns a pushed pipeline into exactly one queued run", async () => {
    pipelineCommitOid = (await project.writeFile("main", ".myelin/ci.toml", pipeline)).commitOid;

    run = await eventually<JsonRecord>(
      async () => {
        const response = await systemClient.json("/v1/ci/runs?state=all&limit=100");
        const matches = array(response.body.items, "CI run list items")
          .map((item) => record(item, "CI run list item"))
          .filter((item) => item.repo_ref === repoRef && item.commit_oid === pipelineCommitOid);
        if (matches.length === 0) return undefined;
        expect(matches).toHaveLength(1);
        return matches[0];
      },
      { description: `CI run for ${repoRef} at ${pipelineCommitOid}` },
    );

    expect(run).toMatchObject({
      repo_ref: repoRef,
      commit_oid: pipelineCommitOid,
      trigger_kind: "push",
      state: "queued",
      cost_settled: false,
    });
    expect(run.run_id).toMatch(/^[0-9a-f-]{36}$/);
  });

  test("surfaces the queued run while local execution is intentionally disabled", async () => {
    const runId = string(run.run_id, "CI run id");
    const detail = await systemClient.json(`/v1/ci/runs/${encodeURIComponent(runId)}`);
    expect(detail.body).toMatchObject({
      run: {
        run_id: runId,
        repo_ref: repoRef,
        commit_oid: pipelineCommitOid,
        state: "queued",
      },
    });
    expect(array(detail.body.jobs, "CI jobs")).toEqual([]);
    expect(array(detail.body.steps, "CI steps")).toEqual([]);

    const missing = await systemClient.json(
      "/v1/ci/runs/00000000-0000-0000-0000-000000000000",
      { expectedStatus: 404 },
    );
    expect(missing.body).toHaveProperty("error");
  });

  test("inherits repository visibility instead of exposing runs platform-wide", async () => {
    const runId = string(run.run_id, "CI run id");
    const reviewerRuns = await reviewerClient.json("/v1/ci/runs?state=all&limit=100");
    expect(
      array(reviewerRuns.body.items, "reviewer-visible CI runs")
        .map((item) => record(item, "reviewer-visible CI run"))
        .some((item) => item.run_id === runId),
    ).toBe(false);

    const hiddenDetail = await reviewerClient.json(
      `/v1/ci/runs/${encodeURIComponent(runId)}`,
      { expectedStatus: 404 },
    );
    expect(hiddenDetail.body).toMatchObject({
      error: { code: "not_found" },
    });
  });
});
