import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import {
  browserApprovedCliClient,
  reviewerClient,
  systemClient,
  uniqueName,
} from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import {
  activateExternalAgent,
  beginAgentRun,
  closeAgentRun,
} from "../src/journeys/agents.js";
import { ciRunsMatching } from "../src/journeys/ci-runs.js";
import { array, record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

const runnerImage =
  "myelin.local/linux-small-v1-rootfs@sha256:65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";

function pipeline(command: string): string {
  return `schema_version = 2
on = "push"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "contract"
image = "${runnerImage}"
command = ["sh", "-c", "${command}"]
`;
}

async function theOnlyRun(
  client: Awaited<ReturnType<typeof browserApprovedCliClient>>,
  predicate: (run: JsonRecord) => boolean,
  description: string,
): Promise<JsonRecord> {
  return eventually<JsonRecord>(async () => {
    const matching = await ciRunsMatching(client, predicate);
    return matching.length === 1 ? matching[0] : undefined;
  }, { description, timeoutMs: 60_000 });
}

describe("people control their own CI", () => {
  test("re-runs one failed run on its exact provenance, once per intent", async () => {
    const founder = await browserApprovedCliClient();
    const project = new GitProject(uniqueName("human-ci-retry"), founder);
    await project.create();
    await project.writeFile("main", "README.md", "# A deliberately failing pipeline\n");
    const commit = (await project.writeFile(
      "main",
      ".myelin/ci.toml",
      pipeline("printf 'flaky\\\\n'; exit 23"),
    )).commitOid;
    const failed = await theOnlyRun(
      founder,
      (run) => run.commit_oid === commit && run.state === "failed",
      "the source run to fail durably",
    );
    const runId = string(failed.run_id, "failed run id");

    const detail = await founder.json(`/v1/ci/runs/${runId}`);
    expect(detail.body.controls).toEqual({ retry: true, cancel: false });

    await reviewerClient.json(`/v1/ci/runs/${runId}/retry`, {
      method: "POST",
      body: {},
      idempotencyKey: `outsider-retry-${randomUUID()}`,
      expectedStatus: 404,
    });
    await founder.json(`/v1/ci/runs/${runId}/cancel`, {
      method: "POST",
      body: {},
      idempotencyKey: `finished-cancel-${randomUUID()}`,
      expectedStatus: 409,
    });

    const key = `human-retry-${randomUUID()}`;
    const retried = await founder.json(`/v1/ci/runs/${runId}/retry`, {
      method: "POST",
      body: {},
      idempotencyKey: key,
      expectedStatus: 202,
    });
    const newRunId = string(retried.body.run_id, "retried run id");
    expect(retried.body).toMatchObject({
      ref: `myelin://${systemTestConfig.tenant}/ci/run/${newRunId}`,
      source_run_id: runId,
      replayed: false,
    });
    expect(newRunId).not.toBe(runId);
    const replayed = await founder.json(`/v1/ci/runs/${runId}/retry`, {
      method: "POST",
      body: {},
      idempotencyKey: key,
      expectedStatus: 200,
    });
    expect(replayed.body).toEqual({ ...retried.body, replayed: true });

    const fresh = await theOnlyRun(
      founder,
      (run) => run.run_id === newRunId && run.state === "failed" && run.cost_settled === true,
      "the human retry to execute and fail again",
    );
    expect(fresh).toMatchObject({
      repo_ref: failed.repo_ref,
      source_ref: failed.source_ref,
      commit_oid: commit,
      definition_snapshot: failed.definition_snapshot,
      trust_tier: failed.trust_tier,
      trigger_kind: "manual",
    });
    expect(await ciRunsMatching(founder, (run) => run.commit_oid === commit)).toHaveLength(2);
  }, 120_000);

  test("stops a running sandbox, settles it once, and keeps agents on governed tools", async () => {
    const founder = await browserApprovedCliClient();
    const project = new GitProject(uniqueName("human-ci-cancel"), founder);
    await project.create();
    await project.writeFile("main", "README.md", "# A deliberately long pipeline\n");
    const commit = (await project.writeFile(
      "main",
      ".myelin/ci.toml",
      pipeline("printf 'long work\\\\n'; sleep 90"),
    )).commitOid;
    const running = await theOnlyRun(
      founder,
      (run) => run.commit_oid === commit && run.state === "running",
      "the target sandbox to be running",
    );
    const runId = string(running.run_id, "running run id");
    const job = await eventually<JsonRecord>(async () => {
      const response = await founder.json(`/v1/ci/runs/${runId}`);
      return array(response.body.jobs, "jobs")
        .map((value) => record(value, "job"))
        .find((candidate) => candidate.state === "running");
    }, { description: "the workload to start in gVisor", timeoutMs: 60_000 });
    expect((await founder.json(`/v1/ci/runs/${runId}`)).body.controls).toEqual({
      retry: false,
      cancel: true,
    });

    const agent = await activateExternalAgent(
      founder,
      uniqueName("Ungoverned CI attempt"),
      ["ci.cancel_run"],
    );
    const agentRun = await beginAgentRun(founder, agent.agent.id);
    try {
      await systemClient.json(`/v1/ci/runs/${runId}/cancel`, {
        method: "POST",
        body: {},
        token: agentRun.credential.token,
        tokenScheme: "agent",
        idempotencyKey: `agent-bypass-${randomUUID()}`,
        expectedStatus: 403,
      });
    } finally {
      await closeAgentRun(agentRun);
    }
    await reviewerClient.json(`/v1/ci/runs/${runId}/cancel`, {
      method: "POST",
      body: {},
      idempotencyKey: `outsider-cancel-${randomUUID()}`,
      expectedStatus: 404,
    });

    const key = `human-cancel-${randomUUID()}`;
    const started = Date.now();
    const cancelled = await founder.json(`/v1/ci/runs/${runId}/cancel`, {
      method: "POST",
      body: {},
      idempotencyKey: key,
      expectedStatus: 200,
    });
    expect(cancelled.body).toMatchObject({
      run_id: runId,
      ref: `myelin://${systemTestConfig.tenant}/ci/run/${runId}`,
      state: "cancelled",
      event_id: expect.stringMatching(/^ci-cancel-[0-9a-f]{64}$/),
      replayed: false,
    });
    const replayed = await founder.json(`/v1/ci/runs/${runId}/cancel`, {
      method: "POST",
      body: {},
      idempotencyKey: key,
      expectedStatus: 200,
    });
    expect(replayed.body).toEqual({ ...cancelled.body, replayed: true });

    await theOnlyRun(
      founder,
      (run) => run.run_id === runId && run.state === "cancelled" && run.cost_settled === true,
      "the cancelled sandbox to settle durably",
    );
    expect(Date.now() - started).toBeLessThan(60_000);
    const settled = await founder.json(`/v1/ci/runs/${runId}`);
    expect(array(settled.body.jobs, "jobs")).toContainEqual(
      expect.objectContaining({ job_id: job.job_id, state: "cancelled" }),
    );
    expect(settled.body.controls).toEqual({ retry: true, cancel: false });
    await founder.json(`/v1/ci/runs/${runId}/cancel`, {
      method: "POST",
      body: {},
      idempotencyKey: `cancel-again-${randomUUID()}`,
      expectedStatus: 409,
    });
  }, 150_000);
});
