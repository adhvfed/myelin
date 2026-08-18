import { describe, expect, test } from "vitest";

import { browserApprovedCliClient, systemClient } from "../src/context.js";

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

      // the storm self-identifies as machine work the way a well-behaved
      // runner does: the run-class header demotes it to the batch lane.
      const stormSize = 300;
      const storm = Promise.all(
        Array.from({ length: stormSize }, () =>
          systemClient.request("/v1/whoami", {
            headers: { "x-myelin-run-class": "batch-ci" },
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
      await systemClient.json("/v1/whoami", {
        headers: { "x-myelin-run-class": "batch-ci" },
      });
    },
    120_000,
  );
});
