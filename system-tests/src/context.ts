import { SystemTestClient } from "./client.js";
import { systemTestConfig } from "./config.js";

export const systemClient = new SystemTestClient(systemTestConfig);
export const reviewerClient = new SystemTestClient({
  ...systemTestConfig,
  token: systemTestConfig.reviewerToken,
});

export function uniqueName(prefix: string): string {
  const run = systemTestConfig.runId.replaceAll("-", "").slice(0, 10).toLowerCase();
  return `${prefix}-${Date.now().toString(36)}-${run}`;
}
