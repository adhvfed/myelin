import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import {
  browserApprovedCliClient,
  reviewerClient,
  systemClient,
  uniqueName,
} from "../src/context.js";
import { ExternalEventBus } from "../src/event-bus.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { findInboxItemMatching } from "../src/journeys/inbox.js";
import { awaitActiveIssue } from "../src/journeys/issues.js";
import { array, integer, record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

describe("human-governed agent merges", () => {
  test("keeps every proposed merge asleep until its founder explicitly decides its fate", async () => {
    const founder = await browserApprovedCliClient();
    const issue = await awaitActiveIssue(systemClient, uniqueName("Merge handoff"));
    const issueRef = string(issue.ref, "merge handoff issue ref");
    const issueKey = string(issue.key, "merge handoff issue key");

    const slug = uniqueName("agent-approved-merge");
    const project = new GitProject(slug, founder);
    await project.create();
    await founder.json(`${project.path}/branch-protection`, {
      method: "POST",
      body: {
        rulesets: [{
          ref_pattern: "refs/heads/main",
          required_contexts: [],
          required_approvals: 0,
          require_codeowner_review: false,
          require_conversation_resolution: false,
          allow_force_push: false,
        }],
      },
      expectedStatus: 200,
    });
    await project.writeFile("main", "README.md", `# ${slug}\n`);
    const featureCommit = (await project.writeFile(
      "feature/human-gated",
      "approved.txt",
      "A human approved this exact agent effect.\n",
      { startRef: "main" },
    )).commitOid;
    const opened = await founder.json(`${project.path}/prs`, {
      method: "POST",
      body: {
        title: "Human-gated agent merge",
        base_ref: "refs/heads/main",
        head_ref: "refs/heads/feature/human-gated",
        head_oid: featureCommit,
        reviewers: [],
      },
      idempotencyKey: `agent-gated-pr-${randomUUID()}`,
      expectedStatus: 201,
    });
    const pullRequest = record(
      record(opened.body.applied, "agent-gated PR receipt").pr,
      "agent-gated pull request",
    );
    const pullRequestNumber = integer(pullRequest.number, "agent-gated PR number");
    const pullRequestRef =
      `myelin://${systemTestConfig.tenant}/git/pr/${slug}:${pullRequestNumber}`;

    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: {
        name: uniqueName("Merge companion"),
        runtime: "hosted",
        tools: ["git.merge"],
      },
      idempotencyKey: `merge-agent-${randomUUID()}`,
      expectedStatus: 201,
    });
    const agentId = string(record(activated.body.agent, "merge agent").id, "merge agent id");
    const mergeIntent = {
      event_type: "issue.issue.updated",
      filter: "payload.change_kind == 'merge_request'",
      run_as_agent_id: agentId,
      task: `Merge pull request ${slug}#${pullRequestNumber}.`,
      budget_minor_units: 100_000,
      max_firings: 2,
      delegation_caveats: [`repo:${slug}`, "pull_request.merge"],
      require_human_approval: false,
    };
    const created = await founder.json("/v1/triggers", {
      method: "POST",
      body: mergeIntent,
      idempotencyKey: `merge-trigger-${randomUUID()}`,
      expectedStatus: 201,
    });
    const triggerId = string(
      record(created.body.trigger, "merge trigger").id,
      "merge trigger id",
    );

    const publishMergeRequest = async (eventId: string): Promise<void> => {
      const now = new Date().toISOString();
      const bus = await ExternalEventBus.connect(systemTestConfig.natsUrl);
      try {
        expect((await bus.publish({
          event_id: eventId,
          type_: "issue.issue.updated",
          schema_ver: 1,
          tenant: systemTestConfig.tenant,
          region: systemTestConfig.region,
          actor: {
            tenant: systemTestConfig.tenant,
            region: systemTestConfig.region,
            principal_id: "issues-service",
            kind: "Service",
            data_role: "Controller",
            status: "Active",
          },
          subject: issueRef,
          aggregate: `issue:${issueKey}`,
          causation_id: null,
          correlation_id: eventId,
          caused_by: null,
          depth: 1,
          contains_personal_data: false,
          data_role: "Controller",
          visibility: "Internal",
          pii_key_ref: null,
          occurred_at: now,
          recorded_at: now,
          payload: { issue: issueRef, change_kind: "merge_request" },
        })).duplicate).toBe(false);
      } finally {
        await bus.close();
      }
    };
    const awaitUnreadApproval = (description: string) => eventually<JsonRecord>(async () => {
      return findInboxItemMatching(
        founder,
        (item) => item.subject === pullRequestRef && item.state === "unread" &&
          item.action !== null && typeof item.action === "object" &&
          record(item.action, "agent approval action").kind === "agent_effect_approval",
      );
    }, { description });
    const awaitTerminalFiring = (
      automationId: string,
      eventId: string,
      description: string,
    ) =>
      eventually<JsonRecord>(async () => {
        const response = await founder.json(
          `/v1/triggers/${encodeURIComponent(automationId)}/firings?limit=100`,
        );
        return array(response.body.items, "merge trigger history")
          .map((item) => record(item, "merge trigger firing"))
          .find((item) => item.event_id === eventId && item.state === "terminal");
      }, { description });

    const rejectedEventId = `merge-request-rejected-${randomUUID()}`;
    await publishMergeRequest(rejectedEventId);

    const rejectedNotice = await awaitUnreadApproval(
      "the first agent to park on its exact merge and release its runtime",
    );
    const rejectedAction = record(rejectedNotice.action, "rejected agent merge action");
    const rejectedGateId = string(rejectedAction.gate_id, "rejected agent merge gate id");
    expect(rejectedGateId).toMatch(/^gate:[0-9a-f]{32}$/);
    expect(rejectedNotice).toMatchObject({
      reason: "approval_requested",
      state: "unread",
      coalesce_count: 1,
      subject: pullRequestRef,
      action: {
        kind: "agent_effect_approval",
        gate_id: rejectedGateId,
        run_id: expect.any(String),
      },
    });

    expect((await founder.json(`${project.path}/prs/${pullRequestNumber}`)).body)
      .toMatchObject({ pr_state: "open" });
    await reviewerClient.json(
      `/v1/agent-approvals/${encodeURIComponent(rejectedGateId)}/decision`,
      {
        method: "POST",
        body: { decision: "approve" },
        idempotencyKey: `peer-agent-approval-${randomUUID()}`,
        expectedStatus: 403,
      },
    );

    const rejected = await founder.json(
      `/v1/agent-approvals/${encodeURIComponent(rejectedGateId)}/decision`,
      {
        method: "POST",
        body: { decision: "reject" },
        idempotencyKey: `agent-rejection-${randomUUID()}`,
        expectedStatus: 200,
      },
    );
    expect(rejected.body).toMatchObject({
      gate_id: rejectedGateId,
      state: "rejected",
      changed: true,
    });
    await eventually(async () => {
      const response = await founder.json(`${project.path}/prs/${pullRequestNumber}`);
      const done = await findInboxItemMatching(
        founder,
        (item) => item.id === rejectedNotice.id && item.state === "done",
      );
      return response.body.pr_state === "open" && done ? true : undefined;
    }, { description: "the rejected effect to finish without touching the pull request" });
    expect(await awaitTerminalFiring(
      triggerId,
      rejectedEventId,
      "the rejected hosted workflow to settle and free its firing slot",
    )).toMatchObject({ outcome: "succeeded" });

    const stoppedEventId = `merge-request-stopped-${randomUUID()}`;
    await publishMergeRequest(stoppedEventId);
    const stoppedNotice = await awaitUnreadApproval(
      "the next agent to park before its automation is switched off",
    );
    const stoppedAction = record(stoppedNotice.action, "stopped agent merge action");
    const stoppedGateId = string(stoppedAction.gate_id, "stopped agent merge gate id");
    const disabled = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/disable`,
      { method: "POST", body: {}, expectedStatus: 200 },
    );
    expect(disabled.body).toMatchObject({
      action: "disable",
      changed: true,
      canceled_firings: 1,
      trigger: { id: triggerId, state: "disabled", firings_used: 2 },
    });
    expect(await awaitTerminalFiring(
      triggerId,
      stoppedEventId,
      "the disabled automation to close its parked workflow",
    )).toMatchObject({
      outcome: "terminated",
      terminal_reason: "automation disabled by owner",
    });
    const lateApproval = await founder.json(
      `/v1/agent-approvals/${encodeURIComponent(stoppedGateId)}/decision`,
      {
        method: "POST",
        body: { decision: "approve" },
        idempotencyKey: `late-agent-approval-${randomUUID()}`,
        expectedStatus: 409,
      },
    );
    expect(lateApproval.body).toMatchObject({ error: { code: "conflict" } });
    await eventually(async () => {
      const done = await findInboxItemMatching(
        founder,
        (item) => item.id === stoppedNotice.id && item.state === "done",
      );
      return done === undefined ? undefined : true;
    }, { description: "disable to remove its stale approval card" });
    expect((await founder.json(`${project.path}/prs/${pullRequestNumber}`)).body)
      .toMatchObject({ pr_state: "open" });

    const replacement = await founder.json("/v1/triggers", {
      method: "POST",
      body: { ...mergeIntent, max_firings: 1 },
      idempotencyKey: `replacement-merge-trigger-${randomUUID()}`,
      expectedStatus: 201,
    });
    const replacementTriggerId = string(
      record(replacement.body.trigger, "replacement merge trigger").id,
      "replacement merge trigger id",
    );
    const eventId = `merge-request-approved-${randomUUID()}`;
    await publishMergeRequest(eventId);
    const approvalNotice = await awaitUnreadApproval(
      "the second agent to offer the same merge for an independent human decision",
    );
    const action = record(approvalNotice.action, "approved agent merge action");
    const gateId = string(action.gate_id, "approved agent merge gate id");
    expect(gateId).not.toBe(rejectedGateId);

    const approved = await founder.json(
      `/v1/agent-approvals/${encodeURIComponent(gateId)}/decision`,
      {
        method: "POST",
        body: { decision: "approve" },
        idempotencyKey: `agent-approval-${randomUUID()}`,
        expectedStatus: 200,
      },
    );
    expect(approved.body).toMatchObject({
      gate_id: gateId,
      state: "approved",
      changed: true,
    });
    const replayed = await founder.json(
      `/v1/agent-approvals/${encodeURIComponent(gateId)}/decision`,
      {
        method: "POST",
        body: { decision: "approve" },
        idempotencyKey: `agent-approval-replay-${randomUUID()}`,
        expectedStatus: 200,
      },
    );
    expect(replayed.body).toMatchObject({ gate_id: gateId, changed: false });

    await eventually(async () => {
      const response = await founder.json(`${project.path}/prs/${pullRequestNumber}`);
      return response.body.pr_state === "merged" ? response.body : undefined;
    }, { description: "the approved agent to resume and merge exactly once" });
    expect((await project.readFile("main", "approved.txt")).contents).toBe(
      "A human approved this exact agent effect.\n",
    );
    const completedNotice = await eventually<JsonRecord>(async () => {
      return findInboxItemMatching(
        founder,
        (item) => item.id === approvalNotice.id && item.state === "done",
      );
    }, { description: "the decided agent effect to leave the active inbox" });
    expect(completedNotice.action).toEqual(action);

    const completedFiring = await awaitTerminalFiring(
      replacementTriggerId,
      eventId,
      "the resumed hosted workflow to finish after the approved effect",
    );
    expect(completedFiring).toMatchObject({ outcome: "succeeded" });
  }, 90_000);
});
