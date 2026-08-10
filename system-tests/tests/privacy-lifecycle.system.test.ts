import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import { browserApprovedCliClient, privacyClient, uniqueName } from "../src/context.js";
import { ExternalEventBus, type ExternalEventEnvelope } from "../src/event-bus.js";
import { eventually } from "../src/eventually.js";
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

  return eventually<JsonRecord>(async () => {
    const response = await person.json(
      `/v1/issues/authorization-requests/${encodeURIComponent(requestEventId)}`,
      { expectedStatus: [200, 202] },
    );
    return response.status === 200 ? record(response.body.issue, "visible issue") : undefined;
  }, { description: `the privacy test issue ${requestEventId} to become visible` });
}

function issueChange(
  eventId: string,
  issueRef: string,
  issueKey: string,
  changeKind: string,
): ExternalEventEnvelope {
  const now = new Date().toISOString();
  return {
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
      change_kind: changeKind,
      changed_fields: ["description"],
    },
  };
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
    const bus = await ExternalEventBus.connect(systemTestConfig.natsUrl);
    try {
      expect((await bus.publish(
        issueChange(firstEventId, issueRef, issueKey, changeKind),
      )).duplicate).toBe(false);
    } finally {
      await bus.close();
    }

    const completed = await eventually<JsonRecord>(async () => {
      const response = await person.json(
        `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
      );
      return array(response.body.items, "privacy automation history")
        .map((item) => record(item, "privacy automation firing"))
        .find((item) => item.event_id === firstEventId && item.state === "terminal" &&
          item.result_state === "available");
    }, {
      description: "the person's hosted agent to leave a recoverable result",
      timeoutMs: 30_000,
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
    const secondBus = await ExternalEventBus.connect(systemTestConfig.natsUrl);
    try {
      expect((await secondBus.publish(
        issueChange(refusedEventId, issueRef, issueKey, changeKind),
      )).duplicate).toBe(false);
    } finally {
      await secondBus.close();
    }
    const refused = await eventually<JsonRecord>(async () => {
      const response = await person.json(
        `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
      );
      return array(response.body.items, "privacy automation history after erasure")
        .map((item) => record(item, "post-erasure automation firing"))
        .find((item) => item.event_id === refusedEventId && item.state === "terminal");
    }, {
      description: "post-erasure agent processing to be refused",
      timeoutMs: 30_000,
    });
    expect(refused).toMatchObject({ outcome: "failed", result_state: null });
    expect((await person.json("/v1/privacy/me/agent-data")).body).toMatchObject({
      agent_data: { state: "erased", recoverable_records: 0 },
    });
  });
});
