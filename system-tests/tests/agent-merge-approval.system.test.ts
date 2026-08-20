import { randomUUID } from "node:crypto";

import { describe, expect, onTestFinished, test } from "vitest";

import {
  browserApprovedCliClient,
  reviewerClient,
  systemClient,
  uniqueName,
} from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { awaitAutomationFiring } from "../src/journeys/automations.js";
import { findInboxItemMatching } from "../src/journeys/inbox.js";
import { announceIssueChange, awaitActiveIssue } from "../src/journeys/issues.js";
import { integer, record, string, type JsonRecord } from "../src/json.js";
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

    const publishMergeRequest = (eventId: string) => announceIssueChange({
      eventId,
      issueRef,
      issueKey,
      changeKind: "merge_request",
    });
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
    ) => awaitAutomationFiring(founder, automationId, eventId, {
      state: "terminal",
      description,
    });

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

  test("keeps a repository-scoped agent inside the repository it was given", async () => {
    const founder = await browserApprovedCliClient();
    const issue = await awaitActiveIssue(systemClient, uniqueName("Repository boundary"));
    const permittedRepository = new GitProject(uniqueName("agent-permitted"), founder);
    const protectedRepository = new GitProject(uniqueName("agent-out-of-scope"), founder);
    await permittedRepository.create();
    await protectedRepository.create();
    await protectedRepository.writeFile("main", "README.md", "# Outside the delegation\n");
    const proposedCommit = (await protectedRepository.writeFile(
      "feature/outside-delegation",
      "proposal.txt",
      "This change belongs to another repository.\n",
      { startRef: "main" },
    )).commitOid;
    const opened = await founder.json(`${protectedRepository.path}/prs`, {
      method: "POST",
      body: {
        title: "A merge outside the delegated repository",
        base_ref: "refs/heads/main",
        head_ref: "refs/heads/feature/outside-delegation",
        head_oid: proposedCommit,
        reviewers: [],
      },
      idempotencyKey: `out-of-scope-pr-${randomUUID()}`,
      expectedStatus: 201,
    });
    const pullRequest = record(
      record(opened.body.applied, "out-of-scope PR receipt").pr,
      "out-of-scope pull request",
    );
    const pullRequestNumber = integer(pullRequest.number, "out-of-scope PR number");
    const pullRequestRef =
      `myelin://${systemTestConfig.tenant}/git/pr/${protectedRepository.slug}:${pullRequestNumber}`;

    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: {
        name: uniqueName("Repository-bound merge companion"),
        runtime: "hosted",
        tools: ["git.merge"],
      },
      idempotencyKey: `repository-bound-agent-${randomUUID()}`,
      expectedStatus: 201,
    });
    const agentId = string(
      record(activated.body.agent, "repository-bound agent").id,
      "repository-bound agent id",
    );
    const created = await founder.json("/v1/triggers", {
      method: "POST",
      body: {
        event_type: "issue.issue.updated",
        filter: "payload.change_kind == 'merge_request'",
        run_as_agent_id: agentId,
        task: `Merge pull request ${protectedRepository.slug}#${pullRequestNumber}.`,
        budget_minor_units: 100_000,
        max_firings: 1,
        delegation_caveats: [`repo:${permittedRepository.slug}`, "pull_request.merge"],
      },
      idempotencyKey: `repository-bound-trigger-${randomUUID()}`,
      expectedStatus: 201,
    });
    const automationId = string(
      record(created.body.trigger, "repository-bound automation").id,
      "repository-bound automation id",
    );
    onTestFinished(async () => {
      await founder.json(`/v1/triggers/${encodeURIComponent(automationId)}/disable`, {
        method: "POST",
        body: {},
      });
    });

    const eventId = `out-of-scope-merge-${randomUUID()}`;
    await announceIssueChange({
      eventId,
      issueRef: string(issue.ref, "repository-bound issue ref"),
      issueKey: string(issue.key, "repository-bound issue key"),
      changeKind: "merge_request",
    });

    const firing = await awaitAutomationFiring(founder, automationId, eventId, {
      state: "terminal",
      resultState: "available",
      description: "the repository-scoped agent to finish without crossing its boundary",
    });
    expect(firing).toMatchObject({ outcome: "succeeded", terminal_reason: null });
    expect((await founder.json(
      `${protectedRepository.path}/prs/${pullRequestNumber}`,
    )).body).toMatchObject({ pr_state: "open" });
    expect(await findInboxItemMatching(
      founder,
      (item) => item.subject === pullRequestRef && item.state !== "done",
    )).toBeUndefined();

    const runId = string(firing.run_id, "repository-bound run id");
    const result = await founder.json(
      `/v1/triggers/${encodeURIComponent(automationId)}/runs/${encodeURIComponent(runId)}/result`,
    );
    expect(result.body).toMatchObject({
      result: {
        run_id: runId,
        answer: "The governed pull-request merge was refused; no merge was applied.",
      },
    });
  }, 90_000);
});
