import { describe, expect, test } from "vitest";

import { systemClient } from "../src/context.js";
import { systemTestConfig } from "../src/config.js";

describe("running edge platform", () => {
  test("publishes live and ready probes before the suite starts", async () => {
    const live = await systemClient.json("/livez", { authenticated: false });
    expect(live.body).toEqual({ status: "ok" });
    expect(live.headers.get("cache-control")).toContain("no-store");

    const ready = await systemClient.json("/readyz", { authenticated: false });
    expect(ready.body).toEqual({ status: "ok" });
    expect(ready.headers.get("cache-control")).toContain("no-store");
  });

  test("authenticates the bootstrapped system-test principal", async () => {
    const response = await systemClient.json("/v1/whoami");
    expect(response.body).toMatchObject({
      principal_id: "system-test",
      tenant: systemTestConfig.tenant,
      region: systemTestConfig.region,
      kind: "human",
    });
  });

  test("fails closed without a valid capability", async () => {
    const missing = await systemClient.json("/v1/whoami", {
      authenticated: false,
      expectedStatus: 401,
    });
    expect(missing.body).toEqual({
      error: { code: "unauthorized", message: "authentication required" },
    });

    const forged = await systemClient.json("/v1/whoami", {
      authenticated: false,
      headers: {
        authorization: "Bearer forged-system-test-capability",
        "x-myelin-token-scheme": systemTestConfig.tokenScheme,
      },
      expectedStatus: 401,
    });
    expect(forged.body).toEqual({
      error: { code: "unauthorized", message: "authentication required" },
    });
  });

  test("returns a bounded not-found response for an unknown API route", async () => {
    const response = await systemClient.json("/v1/system-test-route-that-does-not-exist", {
      expectedStatus: 404,
    });
    expect(response.body).toEqual({
      error: {
        code: "not_found",
        message: "no route for GET /v1/system-test-route-that-does-not-exist",
      },
    });
    expect(JSON.stringify(response.body).length).toBeLessThan(1024);
  });
});
