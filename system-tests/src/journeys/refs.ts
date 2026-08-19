// The reference graph journeys: waiting for a link or backlink to appear in
// the projection, and for its removal to propagate. These are eventual-
// consistency waits over the refs read API - the projection is fed by the
// event spine, so "await" here is the honest verb.
import { eventually } from "../eventually.js";
import { array, record, type JsonRecord } from "../json.js";
import type { SystemTestClient } from "../client.js";

export async function awaitBacklink(
  client: SystemTestClient,
  targetRef: string,
  sourceRef: string,
  relationName: string,
): Promise<JsonRecord> {
  return eventually<JsonRecord>(async () => {
    const response = await client.json(
      `/v1/refs/backlinks?ref=${encodeURIComponent(targetRef)}`,
    );
    return array(response.body.items, `backlinks for ${targetRef}`)
      .map((item) => record(item, "backlink"))
      .find((item) => item.root_ref === sourceRef && item.relation === relationName);
  }, { description: `${sourceRef} to become a ${relationName} backlink of ${targetRef}` });
}

export async function awaitBacklinkGone(
  client: SystemTestClient,
  targetRef: string,
  sourceRef: string,
  relationName: string,
): Promise<void> {
  await eventually<boolean>(async () => {
    const response = await client.json(
      `/v1/refs/backlinks?ref=${encodeURIComponent(targetRef)}`,
    );
    const remains = array(response.body.items, `backlinks for ${targetRef}`)
      .map((item) => record(item, "remaining backlink"))
      .some((item) => item.root_ref === sourceRef && item.relation === relationName);
    return remains ? undefined : true;
  }, { description: `${relationName} between ${sourceRef} and ${targetRef} to disappear` });
}

export async function awaitLink(
  client: SystemTestClient,
  sourceRef: string,
  targetRef: string,
  relationName: string,
): Promise<JsonRecord> {
  return eventually<JsonRecord>(async () => {
    const response = await client.json(
      `/v1/refs/links?ref=${encodeURIComponent(sourceRef)}`,
    );
    return array(response.body.items, `links from ${sourceRef}`)
      .map((item) => record(item, "link"))
      .find((item) => item.root_ref === targetRef && item.relation === relationName);
  }, { description: `${sourceRef} to expose its ${relationName} link to ${targetRef}` });
}
