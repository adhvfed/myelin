import type { SystemTestClient } from "./client.js";
import { array, record, string, type JsonRecord } from "./json.js";

export async function findAutomation(
  client: SystemTestClient,
  automationId: string,
): Promise<JsonRecord | undefined> {
  let cursor: string | undefined;
  do {
    const query = new URLSearchParams({ limit: "100" });
    if (cursor) query.set("cursor", cursor);
    const response = await client.json(`/v1/triggers?${query}`);
    const match = array(response.body.items, "automation roster page")
      .map((item) => record(item, "automation roster item"))
      .find((item) => item.id === automationId);
    if (match) return match;

    const next = record(response.body.page, "automation roster cursor").next_cursor;
    cursor = next === null ? undefined : string(next, "next automation cursor");
  } while (cursor);
  return undefined;
}
