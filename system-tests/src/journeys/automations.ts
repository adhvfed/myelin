// Automation journeys: observe one durable firing without coupling a story to
// the trigger-history envelope or open-ended polling.
import type { SystemTestClient } from "../client.js";
import { eventually } from "../eventually.js";
import { array, record, type JsonRecord } from "../json.js";

export type AutomationFiringState =
  | "queued"
  | "awaiting_approval"
  | "claimed"
  | "started"
  | "terminal";

export interface AwaitAutomationFiringOptions {
  state: AutomationFiringState;
  resultState?: "available" | "erased";
  description: string;
}

export async function awaitAutomationFiring(
  client: SystemTestClient,
  automationId: string,
  eventId: string,
  options: AwaitAutomationFiringOptions,
): Promise<JsonRecord> {
  return eventually<JsonRecord>(async () => {
    const response = await client.json(
      `/v1/triggers/${encodeURIComponent(automationId)}/firings?limit=100`,
    );
    return array(response.body.items, "automation firing history")
      .map((item) => record(item, "automation firing"))
      .find((item) => (
        item.event_id === eventId &&
        item.state === options.state &&
        (options.resultState === undefined || item.result_state === options.resultState)
      ));
  }, { description: options.description });
}
