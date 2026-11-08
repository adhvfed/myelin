import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { browserApprovedCliClient, reviewerClient, uniqueName } from "../src/context.js";
import { systemTestConfig } from "../src/config.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { proposeChange } from "../src/journeys/pull-requests.js";
import { array, record, string } from "../src/json.js";

describe("automations on pull-request reviews", () => {
  test("an agent is called in when a reviewer requests changes, not when they approve", async () => {
    const founder = await browserApprovedCliClient();
    const project = new GitProject(uniqueName("review-automation"), founder);
    await project.create();
    await project.writeFile("main", "README.md", "# Review automation\n");
    const change = await proposeChange(founder, project, {
      branch: "agent/first-draft",
      path: "draft.txt",
      contents: "first draft\n",
      title: "First draft",
      reviewers: [systemTestConfig.reviewerPrincipal],
    });
    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: { name: uniqueName("review-responder"), runtime: "hosted", tools: ["git.read_file"] },
      idempotencyKey: `review-agent-${randomUUID()}`,
      expectedStatus: 201,
    });
    const agentId = string(record(activated.body.agent, "review responder").id, "agent id");
    const created = await founder.json("/v1/triggers", {
      method: "POST",
      body: {
        event_type: "git.review.submitted",
        repository: project.slug,
        filter: "payload.review_verdict == 'request_changes'",
        run_as_agent_id: agentId,
        task: "Address the changes the reviewer requested on this pull request.",
        budget_minor_units: 10_000,
        max_firings: 2,
        require_human_approval: true,
      },
      idempotencyKey: `review-trigger-${randomUUID()}`,
      expectedStatus: 201,
    });
    const triggerId = string(record(created.body.trigger, "review trigger").id, "trigger id");
    const firings = async () =>
      array((await founder.json(
        `/v1/triggers/${encodeURIComponent(triggerId)}/firings?limit=100`,
      )).body.items, "review automation firings").map((item) => record(item, "firing"));

    try {
      const review = (verdict: string) =>
        reviewerClient.json(`${project.path}/prs/${change.number}/reviews`, {
          method: "POST",
          body: { verdict },
        });
      await review("approve");
      await review("request_changes");

      const firing = await eventually(async () =>
        (await firings()).find((item) => item.state === "awaiting_approval"), {
        description: "the requested changes to call in the review responder",
      });
      expect(firing).toMatchObject({
        event_type: "git.review.submitted",
        state: "awaiting_approval",
        run_id: null,
      });
      expect(await firings(), "the approval did not match the automation").toHaveLength(1);
    } finally {
      await founder.json(`/v1/triggers/${encodeURIComponent(triggerId)}/disable`, {
        method: "POST",
        body: {},
        idempotencyKey: `disable-review-trigger-${randomUUID()}`,
      });
    }
  }, 120_000);
});
