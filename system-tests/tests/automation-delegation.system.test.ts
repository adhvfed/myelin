import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { findAutomation } from "../src/automations.js";
import { systemTestConfig } from "../src/config.js";
import { browserApprovedCliClient, privacyClient, uniqueName } from "../src/context.js";
import { GitProject } from "../src/git-project.js";
import { array, record, string } from "../src/json.js";

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
    const response = await founder.json(`/v1/agent-runs/${encodeURIComponent(runId)}/mcp`, {
      method: "POST",
      body: {
        jsonrpc: "2.0",
        id: 1,
        method: "tools/call",
        params: { name: "git.list_repositories", arguments: { limit: 100 } },
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
    expect(array(repositoryPage.items, "repositories visible through delegation")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ slug: `${systemTestConfig.tenant}/${repository.slug}` }),
      ]),
    );

    const closed = await founder.json(`/v1/agent-runs/${encodeURIComponent(runId)}/close`, {
      method: "POST",
      body: {},
      token: runToken,
      tokenScheme: "agent",
    });
    expect(closed.body).toMatchObject({ run: { id: runId, state: "closed" }, closed: true });
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
