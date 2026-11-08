import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import {
  browserApprovedCliClient,
  reviewerClient,
  uniqueName,
} from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import {
  activateExternalAgent,
  askAgentToAct,
  askAgentToRequestApproval,
  beginAgentRun,
  closeAgentRun,
} from "../src/journeys/agents.js";
import { ciRunsMatching } from "../src/journeys/ci-runs.js";
import { findInboxItemMatching } from "../src/journeys/inbox.js";
import { array, record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

const runnerImage =
  "myelin.local/linux-small-v1-rootfs@sha256:65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";

const longRunningPipeline = `schema_version = 2
on = "push"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "contract"
image = "${runnerImage}"
command = ["sh", "-c", "printf 'waiting for an owner decision\\n'; sleep 90"]
`;

describe("human-governed agent CI cancellation", () => {
  test("stops one running sandbox only after its owner approves", async () => {
    const founder = await browserApprovedCliClient();
    const project = new GitProject(uniqueName("agent-ci-cancel"), founder);
    await project.create();
    await project.writeFile("main", "README.md", "# A deliberately long pipeline\n");
    const runningCommit = (await project.writeFile(
      "main",
      ".myelin/ci.toml",
      longRunningPipeline,
    )).commitOid;
    const running = await eventually<JsonRecord>(async () => {
      const matching = await ciRunsMatching(
        founder,
        (candidate) => candidate.commit_oid === runningCommit && candidate.state === "running",
      );
      return matching.length === 1 ? matching[0] : undefined;
    }, { description: "the target CI sandbox to be running", timeoutMs: 60_000 });
    const runId = string(running.run_id, "running CI run id");
    const runRef = `myelin://${systemTestConfig.tenant}/ci/run/${runId}`;
    const runningJob = await eventually<JsonRecord>(async () => {
      const response = await founder.json(`/v1/ci/runs/${encodeURIComponent(runId)}`);
      const jobs = array(response.body.jobs, "running CI jobs").map((job) =>
        record(job, "running CI job")
      );
      return jobs.find((job) => job.state === "running");
    }, { description: "the workload itself to start in gVisor", timeoutMs: 60_000 });
    const jobId = string(runningJob.job_id, "running CI job id");

    const agent = await activateExternalAgent(
      founder,
      uniqueName("CI interruption companion"),
      ["ci.cancel_run"],
    );
    const agentRun = await beginAgentRun(founder, agent.agent.id);
    try {
      const cancelArguments = { run_id: runId };
      const cancelKey = `ci-cancel-${randomUUID()}`;
      const gateId = await askAgentToRequestApproval(
        agentRun,
        1,
        "ci.cancel_run",
        cancelArguments,
        cancelKey,
      );

      const approvalNotice = await eventually<JsonRecord>(async () => {
        return findInboxItemMatching(
          founder,
          (item) => item.subject === runRef && item.state === "unread" &&
            item.action !== null && typeof item.action === "object" &&
            record(item.action, "CI cancellation approval action").gate_id === gateId,
        );
      }, { description: "the exact running CI run to appear in the owner's approval inbox" });
      expect(approvalNotice).toMatchObject({
        subject: runRef,
        reason: "approval_requested",
        action: {
          kind: "agent_effect_approval",
          gate_id: gateId,
          run_id: agentRun.run.id,
        },
      });

      await reviewerClient.json(
        `/v1/agent-approvals/${encodeURIComponent(gateId)}/decision`,
        {
          method: "POST",
          body: { decision: "approve" },
          idempotencyKey: `foreign-ci-cancel-approval-${randomUUID()}`,
          expectedStatus: 403,
        },
      );
      expect(await ciRunsMatching(
        founder,
        (candidate) => candidate.run_id === runId && candidate.state === "running",
      )).toHaveLength(1);

      const approved = await founder.json(
        `/v1/agent-approvals/${encodeURIComponent(gateId)}/decision`,
        {
          method: "POST",
          body: { decision: "approve" },
          idempotencyKey: `owner-ci-cancel-approval-${randomUUID()}`,
          expectedStatus: 200,
        },
      );
      expect(approved.body).toMatchObject({
        gate_id: gateId,
        run_id: agentRun.run.id,
        state: "approved",
        changed: true,
      });

      const cancelled = await askAgentToAct(
        agentRun,
        2,
        "ci.cancel_run",
        cancelArguments,
        cancelKey,
        gateId,
      );
      const replayed = await askAgentToAct(
        agentRun,
        3,
        "ci.cancel_run",
        cancelArguments,
        cancelKey,
        gateId,
      );
      expect(replayed).toEqual(cancelled);
      expect(cancelled).toMatchObject({
        event_id: expect.stringMatching(/^ci-agent-cancel-[0-9a-f]{64}$/),
        ref: runRef,
        data: {
          run_id: runId,
          workflow_run_id: expect.stringMatching(/^[0-9a-f-]{36}$/),
          state: "cancelled",
        },
      });

      const terminal = await eventually<JsonRecord>(async () => {
        const matching = await ciRunsMatching(
          founder,
          (candidate) => candidate.run_id === runId &&
            candidate.state === "cancelled" && candidate.cost_settled === true,
        );
        return matching.length === 1 ? matching[0] : undefined;
      }, { description: "the cancelled sandbox to settle durably", timeoutMs: 60_000 });
      expect(terminal).toMatchObject({
        run_id: runId,
        repo_ref: running.repo_ref,
        source_ref: running.source_ref,
        commit_oid: runningCommit,
        definition_snapshot: running.definition_snapshot,
        trigger_kind: "push",
        trust_tier: running.trust_tier,
        state: "cancelled",
        cost_settled: true,
      });
      const terminalDetail = record(
        (await founder.json(`/v1/ci/runs/${encodeURIComponent(runId)}`)).body,
        "cancelled CI detail",
      );
      expect(array(terminalDetail.jobs, "cancelled CI jobs")).toContainEqual(
        expect.objectContaining({ job_id: jobId, state: "cancelled" }),
      );
      expect(await ciRunsMatching(
        founder,
        (candidate) => candidate.commit_oid === runningCommit,
      )).toHaveLength(1);
    } finally {
      await closeAgentRun(agentRun);
    }
  }, 120_000);
});
