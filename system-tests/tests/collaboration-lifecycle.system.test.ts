import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import {
  browserApprovedCliClient,
  reviewerClient,
  systemClient,
  uniqueName,
} from "../src/context.js";
import { ExternalEventBus, type ExternalEventEnvelope } from "../src/event-bus.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { array, integer, record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

async function awaitActiveIssue(title: string): Promise<JsonRecord> {
  const proposed = await systemClient.json("/v1/issues", {
    method: "POST",
    body: {
      project_id: systemTestConfig.issues.projectId,
      type_id: systemTestConfig.issues.typeId,
      prefix: systemTestConfig.issues.prefix,
      title,
    },
    expectedStatus: 202,
  });
  const authorization = record(proposed.body.authorization, "issue authorization");
  const requestEventId = string(authorization.request_event_id, "authorization request id");

  return eventually<JsonRecord>(
    async () => {
      const response = await systemClient.json(
        `/v1/issues/authorization-requests/${encodeURIComponent(requestEventId)}`,
        { expectedStatus: [200, 202] },
      );
      return response.status === 200 ? record(response.body.issue, "active issue") : undefined;
    },
    { description: `issue authorization ${requestEventId}` },
  );
}

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

    const triggerRetryKey = `trigger-${randomUUID()}`;
    const intent = {
      event_type: "ci.run.failed",
      source_branch: "main",
      run_as_agent_id: agentId,
      task: "Find the failure, open an issue, and prepare the smallest safe fix.",
      budget_minor_units: 250_000,
      max_firings: 10,
      max_causal_depth: 4,
      delegation_caveats: ["repo:core", "issue:create"],
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
    expect(created.body).toMatchObject({ created: true, durable: true });
    expect(trigger).toMatchObject({
      run_as_agent_id: agentId,
      event_type: "ci.run.failed",
      task: intent.task,
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

    const rediscovered = await founder.json("/v1/triggers?limit=100");
    expect(array(rediscovered.body.items, "founder's agent triggers")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: triggerId, run_as_agent_id: agentId }),
      ]),
    );

    const slug = uniqueName("triggered-ci");
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
    const run = await eventually<JsonRecord>(async () => {
      const response = await systemClient.json("/v1/ci/runs?state=all&limit=100");
      return array(response.body.items, "visible CI runs")
        .map((item) => record(item, "visible CI run"))
        .find((item) => item.repo_ref === repoRef && item.commit_oid === commitOid);
    }, { description: "the founder's mainline CI run to become visible" });
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
        commit_oid: commitOid,
        source_ref: "refs/heads/main",
        structured_failure: { failed_stage: "contract" },
      },
    };

    const bus = await ExternalEventBus.connect(systemTestConfig.natsUrl);
    try {
      expect((await bus.publish(failedMainline)).duplicate).toBe(false);
      expect((await bus.publish(failedMainline)).duplicate).toBe(true);
    } finally {
      await bus.close();
    }

    const fired = await eventually<JsonRecord>(async () => {
      const response = await founder.json("/v1/triggers?limit=100");
      const binding = array(response.body.items, "founder's agent triggers")
        .map((item) => record(item, "founder's agent trigger"))
        .find((item) => item.id === triggerId);
      return binding?.firings_used === 1 ? binding : undefined;
    }, { description: "the governed agent binding to reserve the red mainline event exactly once" });
    expect(fired).toMatchObject({
      id: triggerId,
      run_as_agent_id: agentId,
      firings_used: 1,
      state: "active",
    });

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
        .find((item) => item.event_id === eventId && item.state === "terminal");
    }, { description: "the hosted agent to complete its one governed workflow" });
    const hostedRunId = string(completedFiring.run_id, "completed hosted run id");
    expect(hostedRunId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
    expect(completedFiring).toMatchObject({
      event_id: eventId,
      event_type: "ci.run.failed",
      trigger_ref: `myelin://${systemTestConfig.tenant}/identity/trigger/${triggerId}`,
      state: "terminal",
      outcome: "succeeded",
      approval: {
        decision: "approved",
        decided_by: expect.any(String),
        decided_at: expect.any(String),
      },
      run_ref: `myelin://${systemTestConfig.tenant}/agent/run/${hostedRunId}`,
    });
    const triageTitle = `CI failure ${runId} needs triage`;
    const triageIssues = await eventually<JsonRecord[]>(async () => {
      const response = await systemClient.json("/v1/issues?state=open&limit=100");
      const matching = array(response.body.items, "open issues after the governed hosted run")
        .map((item) => record(item, "open issue after the governed hosted run"))
        .filter(
          (item) => item.title === triageTitle && item.created_by === `agent:${agentId}`,
        );
      return matching.length > 0 ? matching : undefined;
    }, { description: "the hosted agent to read CI and open exactly one governed issue" });
    expect(triageIssues).toHaveLength(1);
    expect(triageIssues[0]).toMatchObject({
      title: triageTitle,
      state: "Todo",
      created_by: `agent:${agentId}`,
      creator_kind: "agent",
    });
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

    const retired = await founder.json("/v1/triggers?limit=100");
    expect(array(retired.body.items, "founder's retired agent triggers")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: triggerId, state: "disabled", firings_used: 1 }),
      ]),
    );
  });

  test("holds a visible issue event for a human decision without pretending it is CI", async () => {
    const founder = await browserApprovedCliClient();
    const issue = await awaitActiveIssue(uniqueName("Automation review"));
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

    const history = await founder.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
    );
    expect(array(history.body.items, "filtered issue automation history")).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ event_id: ignoredEventId })]),
    );
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
  });

  test("lets a founder create and rediscover a project without operator-provided IDs", async () => {
    const name = uniqueName("Developer experience");
    const issuePrefix = `P${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`;
    const retryKey = `project-${randomUUID()}`;
    const created = await systemClient.json("/v1/projects", {
      method: "POST",
      body: { name, issue_prefix: issuePrefix },
      idempotencyKey: retryKey,
      expectedStatus: 201,
    });
    const project = record(created.body.project, "created project");
    const projectId = string(project.id, "project id");
    const projectRef = string(project.ref, "project ref");
    const defaultIssueTypeId = string(project.default_issue_type_id, "default issue type id");

    expect(created.body).toMatchObject({ created: true, durable: true });
    expect(project).toMatchObject({ name, issue_prefix: issuePrefix });
    expect(projectId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
    expect(defaultIssueTypeId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
    expect(projectRef).toBe(
      `myelin://${systemTestConfig.tenant}/identity/project/${projectId}`,
    );

    const replay = await systemClient.json("/v1/projects", {
      method: "POST",
      body: { name, issue_prefix: issuePrefix },
      idempotencyKey: retryKey,
      expectedStatus: 200,
    });
    expect(replay.body).toMatchObject({
      created: false,
      durable: true,
      project: { id: projectId, ref: projectRef },
    });

    const conflictingReplay = await systemClient.json("/v1/projects", {
      method: "POST",
      body: { name: `${name} changed`, issue_prefix: issuePrefix },
      idempotencyKey: retryKey,
      expectedStatus: 409,
    });
    expect(conflictingReplay.body).toMatchObject({ error: { code: "conflict" } });

    const rediscovered = await systemClient.json(`/v1/projects/${projectId}`);
    expect(rediscovered.body).toMatchObject({ project: { id: projectId, ref: projectRef, name } });

    const listed = await systemClient.json("/v1/projects?limit=100");
    expect(array(listed.body.items, "founder's projects")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: projectId, ref: projectRef, name })]),
    );

    const hiddenFromPeer = await reviewerClient.json(`/v1/projects/${projectId}`, {
      expectedStatus: 404,
    });
    expect(hiddenFromPeer.body).toMatchObject({ error: { code: "not_found" } });
    const peerProjects = await reviewerClient.json("/v1/projects?limit=100");
    expect(array(peerProjects.body.items, "peer's projects")).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ id: projectId })]),
    );
  });

  test("creates a public conversation and exchanges durable messages", async () => {
    const channel = uniqueName("system-chat");
    const topic = "Coordinate the externally tested release";
    const conversationRetryKey = `conversation-${randomUUID()}`;
    const created = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { channel, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 201,
    });
    const conversation = record(created.body.conversation, "created conversation");
    const conversationId = string(conversation.id, "conversation id");
    expect(created.body).toMatchObject({ durable: true });
    expect(conversation).toMatchObject({
      channel,
      topic,
    });

    const retriedCreate = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { channel, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 200,
    });
    expect(retriedCreate.body).toMatchObject({
      durable: true,
      conversation: { id: conversationId, channel, topic },
    });

    const conflictingRetry = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { channel: `${channel}-different`, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 409,
    });
    expect(conflictingRetry.body).toMatchObject({ error: { code: "conflict" } });

    const listed = await reviewerClient.json("/v1/chat/conversations?limit=100");
    expect(array(listed.body.items, "conversation list items")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: conversationId, channel })]),
    );

    const firstRetryKey = `author-${randomUUID()}`;
    const first = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "The lifecycle suite is running against the real backend." },
        idempotencyKey: firstRetryKey,
        expectedStatus: 201,
      },
    );
    expect(first.body).toMatchObject({ durable: true });
    const firstMessageId = string(first.body.message_id, "first message id");
    expect(firstMessageId).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);

    const replay = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "The lifecycle suite is running against the real backend." },
        idempotencyKey: firstRetryKey,
        expectedStatus: 201,
      },
    );
    expect(replay.body).toEqual(first.body);

    const reviewerMessage = await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: {
          content: "Confirmed from a second principal.",
          client_nonce: `reviewer-${randomUUID()}`,
        },
        expectedStatus: 201,
      },
    );
    const reviewerMessageId = string(reviewerMessage.body.message_id, "reviewer message id");

    const finalMessage = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: {
          content: "The paged history is consistent.",
          client_nonce: `author-final-${randomUUID()}`,
        },
        expectedStatus: 201,
      },
    );
    const finalMessageId = string(finalMessage.body.message_id, "final message id");

    const recent = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=2`,
    );
    const recentItems = array(recent.body.items, "recent conversation messages").map((item) =>
      record(item, "recent conversation message"),
    );
    expect(recentItems.map((item) => item.id)).toEqual([reviewerMessageId, finalMessageId]);
    const recentPage = record(recent.body.page, "recent message page");
    expect(recentPage).toMatchObject({ limit: 2, next_cursor: reviewerMessageId });

    const older = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=2&before=${encodeURIComponent(reviewerMessageId)}`,
    );
    expect(array(older.body.items, "older conversation messages")).toEqual([
      expect.objectContaining({ id: firstMessageId }),
    ]);
    expect(older.body).toMatchObject({ page: { limit: 2, next_cursor: null } });

    const messages = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=100`,
    );
    expect(messages.body).toMatchObject({ conversation: { id: conversationId, channel } });
    expect(array(messages.body.items, "conversation messages")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          content: "The lifecycle suite is running against the real backend.",
          is_you: true,
          state: "active",
        }),
        expect.objectContaining({
          content: "Confirmed from a second principal.",
          is_you: false,
          state: "active",
        }),
        expect.objectContaining({
          content: "The paged history is consistent.",
          is_you: true,
          state: "active",
        }),
      ]),
    );
  });

  test("creates, edits, and conflict-checks a durable knowledge page", async () => {
    const title = uniqueName("System-tested product spec");
    const retryKey = `knowledge-${randomUUID()}`;
    const createBody = {
      title,
      template: "product-spec",
      visibility: "team",
    };
    const created = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: createBody,
      idempotencyKey: retryKey,
      expectedStatus: 201,
    });
    const page = record(created.body.page, "created knowledge page");
    const pageId = string(page.id, "knowledge page id");
    const initialVersion = integer(page.version, "knowledge page version");
    expect(created.body).toMatchObject({ created: true, durable: true });
    expect(page).toMatchObject({ title, visibility: "team", can_edit: true });
    expect(array(page.blocks, "template blocks").length).toBeGreaterThan(1);

    const replay = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: createBody,
      idempotencyKey: retryKey,
      expectedStatus: 200,
    });
    expect(replay.body).toMatchObject({ created: false, durable: true, page: { id: pageId } });

    const reviewerView = await reviewerClient.json(
      `/v1/knowledge/pages/${encodeURIComponent(pageId)}`,
    );
    expect(reviewerView.body).toMatchObject({
      page: { id: pageId, title, visibility: "team", can_edit: false },
    });

    const reviewerSave = await reviewerClient.json(
      `/v1/knowledge/pages/${encodeURIComponent(pageId)}`,
      {
        method: "PUT",
        body: {
          expected_version: initialVersion,
          title: "A reviewer must not overwrite this page",
          visibility: "team",
          blocks: [{ type: "paragraph", markdown: "Unauthorized replacement." }],
        },
        expectedStatus: 404,
      },
    );
    expect(reviewerSave.body).toMatchObject({ error: { code: "not_found" } });

    const unchanged = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`);
    expect(unchanged.body).toMatchObject({
      page: { id: pageId, title, version: initialVersion, can_edit: true },
    });

    const editedTitle = `${title} — approved`;
    const saved = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`, {
      method: "PUT",
      body: {
        expected_version: initialVersion,
        title: editedTitle,
        visibility: "team",
        blocks: [
          { type: "heading", markdown: "Outcome" },
          { type: "paragraph", markdown: "Every engineering workflow is observable end to end." },
          { type: "task_list", markdown: "Keep the contract suite green." },
        ],
      },
    });
    const savedPage = record(saved.body.page, "saved knowledge page");
    const savedVersion = integer(saved.body.version, "saved knowledge version");
    expect(saved.body).toMatchObject({ durable: true });
    expect(savedVersion).toBeGreaterThan(initialVersion);
    expect(savedPage).toMatchObject({ id: pageId, title: editedTitle, version: savedVersion });
    expect(array(savedPage.blocks, "saved knowledge blocks")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ type: "paragraph", markdown: expect.stringContaining("observable") }),
      ]),
    );

    const stale = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`, {
      method: "PUT",
      body: {
        expected_version: initialVersion,
        title: "Stale overwrite",
        visibility: "team",
        blocks: [{ type: "paragraph", markdown: "This must not win." }],
      },
      expectedStatus: 409,
    });
    expect(stale.body).toHaveProperty("error");

    const persisted = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`);
    expect(persisted.body).toMatchObject({ page: { title: editedTitle, version: savedVersion } });
    const pages = await systemClient.json("/v1/knowledge/pages?limit=100");
    expect(array(pages.body.items, "knowledge list items")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: pageId, title: editedTitle })]),
    );
  });

  test("moves an issue through authorization, discovery, and completion", async () => {
    const title = uniqueName("Ship the backend lifecycle suite");
    const proposed = await systemClient.json("/v1/issues", {
      method: "POST",
      body: {
        project_id: systemTestConfig.issues.projectId,
        type_id: systemTestConfig.issues.typeId,
        prefix: systemTestConfig.issues.prefix,
        title,
      },
      expectedStatus: 202,
    });
    const summary = record(proposed.body.issue, "issue proposal");
    const authorization = record(proposed.body.authorization, "issue authorization");
    const issueId = string(summary.id, "issue id");
    const requestEventId = string(authorization.request_event_id, "authorization request id");
    expect(summary.key).toMatch(/^MYL-\d+$/);
    expect(authorization.status).toBe("pending");

    const active = await eventually<JsonRecord>(
      async () => {
        const response = await systemClient.json(
          `/v1/issues/authorization-requests/${encodeURIComponent(requestEventId)}`,
          { expectedStatus: [200, 202] },
        );
        if (response.status === 202) {
          expect(response.body.status).toBe("pending");
          return undefined;
        }
        expect(response.body.status).toBe("active");
        return record(response.body.issue, "active issue");
      },
      { description: `issue authorization ${requestEventId}` },
    );
    expect(active).toMatchObject({ id: issueId, title, state_category: "unstarted" });

    const viewed = await reviewerClient.json(`/v1/issues/${encodeURIComponent(issueId)}`);
    expect(viewed.body).toMatchObject({ id: issueId, title, state_category: "unstarted" });

    const open = await systemClient.json("/v1/issues?state=open&limit=100");
    expect(array(open.body.items, "open issues")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: issueId, title })]),
    );

    const closed = await systemClient.json(`/v1/issues/${encodeURIComponent(issueId)}/close`, {
      method: "POST",
      body: {},
    });
    expect(closed.body).toMatchObject({ id: issueId, title, state_category: "completed" });

    const retry = await systemClient.json(`/v1/issues/${encodeURIComponent(issueId)}/close`, {
      method: "POST",
      body: {},
    });
    expect(retry.body).toMatchObject({ id: issueId, state_category: "completed" });

    const closedList = await systemClient.json("/v1/issues?state=closed&limit=100");
    expect(array(closedList.body.items, "closed issues")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: issueId, title })]),
    );
  });

  test("hands an issue into a shared conversation by its canonical reference", async () => {
    const issue = await awaitActiveIssue(uniqueName("Coordinate the referenced rollout"));
    const issueRef = string(issue.ref, "issue reference");
    expect(issueRef).toMatch(/^myelin:\/\/[^/]+\/issue\/issue\/MYL-\d+$/);

    const created = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: {
        channel: uniqueName("system-chat-ref"),
        topic: "Share work without copying it",
      },
      idempotencyKey: `conversation-ref-${randomUUID()}`,
      expectedStatus: 201,
    });
    const conversation = record(created.body.conversation, "reference conversation");
    const conversationId = string(conversation.id, "reference conversation id");

    await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: {
          content: "Follow progress in \uFFFC.",
          references: [issueRef],
        },
        idempotencyKey: `message-ref-${randomUUID()}`,
        expectedStatus: 201,
      },
    );

    const history = await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=10`,
    );
    expect(array(history.body.items, "referenced conversation messages")).toEqual([
      expect.objectContaining({
        content: "Follow progress in \uFFFC.",
        nodes: [{ kind: "artifact_ref", ref: issueRef }],
      }),
    ]);
  });
});
