import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { browserApprovedCliClient, privacyClient, uniqueName } from "../src/context.js";
import { awaitAuthorizedIssue } from "../src/issues.js";
import { awaitAutomationFiring } from "../src/journeys/automations.js";
import { announceIssueChange } from "../src/journeys/issues.js";
import { integer, record, string, type JsonRecord } from "../src/json.js";
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

describe("a person's agent-data privacy lifecycle", () => {
  test("shows what is held, erases it once, and refuses to quietly rebuild it", async () => {
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

    const erased = await person.json("/v1/privacy/me/agent-data/erase", {
      method: "POST",
      body: {},
      idempotencyKey: false,
    });
    const receipt = record(erased.body.erasure, "agent-data erasure receipt");
    expect(receipt).toMatchObject({
      subject: "self",
      scope: "agent_data",
      erased: true,
      already_erased: false,
      traces_erased: 1,
      key_destroyed_this_request: true,
      key_unrecoverable: true,
      new_processing_blocked: true,
      irreversible: true,
    });
    expect(integer(receipt.model_steps_erased, "erased model replays")).toBeGreaterThanOrEqual(1);
    expect(integer(receipt.records_erased, "all erased agent records")).toBe(recoverableBefore);
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
  });
});
