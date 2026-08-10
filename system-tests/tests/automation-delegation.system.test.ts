import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { browserApprovedCliClient, uniqueName } from "../src/context.js";
import { array, record, string } from "../src/json.js";

describe("automation delegation", () => {
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

    const rediscovered = await founder.json("/v1/triggers?limit=100");
    const matching = array(rediscovered.body.items, "founder's automations")
      .map((item) => record(item, "automation"))
      .filter((item) => item.task === task);
    expect(matching).toEqual([expect.objectContaining({ id: trigger.id })]);
  });
});
