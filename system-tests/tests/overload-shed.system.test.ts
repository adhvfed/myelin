import { describe, expect, onTestFinished, test } from "vitest";

import { browserApprovedCliClient, systemClient } from "../src/context.js";
import {
  activateExternalAgent,
  beginAgentRun,
  closeAgentRun,
} from "../src/journeys/agents.js";

// The product promise under test: machine traffic (CI, agents) saturating the
// edge is shed with 429 + Retry-After, while an interactive human keeps
// getting served from the reserved lane the whole time.
describe("overload shedding", () => {
  test(
    "a machine storm is shed with 429 + Retry-After while a human keeps working",
    async () => {
      // a real human-scheme session, approved through the browser device flow.
      const human = await browserApprovedCliClient();
      await human.json("/v1/whoami");

      // Myelin creates the other actor and its one-minute credential. These
      // requests intentionally omit both x-myelin-token-scheme and
      // x-myelin-run-class: Edge must classify the signed, durable agent
      // identity rather than trusting a caller to volunteer its traffic class.
      const agent = await activateExternalAgent(human, "overload probe", ["projects.list"]);
      const run = await beginAgentRun(human, agent.agent.id);
      onTestFinished(() => closeAgentRun(run));
      const agentPath = `/v1/agent-runs/${encodeURIComponent(run.run.id)}/mcp`;
      const stormSize = 300;
      const storm = Promise.all(
        Array.from({ length: stormSize }, (_, index) =>
          systemClient.request(agentPath, {
            method: "POST",
            authenticated: false,
            headers: { authorization: `Bearer ${run.credential.token}` },
            body: { jsonrpc: "2.0", id: index + 1, method: "tools/list" },
            expectedStatus: [200, 429, 503],
          }),
        ),
      );

      // interactive probes while the storm is in flight: strict 200s, no
      // shed status is ever acceptable on the human lane.
      const humanProbes: Array<Promise<unknown>> = [];
      for (let i = 0; i < 10; i += 1) {
        humanProbes.push(human.json("/v1/whoami"));
        await new Promise((resolve) => setTimeout(resolve, 20));
      }

      const results = await storm;
      await Promise.all(humanProbes);

      const admitted = results.filter((r) => r.status === 200);
      const shed = results.filter((r) => r.status === 429);
      expect(admitted.length).toBeGreaterThan(0);
      expect(shed.length).toBeGreaterThan(0);
      for (const response of shed) {
        expect(response.headers.get("retry-after")).toMatch(/^\d+$/);
      }

      // the lane recovers as soon as the storm drains.
      await systemClient.json(agentPath, {
        method: "POST",
        authenticated: false,
        headers: { authorization: `Bearer ${run.credential.token}` },
        body: { jsonrpc: "2.0", id: stormSize + 1, method: "tools/list" },
      });
    },
    120_000,
  );
});
