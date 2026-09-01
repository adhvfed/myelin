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
import { record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

const runnerImage =
  "myelin.local/linux-small-v1-rootfs@sha256:65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";

const failingPipeline = `schema_version = 2
on = "push"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "contract"
image = "${runnerImage}"
command = ["sh", "-c", "printf 'retry me once\\n'; exit 23"]
`;

describe("human-governed agent CI retries", () => {
  test("replays one immutable failed run only after its owner approves fresh compute", async () => {
    const founder = await browserApprovedCliClient();
    const project = new GitProject(uniqueName("agent-ci-retry"), founder);
    await project.create();
    await project.writeFile("main", "README.md", "# A deliberately failing pipeline\n");
    const failedCommit = (await project.writeFile(
      "main",
      ".myelin/ci.toml",
      failingPipeline,
    )).commitOid;
    const original = await eventually<JsonRecord>(async () => {
      const matching = await ciRunsMatching(
        founder,
        (candidate) => candidate.commit_oid === failedCommit && candidate.state === "failed",
      );
      return matching.length === 1 ? matching[0] : undefined;
    }, { description: "the source CI run to fail durably", timeoutMs: 60_000 });
    const originalRunId = string(original.run_id, "source failed CI run id");
    const originalRunRef = `myelin://${systemTestConfig.tenant}/ci/run/${originalRunId}`;

    const agent = await activateExternalAgent(
      founder,
      uniqueName("CI recovery companion"),
      ["ci.retry_run"],
    );
    const run = await beginAgentRun(founder, agent.agent.id);
    try {
      const retryArguments = { run_id: originalRunId };
      const retryKey = `ci-retry-${randomUUID()}`;
      const gateId = await askAgentToRequestApproval(
        run,
        1,
        "ci.retry_run",
        retryArguments,
        retryKey,
      );

      const approvalNotice = await eventually<JsonRecord>(async () => {
        return findInboxItemMatching(
          founder,
          (item) => item.subject === originalRunRef && item.state === "unread" &&
            item.action !== null && typeof item.action === "object" &&
            record(item.action, "CI retry approval action").gate_id === gateId,
        );
      }, { description: "the exact failed run to appear in the owner's approval inbox" });
      expect(approvalNotice).toMatchObject({
        subject: originalRunRef,
        reason: "approval_requested",
        action: {
          kind: "agent_effect_approval",
          gate_id: gateId,
          run_id: run.run.id,
        },
      });

      expect(await ciRunsMatching(
        founder,
        (candidate) => candidate.commit_oid === failedCommit,
      )).toHaveLength(1);
      await reviewerClient.json(
        `/v1/agent-approvals/${encodeURIComponent(gateId)}/decision`,
        {
          method: "POST",
          body: { decision: "approve" },
          idempotencyKey: `foreign-ci-retry-approval-${randomUUID()}`,
          expectedStatus: 403,
        },
      );
      const approved = await founder.json(
        `/v1/agent-approvals/${encodeURIComponent(gateId)}/decision`,
        {
          method: "POST",
          body: { decision: "approve" },
          idempotencyKey: `owner-ci-retry-approval-${randomUUID()}`,
          expectedStatus: 200,
        },
      );
      expect(approved.body).toMatchObject({
        gate_id: gateId,
        run_id: run.run.id,
        state: "approved",
        changed: true,
      });

      const retried = await askAgentToAct(
        run,
        2,
        "ci.retry_run",
        retryArguments,
        retryKey,
        gateId,
      );
      const replayed = await askAgentToAct(
        run,
        3,
        "ci.retry_run",
        retryArguments,
        retryKey,
        gateId,
      );
      expect(replayed).toEqual(retried);
      const retriedRunId = string(record(retried.data, "CI retry receipt").run_id, "retried run id");
      expect(retried).toMatchObject({
        event_id: expect.stringMatching(/^[0-9a-f-]{36}$/),
        ref: `myelin://${systemTestConfig.tenant}/ci/run/${retriedRunId}`,
        data: {
          run_id: retriedRunId,
          source_run_id: originalRunId,
          workflow_run_id: expect.stringMatching(/^[0-9a-f-]{36}$/),
        },
      });
      expect(retriedRunId).not.toBe(originalRunId);

      const retry = await eventually<JsonRecord>(async () => {
        const matching = await ciRunsMatching(
          founder,
          (candidate) => candidate.run_id === retriedRunId && candidate.state === "failed",
        );
        return matching.length === 1 ? matching[0] : undefined;
      }, { description: "the approved retry to execute and fail independently", timeoutMs: 60_000 });
      expect(retry).toMatchObject({
        run_id: retriedRunId,
        repo_ref: original.repo_ref,
        source_ref: original.source_ref,
        commit_oid: failedCommit,
        definition_snapshot: original.definition_snapshot,
        trigger_kind: "agent",
        trust_tier: original.trust_tier,
        state: "failed",
        cost_settled: true,
      });
      expect(await ciRunsMatching(
        founder,
        (candidate) => candidate.commit_oid === failedCommit,
      )).toHaveLength(2);
    } finally {
      await closeAgentRun(run);
    }
  }, 120_000);
});
