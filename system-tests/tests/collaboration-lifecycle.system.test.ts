import { randomUUID } from "node:crypto";

import { describe, expect, onTestFinished, test } from "vitest";

import { findAutomation } from "../src/automations.js";
import {
  browserApprovedCliClient,
  reviewerClient,
  systemClient,
  uniqueName,
} from "../src/context.js";
import { ExternalEventBus, type ExternalEventEnvelope } from "../src/event-bus.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { awaitTheOnlyCiRun } from "../src/journeys/ci-runs.js";
import { findInboxItemMatching } from "../src/journeys/inbox.js";
import {
  awaitActiveIssue,
  expectOpaqueIssueAuthor,
  issuesMatching,
} from "../src/journeys/issues.js";
import { array, integer, record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

describe("collaboration lifecycle", () => {
  test("reads red mainline CI and opens one governed issue without an integration API key", async () => {
    const founder = await browserApprovedCliClient();
    const agentName = uniqueName("triage-bot");
    const agentRetryKey = `agent-${randomUUID()}`;
    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: {
        name: agentName,
        runtime: "hosted",
        tools: ["ci.read_run", "issues.create"],
      },
      idempotencyKey: agentRetryKey,
      expectedStatus: 201,
    });
    const agent = record(activated.body.agent, "activated triage agent");
    const agentId = string(agent.id, "triage agent id");
    const colleague = await browserApprovedCliClient(reviewerClient);

    const colleagueRetirement = await colleague.json(
      `/v1/agents/${encodeURIComponent(agentId)}/retire`,
      {
        method: "POST",
        body: {},
        idempotencyKey: `colleague-agent-retire-${randomUUID()}`,
        expectedStatus: 404,
      },
    );
    expect(colleagueRetirement.body).toMatchObject({ error: { code: "not_found" } });

    const borrowedAgentTrigger = await colleague.json("/v1/triggers", {
      method: "POST",
      body: {
        event_type: "ci.run.failed",
        run_as_agent_id: agentId,
        task: "Spend somebody else’s agent budget.",
        budget_minor_units: 1,
        max_firings: 1,
      },
      idempotencyKey: `colleague-trigger-${randomUUID()}`,
      expectedStatus: 409,
    });
    expect(borrowedAgentTrigger.body).toMatchObject({ error: { code: "conflict" } });

    const slug = uniqueName("triggered-ci");
    const triggerRetryKey = `trigger-${randomUUID()}`;
    const intent = {
      event_type: "ci.run.failed",
      repository: slug,
      source_branch: "main",
      run_as_agent_id: agentId,
      task: "Find the failure, open an issue, and prepare the smallest safe fix.",
      budget_minor_units: 250_000,
      max_firings: 10,
      max_causal_depth: 4,
      delegation_caveats: ["run.view", "issue.create"],
      require_human_approval: true,
    };
    const created = await founder.json("/v1/triggers", {
      method: "POST",
      body: intent,
      idempotencyKey: triggerRetryKey,
      expectedStatus: 201,
    });
    const trigger = record(created.body.trigger, "created CI trigger");
    const triggerId = string(trigger.id, "CI trigger id");
    onTestFinished(async () => {
      await founder.json(`/v1/triggers/${encodeURIComponent(triggerId)}/disable`, {
        method: "POST",
        body: {},
        expectedStatus: 200,
      });
    });
    expect(created.body).toMatchObject({ created: true, durable: true });
    expect(trigger).toMatchObject({
      run_as_agent_id: agentId,
      event_type: "ci.run.failed",
      task: intent.task,
      condition:
        `event.type == 'ci.run.failed' AND ` +
        `payload.repo_ref == 'myelin://${systemTestConfig.tenant}/git/repo/${slug}' AND ` +
        `payload.source_ref == 'refs/heads/main'`,
      delegation_caveats: ["run.view", "issue.create", `repo:${slug}`],
      budget_minor_units: 250_000,
      max_firings: 10,
      firings_used: 0,
      require_no_personal_data: true,
      require_human_approval: true,
      state: "active",
    });

    const replay = await founder.json("/v1/triggers", {
      method: "POST",
      body: intent,
      idempotencyKey: triggerRetryKey,
      expectedStatus: 200,
    });
    expect(replay.body).toMatchObject({
      created: false,
      durable: true,
      trigger: { id: triggerId, run_as_agent_id: agentId },
    });

    const conflict = await founder.json("/v1/triggers", {
      method: "POST",
      body: { ...intent, budget_minor_units: 500_000 },
      idempotencyKey: triggerRetryKey,
      expectedStatus: 409,
    });
    expect(conflict.body).toMatchObject({ error: { code: "conflict" } });

    expect(await findAutomation(founder, triggerId)).toMatchObject({
      id: triggerId,
      run_as_agent_id: agentId,
    });

    const project = new GitProject(slug, systemClient);
    const repoRef = `myelin://${systemTestConfig.tenant}/git/repo/${slug}`;
    await project.create();
    await project.writeFile("main", "README.md", `# ${slug}\n`);
    const commitOid = (await project.writeFile("main", ".myelin/ci.toml", `on = "push"

[[jobs]]
name = "contract"
image = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000"
command = ["true"]
`)).commitOid;
    const run = await awaitTheOnlyCiRun(
      systemClient,
      (candidate) => candidate.repo_ref === repoRef && candidate.commit_oid === commitOid,
      "the founder's mainline CI run to become visible",
    );
    const runId = string(run.run_id, "triggering CI run id");
    const eventId = `ci-failed-${randomUUID()}`;
    const now = new Date().toISOString();
    const failedMainline: ExternalEventEnvelope = {
      event_id: eventId,
      type_: "ci.run.failed",
      schema_ver: 1,
      tenant: systemTestConfig.tenant,
      region: systemTestConfig.region,
      actor: {
        tenant: systemTestConfig.tenant,
        region: systemTestConfig.region,
        principal_id: "ci-controlplane",
        kind: "Service",
        data_role: "Controller",
        status: "Active",
      },
      subject: `myelin://${systemTestConfig.tenant}/ci/run/${runId}`,
      aggregate: `run:${runId}`,
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
      payload: {
        run: `myelin://${systemTestConfig.tenant}/ci/run/${runId}`,
        repo_ref: repoRef,
        commit_oid: commitOid,
        source_ref: "refs/heads/main",
        structured_failure: { failed_stage: "contract" },
      },
    };

    const bus = await ExternalEventBus.connect(systemTestConfig.natsUrl);
    const neighbourEventId = `ci-failed-neighbour-${randomUUID()}`;
    try {
      expect((await bus.publish({
        ...failedMainline,
        event_id: neighbourEventId,
        correlation_id: neighbourEventId,
        payload: {
          ...record(failedMainline.payload, "selected repository CI payload"),
          repo_ref: `myelin://${systemTestConfig.tenant}/git/repo/a-neighbour`,
        },
      })).duplicate).toBe(false);
      expect((await bus.publish(failedMainline)).duplicate).toBe(false);
      expect((await bus.publish(failedMainline)).duplicate).toBe(true);
    } finally {
      await bus.close();
    }

    const fired = await eventually<JsonRecord>(async () => {
      const binding = await findAutomation(founder, triggerId);
      return binding?.firings_used === 1 ? binding : undefined;
    }, { description: "the governed agent binding to reserve the red mainline event exactly once" });
    expect(fired).toMatchObject({
      id: triggerId,
      run_as_agent_id: agentId,
      firings_used: 1,
      state: "active",
    });
    const scopedHistory = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
    );
    expect(array(scopedHistory.body.items, "repository-scoped firing history"))
      .not.toEqual(expect.arrayContaining([
        expect.objectContaining({ event_id: neighbourEventId }),
      ]));

    const awaitingApproval = await eventually<JsonRecord>(async () => {
      const response = await founder.json(
        `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
      );
      return array(response.body.items, "guarded trigger firing history")
        .map((item) => record(item, "guarded trigger firing"))
        .find((item) => item.event_id === eventId && item.state === "awaiting_approval");
    }, { description: "the red mainline event to wait without spending its agent budget" });
    expect(awaitingApproval).toMatchObject({
      event_id: eventId,
      state: "awaiting_approval",
      run_id: null,
      terminal_reason: null,
      approval: null,
    });

    const peerApproval = await reviewerClient.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/firings/approve`,
      { method: "POST", body: { event_id: eventId }, expectedStatus: 403 },
    );
    expect(peerApproval.body).toMatchObject({ error: { code: "forbidden" } });

    const approved = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/firings/approve`,
      { method: "POST", body: { event_id: eventId }, expectedStatus: 200 },
    );
    expect(approved.body).toMatchObject({
      action: "approve",
      changed: true,
      durable: true,
      firing: {
        event_id: eventId,
        state: "queued",
        run_id: null,
        approval: {
          decision: "approved",
          decided_by: expect.any(String),
          decided_at: expect.any(String),
        },
      },
    });
    const retriedApproval = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/firings/approve`,
      { method: "POST", body: { event_id: eventId }, expectedStatus: 200 },
    );
    expect(retriedApproval.body).toMatchObject({
      action: "approve",
      changed: false,
      firing: { event_id: eventId, approval: { decision: "approved" } },
    });
    await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/firings/reject`,
      { method: "POST", body: { event_id: eventId }, expectedStatus: 409 },
    );

    const completedFiring = await eventually<JsonRecord>(async () => {
      const response = await founder.json(
        `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
      );
      return array(response.body.items, "governed trigger firing history")
        .map((item) => record(item, "governed trigger firing"))
        .find((item) => item.event_id === eventId && item.state === "terminal" &&
          item.result_state === "available");
    }, { description: "the hosted agent to complete with one readable durable work product" });
    const hostedRunId = string(completedFiring.run_id, "completed hosted run id");
    expect(hostedRunId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
    expect(completedFiring).toMatchObject({
      event_id: eventId,
      event_type: "ci.run.failed",
      trigger_ref: `myelin://${systemTestConfig.tenant}/identity/trigger/${triggerId}`,
      state: "terminal",
      outcome: "succeeded",
      result_state: "available",
      terminal_reason: null,
      approval: {
        decision: "approved",
        decided_by: expect.any(String),
        decided_at: expect.any(String),
      },
      run_ref: `myelin://${systemTestConfig.tenant}/agent/run/${hostedRunId}`,
    });
    const resultResponse = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/runs/${encodeURIComponent(hostedRunId)}/result`,
    );
    const result = record(resultResponse.body.result, "completed hosted agent result");
    expect(result).toMatchObject({
      run_id: hostedRunId,
      run_ref: `myelin://${systemTestConfig.tenant}/agent/run/${hostedRunId}`,
      trace_ref: expect.stringMatching(
        new RegExp(`^myelin://${systemTestConfig.tenant}/knowledge/doc/blake3:[0-9a-f]{64}$`),
      ),
      agent_principal: `agent:${agentId}`,
      answer: "Read the failing CI run and opened one governed triage issue.",
      charged_micro: expect.any(Number),
      recorded_at: expect.any(String),
    });
    expect(integer(result.charged_micro, "hosted agent result charge")).toBeGreaterThan(0);
    expect((await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/runs/${encodeURIComponent(hostedRunId)}/result`,
    )).body).toEqual(resultResponse.body);
    await reviewerClient.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/runs/${encodeURIComponent(hostedRunId)}/result`,
      { expectedStatus: 403 },
    );
    const triageTitle = `CI failure ${runId} needs triage`;
    const triageIssues = await eventually<JsonRecord[]>(async () => {
      const matching = await issuesMatching(
        systemClient,
        (item) => item.title === triageTitle,
        { state: "open" },
      );
      return matching.length > 0 ? matching : undefined;
    }, { description: "the hosted agent to read CI and open exactly one governed issue" });
    expect(triageIssues).toHaveLength(1);
    const triageIssue = record(triageIssues[0], "the agent's one governed issue");
    expect(triageIssue).toMatchObject({
      title: triageTitle,
      state: "Todo",
      creator_kind: "agent",
    });
    const publicAgentAuthor = expectOpaqueIssueAuthor(
      triageIssue.created_by,
      "hosted agent issue author",
    );
    expect(publicAgentAuthor).not.toContain(agentId);
    expect(JSON.stringify(triageIssue)).not.toContain(`agent:${agentId}`);
    const erasePath =
      `/v1/triggers/${encodeURIComponent(triggerId)}`
      + `/runs/${encodeURIComponent(hostedRunId)}/result/erase`;
    await reviewerClient.json(erasePath, {
      method: "POST",
      body: {},
      expectedStatus: 403,
    });
    const erased = await founder.json(erasePath, {
      method: "POST",
      body: {},
      expectedStatus: 200,
    });
    expect(erased.body).toMatchObject({
      erasure: {
        run_id: hostedRunId,
        run_ref: `myelin://${systemTestConfig.tenant}/agent/run/${hostedRunId}`,
        trace_ref: result.trace_ref,
        erased: true,
        already_erased: false,
        available_results: 0,
        recreation_blocked: true,
      },
    });
    const replayedErasure = await founder.json(erasePath, {
      method: "POST",
      body: {},
      expectedStatus: 200,
    });
    expect(replayedErasure.body).toMatchObject({
      erasure: {
        run_id: hostedRunId,
        trace_ref: result.trace_ref,
        erased: true,
        already_erased: true,
        available_results: 0,
        recreation_blocked: true,
      },
    });
    await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/runs/${encodeURIComponent(hostedRunId)}/result`,
      { expectedStatus: 404 },
    );
    const historyAfterErasure = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
    );
    expect(array(historyAfterErasure.body.items, "history after agent result erasure"))
      .toEqual(expect.arrayContaining([
        expect.objectContaining({
          event_id: eventId,
          run_id: hostedRunId,
          outcome: "succeeded",
          result_state: "erased",
        }),
      ]));
    const peerHistory = await reviewerClient.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
      { expectedStatus: 403 },
    );
    expect(peerHistory.body).toMatchObject({ error: { code: "forbidden" } });

    const peerPause = await reviewerClient.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/pause`,
      { method: "POST", body: {}, expectedStatus: 403 },
    );
    expect(peerPause.body).toMatchObject({ error: { code: "forbidden" } });

    const paused = await founder.json(`/v1/triggers/${encodeURIComponent(triggerId)}/pause`, {
      method: "POST",
      body: {},
      expectedStatus: 200,
    });
    expect(paused.body).toMatchObject({
      action: "pause",
      changed: true,
      canceled_firings: 0,
      durable: true,
      trigger: { id: triggerId, state: "paused", firings_used: 1 },
    });

    const retriedPause = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/pause`,
      { method: "POST", body: {}, expectedStatus: 200 },
    );
    expect(retriedPause.body).toMatchObject({
      action: "pause",
      changed: false,
      canceled_firings: 0,
      trigger: { id: triggerId, state: "paused" },
    });

    const resumed = await founder.json(`/v1/triggers/${encodeURIComponent(triggerId)}/resume`, {
      method: "POST",
      body: {},
      expectedStatus: 200,
    });
    expect(resumed.body).toMatchObject({
      action: "resume",
      changed: true,
      trigger: { id: triggerId, state: "active" },
    });

    const disabled = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/disable`,
      { method: "POST", body: {}, expectedStatus: 200 },
    );
    expect(disabled.body).toMatchObject({
      action: "disable",
      changed: true,
      canceled_firings: 0,
      durable: true,
      trigger: { id: triggerId, state: "disabled", firings_used: 1 },
    });

    const cannotRevive = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/resume`,
      { method: "POST", body: {}, expectedStatus: 409 },
    );
    expect(cannotRevive.body).toMatchObject({ error: { code: "conflict" } });

    expect(await findAutomation(founder, triggerId)).toMatchObject({
      id: triggerId,
      state: "disabled",
      firings_used: 1,
    });
  });

  test("holds a visible issue event for a human decision without pretending it is CI", async () => {
    const founder = await browserApprovedCliClient();
    const issue = await awaitActiveIssue(systemClient, uniqueName("Automation review"));
    const issueRef = string(issue.ref, "visible issue ref");
    const issueKey = string(issue.key, "visible issue key");

    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: {
        name: uniqueName("Issue change companion"),
        runtime: "hosted",
        tools: ["issues.view"],
      },
      idempotencyKey: `issue-event-agent-${randomUUID()}`,
      expectedStatus: 201,
    });
    const agentId = string(record(activated.body.agent, "issue agent").id, "issue agent id");
    const created = await founder.json("/v1/triggers", {
      method: "POST",
      body: {
        event_type: "issue.issue.updated",
        filter: "payload.change_kind == 'ownership'",
        run_as_agent_id: agentId,
        task: "Read the changed issue and propose the next smallest useful step.",
        budget_minor_units: 100_000,
        max_firings: 1,
        require_human_approval: true,
      },
      idempotencyKey: `issue-event-trigger-${randomUUID()}`,
      expectedStatus: 201,
    });
    const trigger = record(created.body.trigger, "issue automation");
    const triggerId = string(trigger.id, "issue automation id");
    expect(trigger).toMatchObject({
      event_type: "issue.issue.updated",
      subject_type: "issue",
      condition:
        "event.type == 'issue.issue.updated' AND (payload.change_kind == 'ownership')",
      require_human_approval: true,
      firings_used: 0,
    });

    const ignoredEventId = `issue-title-updated-${randomUUID()}`;
    const eventId = `issue-owner-updated-${randomUUID()}`;
    const now = new Date().toISOString();
    const ownershipChange: ExternalEventEnvelope = {
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
      payload: {
        issue: issueRef,
        change_kind: "ownership",
        changed_fields: ["assignee"],
      },
    };
    const bus = await ExternalEventBus.connect(systemTestConfig.natsUrl);
    try {
      const ignoredTitleChange = await bus.publish({
        ...ownershipChange,
        event_id: ignoredEventId,
        correlation_id: ignoredEventId,
        payload: {
          issue: issueRef,
          change_kind: "title",
          changed_fields: ["title"],
        },
      });
      expect(ignoredTitleChange.duplicate).toBe(false);
      expect((await bus.publish(ownershipChange)).duplicate).toBe(false);
    } finally {
      await bus.close();
    }

    const awaiting = await eventually<JsonRecord>(async () => {
      const response = await founder.json(
        `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
      );
      return array(response.body.items, "issue automation history")
        .map((item) => record(item, "issue automation firing"))
        .find((item) => item.event_id === eventId && item.state === "awaiting_approval");
    }, { description: "the visible issue event to reach its exact human gate" });
    expect(awaiting).toMatchObject({ event_id: eventId, run_id: null, approval: null });

    const approvalNotice = await eventually<JsonRecord>(async () => {
      return findInboxItemMatching(
        founder,
        (item) => item.action !== null && typeof item.action === "object" &&
          record(item.action, "inbox action").event_id === eventId,
      );
    }, { description: "the exact parked automation to appear in its owner's inbox" });
    expect(approvalNotice).toMatchObject({
      reason: "approval_requested",
      class: "critical",
      subject: issueRef,
      state: "unread",
      coalesce_count: 1,
      action: {
        kind: "automation_firing_approval",
        automation_id: triggerId,
        event_id: eventId,
      },
    });
    const approvalNoticeId = string(approvalNotice.id, "automation approval inbox item id");

    const history = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
    );
    expect(array(history.body.items, "filtered issue automation history")).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ event_id: ignoredEventId })]),
    );
    expect(await findInboxItemMatching(
      founder,
      (item) => item.action !== null && typeof item.action === "object" &&
        record(item.action, "filtered inbox action").event_id === ignoredEventId,
    )).toBeUndefined();
    const afterMatch = await founder.json(`/v1/triggers/${encodeURIComponent(triggerId)}`);
    expect(afterMatch.body).toMatchObject({ trigger: { id: triggerId, firings_used: 1 } });

    const rejected = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/firings/reject`,
      { method: "POST", body: { event_id: eventId }, expectedStatus: 200 },
    );
    expect(rejected.body).toMatchObject({
      action: "reject",
      changed: true,
      firing: {
        event_id: eventId,
        state: "terminal",
        run_id: null,
        approval: { decision: "rejected", decided_by: expect.any(String) },
      },
    });
    const completedNotice = await eventually<JsonRecord>(async () => {
      return findInboxItemMatching(
        founder,
        (item) => item.id === approvalNoticeId && item.state === "done",
      );
    }, { description: "the decided automation approval to leave the active inbox" });
    expect(completedNotice.action).toEqual({
      kind: "automation_firing_approval",
      automation_id: triggerId,
      event_id: eventId,
    });
  });

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
  });

});
