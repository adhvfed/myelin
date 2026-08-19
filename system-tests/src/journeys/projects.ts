// Project journeys: creating a project through the product API and locating
// one in the roster the way a client would - by walking the paged listing.
import { randomUUID } from "node:crypto";

import { findPaged } from "../paging.js";
import { record, string, type JsonRecord } from "../json.js";
import type { SystemTestClient } from "../client.js";

export async function createProject(
  client: SystemTestClient,
  name: string,
  options: { issuePrefix?: string } = {},
): Promise<{ id: string; project: JsonRecord }> {
  const issuePrefix = options.issuePrefix
    ?? `P${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`;
  const created = await client.json("/v1/projects", {
    method: "POST",
    body: { name, issue_prefix: issuePrefix },
    idempotencyKey: `project-${randomUUID()}`,
    expectedStatus: 201,
  });
  const project = record(created.body.project, "created project");
  return { id: string(project.id, "created project id"), project };
}

export async function findProject(
  client: SystemTestClient,
  projectId: string,
): Promise<JsonRecord | undefined> {
  return findPaged(client, "/v1/projects", (item) => item.id === projectId);
}
