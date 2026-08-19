// Issue journeys: proposing work and walking it through the authorization
// hold that every issue passes before it becomes ordinary visible work.
import { expect } from "vitest";

import { awaitAuthorizedIssue } from "../issues.js";
import { record, string, type JsonRecord } from "../json.js";
import { systemTestConfig } from "../config.js";
import type { SystemTestClient } from "../client.js";

export interface ProposeIssueOptions {
  projectId?: string;
  typeId?: string;
  prefix?: string;
}

/// Proposes an issue and waits for its authorization to complete, returning
/// the active issue. This is the standard "give me real work to point at"
/// setup step; tests that examine the authorization hold itself should drive
/// the raw endpoints instead.
export async function awaitActiveIssue(
  client: SystemTestClient,
  title: string,
  options: ProposeIssueOptions = {},
): Promise<JsonRecord> {
  const proposed = await client.json("/v1/issues", {
    method: "POST",
    body: {
      project_id: options.projectId ?? systemTestConfig.issues.projectId,
      type_id: options.typeId ?? systemTestConfig.issues.typeId,
      prefix: options.prefix ?? systemTestConfig.issues.prefix,
      title,
    },
    expectedStatus: 202,
  });
  const authorization = record(proposed.body.authorization, "issue authorization");
  const requestEventId = string(authorization.request_event_id, "authorization request id");
  return awaitAuthorizedIssue(client, requestEventId, `issue authorization ${requestEventId}`);
}

/// Asserts the pseudonymous author shape every issue surface must present:
/// an opaque token scoped to the tenant, never a real identity.
export function expectOpaqueIssueAuthor(value: unknown, label: string): string {
  const author = string(value, label);
  const prefix = "issue-author-";
  const suffix = `@${systemTestConfig.tenant}.noreply`;
  expect(author.startsWith(prefix), `${label} prefix`).toBe(true);
  expect(author.endsWith(suffix), `${label} tenant scope`).toBe(true);
  expect(author.slice(prefix.length, -suffix.length), `${label} opaque token`)
    .toMatch(/^[0-9a-f]{32}$/);
  return author;
}
