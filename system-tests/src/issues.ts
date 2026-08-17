import { eventually } from "./eventually.js";
import { integer, record, string, type JsonRecord } from "./json.js";
import type { SystemTestClient } from "./client.js";

export async function awaitAuthorizedIssue(
  client: SystemTestClient,
  requestEventId: string,
  description: string,
): Promise<JsonRecord> {
  return eventually<JsonRecord>(
    async () => {
      const response = await client.json(
        `/v1/issues/authorization-requests/${encodeURIComponent(requestEventId)}`,
        { expectedStatus: [200, 202] },
      );
      if (response.status === 200) {
        const body = record(response.body, "active issue authorization");
        if (string(body.status, "active issue authorization status") !== "active") {
          throw new Error("a completed issue authorization must identify itself as active");
        }
        return record(body.issue, "authorized issue");
      }

      const body = record(response.body, "pending issue authorization");
      if (string(body.status, "pending issue authorization status") !== "pending") {
        throw new Error("an incomplete issue authorization must identify itself as pending");
      }
      const retryAfterMs = integer(body.retry_after_ms, "issue authorization retry guidance");
      if (retryAfterMs !== 1_000) {
        throw new Error(`issue authorization advertised an unsupported ${retryAfterMs}ms retry`);
      }
      return undefined;
    },
    { description, intervalMs: 1_000 },
  );
}
