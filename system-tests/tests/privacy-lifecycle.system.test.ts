import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { browserApprovedCliClient, privacyClient, uniqueName } from "../src/context.js";
import { awaitAuthorizedIssue } from "../src/issues.js";
import { awaitAutomationFiring } from "../src/journeys/automations.js";
import { Conversation } from "../src/journeys/chat.js";
import { announceIssueChange } from "../src/journeys/issues.js";
import { array, integer, record, string, type JsonRecord } from "../src/json.js";
import type { SystemTestClient } from "../src/client.js";

async function createVisibleIssue(
  person: SystemTestClient,
  title: string,
  projectId: string,
  typeId: string,
  prefix: string,
): Promise<JsonRecord> {
  const proposed = await person.json("/v1/issues", {
    method: "POST",
    body: {
      project_id: projectId,
      type_id: typeId,
      prefix,
      title,
    },
    expectedStatus: 202,
  });
  const authorization = record(proposed.body.authorization, "issue authorization");
  const requestEventId = string(authorization.request_event_id, "authorization request id");

  return awaitAuthorizedIssue(
    person,
    requestEventId,
    `the privacy test issue ${requestEventId} to become visible`,
  );
}

describe("a person's privacy lifecycle", () => {
  test("shows what is held, completes one resumable request, and refuses to quietly rebuild it", async () => {
    await privacyClient.json("/v1/privacy/me/agent-data", { expectedStatus: 403 });
    const person = await browserApprovedCliClient(privacyClient);

    const empty = await person.json("/v1/privacy/me/agent-data");
    expect(empty.body).toMatchObject({
      agent_data: {
        subject: "self",
        scope: "agent_data",
        state: "active",
        recoverable_records: 0,
        holders: ["agent_traces", "model_replay", "tool_effects"],
        new_processing_allowed: true,
        erasure_is_irreversible: true,
      },
    });

    const prefix = `PR${randomUUID().replaceAll("-", "").slice(0, 6).toUpperCase()}`;
    const projectResponse = await person.json("/v1/projects", {
      method: "POST",
      body: { name: uniqueName("Private work"), issue_prefix: prefix },
      expectedStatus: 201,
    });
    const project = record(projectResponse.body.project, "private work project");
    const conversation = await Conversation.open(person, {
      projectId: string(project.id, "private work project id"),
      channel: uniqueName("private-work"),
      topic: "Work that must survive a narrow agent-data erasure",
    });
    const conversationMemory = uniqueName("Keep the migration plan in our private room");
    const conversationMessageId = await conversation.post(person, conversationMemory);
    expect(await conversation.messages(person)).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: conversationMessageId, content: conversationMemory }),
      ]),
    );
    const issue = await createVisibleIssue(
      person,
      uniqueName("Private agent work"),
      string(project.id, "private work project id"),
      string(project.default_issue_type_id, "private work issue type id"),
      prefix,
    );
    const issueRef = string(issue.ref, "privacy test issue ref");
    const issueKey = string(issue.key, "privacy test issue key");
    const agentResponse = await person.json("/v1/agents", {
      method: "POST",
      body: {
        name: uniqueName("Private work companion"),
        runtime: "hosted",
        tools: ["issues.view"],
      },
      expectedStatus: 201,
    });
    const agentId = string(
      record(agentResponse.body.agent, "privacy test agent").id,
      "privacy test agent id",
    );
    const changeKind = `privacy-${randomUUID().slice(0, 8)}`;
    const triggerResponse = await person.json("/v1/triggers", {
      method: "POST",
      body: {
        event_type: "issue.issue.updated",
        filter: `payload.change_kind == '${changeKind}'`,
        run_as_agent_id: agentId,
        task: "Read the issue and leave one small, durable work product.",
        budget_minor_units: 100_000,
        max_firings: 2,
      },
      expectedStatus: 201,
    });
    const triggerId = string(
      record(triggerResponse.body.trigger, "privacy test automation").id,
      "privacy test automation id",
    );

    const firstEventId = `privacy-work-${randomUUID()}`;
    await announceIssueChange({
      eventId: firstEventId,
      issueRef,
      issueKey,
      changeKind,
    });

    const completed = await awaitAutomationFiring(person, triggerId, firstEventId, {
      state: "terminal",
      resultState: "available",
      description: "the person's hosted agent to leave a recoverable result",
    });
    const runId = string(completed.run_id, "privacy test run id");
    await person.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/runs/${encodeURIComponent(runId)}/result`,
    );

    const beforeErasure = await person.json("/v1/privacy/me/agent-data");
    const held = record(beforeErasure.body.agent_data, "held agent data");
    expect(held).toMatchObject({ state: "active", new_processing_allowed: true });
    const recoverableBefore = integer(held.recoverable_records, "recoverable agent records");
    expect(recoverableBefore).toBeGreaterThanOrEqual(2);

    const retryKey = `erase-my-agent-data-${randomUUID()}`;
    const submitted = await person.json("/v1/privacy/me/requests", {
      method: "POST",
      body: { kind: "erasure", scope: "agent_data" },
      idempotencyKey: retryKey,
      expectedStatus: 201,
    });
    const request = record(submitted.body.request, "completed privacy request");
    const requestId = string(request.id, "privacy request id");
    expect(submitted.body.created).toBe(true);
    expect(request).toMatchObject({
      kind: "erasure",
      scope: "agent_data",
      state: "completed",
      attempt_count: 1,
      certificate_available: true,
    });
    expect(Date.parse(string(request.deadline_at, "privacy request deadline"))).toBeGreaterThan(
      Date.parse(string(request.submitted_at, "privacy request submission time")),
    );

    const status = await person.json(
      `/v1/privacy/me/requests/${encodeURIComponent(requestId)}`,
    );
    expect(status.body.request).toEqual(request);

    const certified = await person.json(
      `/v1/privacy/me/requests/${encodeURIComponent(requestId)}/certificate`,
    );
    const certificate = record(certified.body.certificate, "privacy request certificate");
    expect(certificate).toMatchObject({
      request_id: requestId,
      kind: "erasure",
      scope: "agent_data",
    });
    expect(string(certificate.content_hash, "certificate content hash")).toMatch(
      /^blake3:[0-9a-f]{64}$/,
    );
    const holders = array(certificate.holders, "certified privacy holders").map((holder) =>
      record(holder, "certified privacy holder"),
    );
    expect(holders.map((holder) => holder.holder)).toEqual([
      "agent_traces",
      "model_replay",
      "tool_effects",
    ]);
    expect(
      holders.reduce(
        (total, holder) => total + integer(holder.records_erased, "holder erasure count"),
        0,
      ),
    ).toBe(recoverableBefore);
    expect(holders).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          holder: "agent_traces",
          operation: "erasure",
          records_erased: 1,
          key_unrecoverable: true,
        }),
        expect.objectContaining({ holder: "model_replay", key_unrecoverable: true }),
        expect.objectContaining({ holder: "tool_effects", key_unrecoverable: true }),
      ]),
    );

    const replayed = await person.json("/v1/privacy/me/requests", {
      method: "POST",
      body: { kind: "erasure", scope: "agent_data" },
      idempotencyKey: retryKey,
    });
    expect(replayed.body).toMatchObject({
      created: false,
      request: { id: requestId, state: "completed", attempt_count: 1 },
    });
    await person.json(
      `/v1/triggers/${encodeURIComponent(triggerId)}/runs/${encodeURIComponent(runId)}/result`,
      { expectedStatus: 404 },
    );

    const afterErasure = await person.json("/v1/privacy/me/agent-data");
    expect(afterErasure.body).toMatchObject({
      agent_data: {
        state: "erased",
        recoverable_records: 0,
        new_processing_allowed: false,
        erasure_is_irreversible: true,
      },
    });
    const repeated = await person.json("/v1/privacy/me/agent-data/erase", {
      method: "POST",
      body: {},
      idempotencyKey: false,
    });
    expect(repeated.body).toMatchObject({
      erasure: {
        erased: true,
        already_erased: true,
        records_erased: 0,
        key_destroyed_this_request: false,
        key_unrecoverable: true,
        new_processing_blocked: true,
      },
    });

    const refusedEventId = `privacy-work-after-erasure-${randomUUID()}`;
    await announceIssueChange({
      eventId: refusedEventId,
      issueRef,
      issueKey,
      changeKind,
    });
    const refused = await awaitAutomationFiring(person, triggerId, refusedEventId, {
      state: "terminal",
      description: "post-erasure agent processing to be refused",
    });
    expect(refused).toMatchObject({
      outcome: "failed",
      result_state: null,
      terminal_reason: "agent processing is blocked by the owner's privacy settings",
    });
    expect((await person.json("/v1/privacy/me/agent-data")).body).toMatchObject({
      agent_data: { state: "erased", recoverable_records: 0 },
    });

    // A narrow privacy choice must not erase the person's work in another product.
    expect(await conversation.messages(person)).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: conversationMessageId, content: conversationMemory }),
      ]),
    );
    const followUp = uniqueName("The private room remains useful after the agent forgets");
    const followUpId = await conversation.post(person, followUp);
    expect(await conversation.messages(person)).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: followUpId, content: followUp }),
      ]),
    );
  });

  test("erases authored Chat history without erasing the person's right to speak again", async () => {
    const person = await browserApprovedCliClient(privacyClient);
    const agentDataBefore = (await person.json("/v1/privacy/me/agent-data")).body;
    const prefix = `CM${randomUUID().replaceAll("-", "").slice(0, 6).toUpperCase()}`;
    const projectResponse = await person.json("/v1/projects", {
      method: "POST",
      body: { name: uniqueName("Chat privacy"), issue_prefix: prefix },
      expectedStatus: 201,
    });
    const project = record(projectResponse.body.project, "Chat privacy project");
    const conversation = await Conversation.open(person, {
      projectId: string(project.id, "Chat privacy project id"),
      channel: uniqueName("erasable-history"),
      topic: "The person controls their authored Chat history",
    });
    const privateThoughts = [
      uniqueName("A launch concern I want removed"),
      uniqueName("A follow-up I also want removed"),
    ];
    const erasedIds = await Promise.all(
      privateThoughts.map((thought) => conversation.post(person, thought)),
    );

    const idempotencyKey = `erase-my-chat-messages-${randomUUID()}`;
    const submitted = await person.json("/v1/privacy/me/requests", {
      method: "POST",
      body: { kind: "erasure", scope: "chat_messages" },
      idempotencyKey,
      expectedStatus: 201,
    });
    const request = record(submitted.body.request, "completed Chat erasure request");
    const requestId = string(request.id, "Chat erasure request id");
    expect(request).toMatchObject({
      kind: "erasure",
      scope: "chat_messages",
      state: "completed",
      attempt_count: 1,
      certificate_available: true,
    });

    const history = await conversation.messages(person);
    for (const messageId of erasedIds) {
      expect(history).toEqual(expect.arrayContaining([
        expect.objectContaining({
          id: messageId,
          is_you: true,
          state: "tombstoned",
          content: "",
          nodes: [],
        }),
      ]));
    }
    expect(JSON.stringify(history)).not.toContain(privateThoughts[0]);
    expect(JSON.stringify(history)).not.toContain(privateThoughts[1]);

    const certified = await person.json(
      `/v1/privacy/me/requests/${encodeURIComponent(requestId)}/certificate`,
    );
    const certificate = record(certified.body.certificate, "Chat erasure certificate");
    expect(certificate).toMatchObject({
      request_id: requestId,
      kind: "erasure",
      scope: "chat_messages",
      holders: [expect.objectContaining({
        holder: "chat_messages",
        operation: "erasure",
        key_unrecoverable: true,
      })],
    });
    const holders = array(certificate.holders, "Chat erasure holders");
    expect(integer(record(holders[0], "Chat message holder").records_erased, "erased messages"))
      .toBeGreaterThanOrEqual(erasedIds.length);

    const freshThought = uniqueName("A new thought after clearing my history");
    const freshId = await conversation.post(person, freshThought);
    const replay = await person.json("/v1/privacy/me/requests", {
      method: "POST",
      body: { kind: "erasure", scope: "chat_messages" },
      idempotencyKey,
    });
    expect(replay.body).toMatchObject({
      created: false,
      request: { id: requestId, state: "completed", attempt_count: 1 },
    });
    expect(await conversation.messages(person)).toEqual(expect.arrayContaining([
      expect.objectContaining({ id: freshId, state: "active", content: freshThought }),
    ]));
    expect((await person.json("/v1/privacy/me/agent-data")).body).toEqual(agentDataBefore);

    const second = await person.json("/v1/privacy/me/requests", {
      method: "POST",
      body: { kind: "erasure", scope: "chat_messages" },
      idempotencyKey: `erase-my-new-chat-messages-${randomUUID()}`,
      expectedStatus: 201,
    });
    expect(second.body).toMatchObject({
      created: true,
      request: { scope: "chat_messages", state: "completed" },
    });
    expect(await conversation.messages(person)).toEqual(expect.arrayContaining([
      expect.objectContaining({ id: freshId, state: "tombstoned", content: "" }),
    ]));
  });
});
