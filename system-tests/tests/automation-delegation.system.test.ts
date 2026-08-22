import { randomUUID } from "node:crypto";

import { describe, expect, onTestFinished, test } from "vitest";

import { findAutomation } from "../src/automations.js";
import { systemTestConfig } from "../src/config.js";
import { browserApprovedCliClient, privacyClient, uniqueName } from "../src/context.js";
import { ExternalEventBus, type ExternalEventEnvelope } from "../src/event-bus.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { array, record, string, type JsonRecord } from "../src/json.js";

async function mapInBatches<Input, Output>(
  inputs: readonly Input[],
  batchSize: number,
  operation: (input: Input) => Promise<Output>,
): Promise<Output[]> {
  const outputs: Output[] = [];
  for (let offset = 0; offset < inputs.length; offset += batchSize) {
    outputs.push(
      ...await Promise.all(inputs.slice(offset, offset + batchSize).map(operation)),
    );
  }
  return outputs;
}

describe("automation delegation", () => {
  test("refuses an ambiguous page instead of quietly showing the wrong automation history", async () => {
    const founder = await browserApprovedCliClient();

    for (const query of ["limt=1", "limit=01", "cursor="]) {
      const response = await founder.json(`/v1/triggers?${query}`, { expectedStatus: 400 });
      expect(response.body).toMatchObject({ error: { code: "bad_request" } });
      expect(JSON.stringify(response.body).length).toBeLessThan(2_048);
    }
  });

  test("shares a founder's view only through one short-lived, auditable agent run", async () => {
    const founder = await browserApprovedCliClient();
    const repository = new GitProject(uniqueName("audited-agent-read"), founder);
    await repository.create();

    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: {
        name: uniqueName("repository-reader"),
        tools: ["git.list_repositories"],
      },
      idempotencyKey: `reader-${randomUUID()}`,
      expectedStatus: 201,
    });
    const agent = record(activated.body.agent, "repository-reading agent");
    const agentId = string(agent.id, "repository-reading agent id");

    const started = await founder.json(`/v1/agents/${encodeURIComponent(agentId)}/runs`, {
      method: "POST",
      body: {},
      idempotencyKey: `reader-run-${randomUUID()}`,
      expectedStatus: 201,
    });
    const run = record(started.body.run, "repository-reading run");
    const credential = record(started.body.credential, "short-lived run credential");
    const runId = string(run.id, "repository-reading run id");
    const runToken = string(credential.token, "short-lived run token");

    // The credential cannot impersonate the founder at an ordinary product endpoint. Its one
    // useful door is MCP, where the agent remains the actor while the founder remains the access
    // subject. A successful result also proves both durable read-audit records were accepted.
    await founder.json("/v1/git/repos?limit=1", {
      token: runToken,
      tokenScheme: "agent",
      expectedStatus: 403,
    });
    const expectedSlug = `${systemTestConfig.tenant}/${repository.slug}`;
    let cursor: string | undefined;
    let found = false;
    for (let pageNumber = 1; pageNumber <= 100 && !found; pageNumber += 1) {
      const response = await founder.json(`/v1/agent-runs/${encodeURIComponent(runId)}/mcp`, {
        method: "POST",
        body: {
          jsonrpc: "2.0",
          id: pageNumber,
          method: "tools/call",
          params: {
            name: "git.list_repositories",
            arguments: { limit: 100, ...(cursor === undefined ? {} : { cursor }) },
          },
        },
        token: runToken,
        tokenScheme: "agent",
        expectedStatus: 200,
      });
      expect(JSON.stringify(response.body)).not.toContain(runToken);
      const result = record(response.body.result, "governed repository read");
      expect(result, `governed repository read failed: ${JSON.stringify(result)}`).toMatchObject({
        isError: false,
        _meta: { tool: "git.list_repositories", runToken: expect.any(String) },
      });
      const content = array(result.content, "governed repository read content");
      const repositoryPage = record(
        JSON.parse(
          string(
            record(content[0], "repository read content item").text,
            "repository read JSON",
          ),
        ),
        "repository page visible to the founder",
      );
      found = array(repositoryPage.items, "repositories visible through delegation")
        .some((item) => record(item, "visible repository").slug === expectedSlug);
      const nextCursor = record(repositoryPage.page, "repository page metadata").next_cursor;
      if (nextCursor === null) break;
      cursor = string(nextCursor, "next repository cursor");
    }
    expect(found, `the delegated catalogue did not contain ${expectedSlug}`).toBe(true);

    const closed = await founder.json(`/v1/agent-runs/${encodeURIComponent(runId)}/close`, {
      method: "POST",
      body: {},
      token: runToken,
      tokenScheme: "agent",
    });
    expect(closed.body).toMatchObject({ run: { id: runId, state: "closed" }, closed: true });
  });

  test("lets another actor wake an automation without feeding it echoes or over-deep work", async () => {
    const founder = await browserApprovedCliClient();
    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: {
        name: uniqueName("self-quiet-agent"),
        runtime: "hosted",
        tools: ["chat.read_messages"],
      },
      idempotencyKey: `self-quiet-agent-${randomUUID()}`,
      expectedStatus: 201,
    });
    const agentId = string(record(activated.body.agent, "self-quiet agent").id, "agent id");
    const conversation = await founder.json("/v1/chat/conversations", {
      method: "POST",
      body: {
        project_id: systemTestConfig.issues.projectId,
        channel: uniqueName("self-quiet-room"),
        topic: "Agents should not assign their own echoes back to themselves",
      },
      idempotencyKey: `self-quiet-room-${randomUUID()}`,
      expectedStatus: 201,
    });
    const conversationId = string(
      record(conversation.body.conversation, "self-quiet conversation").id,
      "conversation id",
    );
    const message = await founder.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "A stable visible message for the automation boundary." },
        idempotencyKey: `self-quiet-message-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    const messageId = string(message.body.message_id, "message id");
    const eventType = `chat.message.self_guard_${randomUUID().replaceAll("-", "")}_tested`;

    const created = await founder.json("/v1/triggers", {
      method: "POST",
      body: {
        event_type: eventType,
        run_as_agent_id: agentId,
        task: "Read the message and prepare one concise response.",
        budget_minor_units: 10_000,
        max_firings: 2,
        max_causal_depth: 2,
        require_human_approval: true,
      },
      idempotencyKey: `self-quiet-trigger-${randomUUID()}`,
      expectedStatus: 201,
    });
    const triggerId = string(
      record(created.body.trigger, "self-quiet automation").id,
      "automation id",
    );
    onTestFinished(async () => {
      await founder.json(`/v1/triggers/${encodeURIComponent(triggerId)}/disable`, {
        method: "POST",
        body: {},
        expectedStatus: 200,
      });
    });

    const now = new Date().toISOString();
    const selfEventId = `self-authored-chat-${randomUUID()}`;
    const overDeepEventId = `over-deep-chat-${randomUUID()}`;
    const humanEventId = `human-authored-chat-${randomUUID()}`;
    const base: ExternalEventEnvelope = {
      event_id: selfEventId,
      type_: eventType,
      schema_ver: 1,
      tenant: systemTestConfig.tenant,
      region: systemTestConfig.region,
      actor: {
        tenant: systemTestConfig.tenant,
        region: systemTestConfig.region,
        principal_id: `agent:${agentId}`,
        kind: {
          Agent: {
            runtime_ref: "hosted:luna",
            on_behalf_of: systemTestConfig.principal,
          },
        },
        data_role: "Processor",
        status: "Active",
      },
      subject: `myelin://${systemTestConfig.tenant}/chat/message/${messageId}`,
      aggregate: `channel:${conversationId}`,
      causation_id: null,
      correlation_id: selfEventId,
      caused_by: null,
      depth: 1,
      contains_personal_data: false,
      data_role: "Processor",
      visibility: "Internal",
      pii_key_ref: null,
      occurred_at: now,
      recorded_at: now,
      payload: { conversation_id: conversationId, message_id: messageId },
    };
    const bus = await ExternalEventBus.connect(systemTestConfig.natsUrl);
    try {
      expect((await bus.publish(base)).duplicate).toBe(false);
      expect((await bus.publish({
        ...base,
        event_id: overDeepEventId,
        correlation_id: overDeepEventId,
        depth: 3,
        actor: {
          tenant: systemTestConfig.tenant,
          region: systemTestConfig.region,
          principal_id: systemTestConfig.principal,
          kind: "Human",
          data_role: "Controller",
          status: "Active",
        },
        data_role: "Controller",
      })).duplicate).toBe(false);
      expect((await bus.publish({
        ...base,
        event_id: humanEventId,
        correlation_id: humanEventId,
        actor: {
          tenant: systemTestConfig.tenant,
          region: systemTestConfig.region,
          principal_id: systemTestConfig.principal,
          kind: "Human",
          data_role: "Controller",
          status: "Active",
        },
        data_role: "Controller",
      })).duplicate).toBe(false);
    } finally {
      await bus.close();
    }

    const firings = await eventually<JsonRecord[]>(async () => {
      const response = await founder.json(
        `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
      );
      const items = array(response.body.items, "self-quiet automation history")
        .map((item) => record(item, "self-quiet automation firing"));
      return items.some((item) => item.event_id === humanEventId) ? items : undefined;
    }, { description: "the other actor's event to reach the automation" });
    expect(firings).toEqual([
      expect.objectContaining({
        event_id: humanEventId,
        state: "awaiting_approval",
      }),
    ]);
    const firedEventIds = firings.map((firing) => firing.event_id);
    expect(firedEventIds).not.toContain(selfEventId);
    expect(firedEventIds).not.toContain(overDeepEventId);
    expect(await findAutomation(founder, triggerId)).toMatchObject({
      id: triggerId,
      firings_used: 1,
    });
  });

  test("narrows real authority and never stores a decorative promise", async () => {
    const founder = await browserApprovedCliClient();
    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: {
        name: uniqueName("careful-triage"),
        runtime: "hosted",
        tools: ["issues.create"],
      },
      idempotencyKey: `agent-${randomUUID()}`,
      expectedStatus: 201,
    });
    const agentId = string(record(activated.body.agent, "triage agent").id, "agent id");
    const task = `Open one issue in response to ${uniqueName("a-failed-build")}.`;
    const intent = (delegationCaveats: string[]) => ({
      event_type: "ci.run.failed",
      run_as_agent_id: agentId,
      task,
      budget_minor_units: 10_000,
      max_firings: 1,
      delegation_caveats: delegationCaveats,
    });

    for (const caveats of [["issue:create"], ["pull_request.merge"]]) {
      const refused = await founder.json("/v1/triggers", {
        method: "POST",
        body: intent(caveats),
        idempotencyKey: `refused-${randomUUID()}`,
        expectedStatus: 400,
      });
      expect(refused.body).toMatchObject({ error: { code: "bad_request" } });
    }

    const accepted = await founder.json("/v1/triggers", {
      method: "POST",
      body: intent(["issue.create", "repo:platform/api"]),
      idempotencyKey: `accepted-${randomUUID()}`,
      expectedStatus: 201,
    });
    const trigger = record(accepted.body.trigger, "scoped automation");
    expect(trigger).toMatchObject({
      run_as_agent_id: agentId,
      task,
      delegation_caveats: ["issue.create", "repo:platform/api"],
    });

    expect(
      await findAutomation(founder, string(trigger.id, "scoped automation id")),
    ).toMatchObject({ id: trigger.id, task });
  });

  test("explains one broken rule without silencing its healthy neighbour", async () => {
    const founder = await browserApprovedCliClient();
    const repository = new GitProject(uniqueName("diagnosable-automation"), founder);
    await repository.create();
    await repository.writeFile("main", "README.md", "# Diagnosable automation\n");
    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: {
        name: uniqueName("branch-observer"),
        runtime: "hosted",
        tools: ["git.read_file"],
      },
      idempotencyKey: `diagnostic-agent-${randomUUID()}`,
      expectedStatus: 201,
    });
    const agentId = string(record(activated.body.agent, "branch observer").id, "agent id");
    const createAutomation = async (
      branchRule: { filter?: string; source_branch?: string },
      task: string,
    ) => {
      const created = await founder.json("/v1/triggers", {
        method: "POST",
        body: {
          event_type: "git.ref.updated",
          ...branchRule,
          run_as_agent_id: agentId,
          task,
          budget_minor_units: 10_000,
          max_firings: 1,
          require_human_approval: true,
        },
        idempotencyKey: `diagnostic-trigger-${randomUUID()}`,
        expectedStatus: 201,
      });
      return string(record(created.body.trigger, task).id, `${task} id`);
    };
    const brokenId = await createAutomation(
      { filter: "payload.ref > 1" },
      "Explain why this branch rule cannot be evaluated.",
    );
    const healthyId = await createAutomation(
      { source_branch: "main" },
      "Park one visible main-branch update for review.",
    );

    try {
      const update = await repository.updateFile(
        "main",
        "README.md",
        "# Diagnosable automation\n\nOne rule must not silence another.\n",
      );
      const diagnostic = await eventually(async () => {
        const automation = await findAutomation(founder, brokenId);
        return automation?.last_evaluation_error === null
          ? undefined
          : automation?.last_evaluation_error;
      }, { description: "the owner-visible rule evaluation error" });
      expect(record(diagnostic, "rule evaluation diagnostic")).toMatchObject({
        code: "type_error",
        detail: "comparison is not defined over the operand types",
        event_id: expect.any(String),
        event_recorded_at: expect.any(String),
      });

      const healthyFiring = await eventually(async () => {
        const history = await founder.json(
          `/v1/triggers/${encodeURIComponent(healthyId)}/firings?limit=100`,
        );
        return array(history.body.items, "healthy automation history")
          .map((item) => record(item, "healthy automation firing"))
          .find((item) => item.state === "awaiting_approval");
      }, { description: "the healthy neighbouring automation to remain actionable" });
      expect(healthyFiring).toMatchObject({
        event_type: "git.ref.updated",
        state: "awaiting_approval",
        run_id: null,
      });
      expect(update.commitOid).toMatch(/^[0-9a-f]{40}$/);
    } finally {
      for (const triggerId of [brokenId, healthyId]) {
        await founder.json(`/v1/triggers/${encodeURIComponent(triggerId)}/disable`, {
          method: "POST",
          body: {},
        });
      }
    }
  });

  test("keeps one owner from crowding every collaborator out of an event", async () => {
    const founder = await browserApprovedCliClient(privacyClient);
    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: {
        name: uniqueName("capacity-companion"),
        runtime: "hosted",
        tools: ["issues.view"],
      },
      idempotencyKey: `capacity-agent-${randomUUID()}`,
      expectedStatus: 201,
    });
    const agentId = string(record(activated.body.agent, "capacity companion").id, "agent id");
    const eventType = `knowledge.row.capacity_${randomUUID().replaceAll("-", "")}_tested`;
    const intent = {
      event_type: eventType,
      run_as_agent_id: agentId,
      task: "Keep this event responsive to other people and their agents.",
      budget_minor_units: 10_000,
      max_firings: 1,
      delegation_caveats: [],
    };
    const ownerLimit = 100;
    const idempotencyKeys = Array.from(
      { length: ownerLimit },
      (_, slot) => `capacity-${slot}-${randomUUID()}`,
    );
    const replayKey = idempotencyKeys.at(0);
    if (!replayKey) throw new Error("the owner capacity story needs at least one slot");
    const triggerIds: string[] = [];

    try {
      const admitted = await mapInBatches(idempotencyKeys, 10, async (idempotencyKey) =>
        founder.json("/v1/triggers", {
          method: "POST",
          body: intent,
          idempotencyKey,
          expectedStatus: 201,
        })
      );
      for (const [index, response] of admitted.entries()) {
        expect(response.body).toMatchObject({ created: true, durable: true });
        triggerIds.push(string(
          record(response.body.trigger, `automation in owner slot ${index + 1}`).id,
          `automation id in owner slot ${index + 1}`,
        ));
      }
      const firstTriggerId = triggerIds.at(0);
      if (!firstTriggerId) throw new Error("the first admitted automation is missing");

      const replayed = await founder.json("/v1/triggers", {
        method: "POST",
        body: intent,
        idempotencyKey: replayKey,
        expectedStatus: 200,
      });
      expect(replayed.body).toMatchObject({
        created: false,
        durable: true,
        trigger: { id: firstTriggerId },
      });

      const crowdedOut = await founder.json("/v1/triggers", {
        method: "POST",
        body: intent,
        idempotencyKey: `one-too-many-${randomUUID()}`,
        expectedStatus: 409,
      });
      expect(crowdedOut.body).toMatchObject({
        error: {
          code: "conflict",
          message:
            "active automation limit reached for this event; pause or disable one of your automations before retrying",
        },
      });

      await founder.json(`/v1/triggers/${encodeURIComponent(firstTriggerId)}/pause`, {
        method: "POST",
        body: {},
      });
      const replacement = await founder.json("/v1/triggers", {
        method: "POST",
        body: intent,
        idempotencyKey: `replacement-${randomUUID()}`,
        expectedStatus: 201,
      });
      triggerIds.push(string(
        record(replacement.body.trigger, "replacement automation").id,
        "replacement automation id",
      ));

      const cannotOverbookByResume = await founder.json(
        `/v1/triggers/${encodeURIComponent(firstTriggerId)}/resume`,
        { method: "POST", body: {}, expectedStatus: 409 },
      );
      expect(cannotOverbookByResume.body).toMatchObject({
        error: {
          code: "conflict",
          message:
            "active automation limit reached for this event; pause or disable one of your automations before retrying",
        },
      });
    } finally {
      await mapInBatches(triggerIds, 20, async (triggerId) =>
        founder.json(`/v1/triggers/${encodeURIComponent(triggerId)}/disable`, {
          method: "POST",
          body: {},
        })
      );
    }
  }, 90_000);
});
