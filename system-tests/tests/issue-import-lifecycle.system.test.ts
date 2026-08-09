import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { array, record, string, type JsonRecord } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

describe("issue migration lifecycle", () => {
  test("reconciles first, creates once, and resumes without a duplicate", async () => {
    const jobId = randomUUID();
    const prefix = `I${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`;
    const records = [
      {
        source_id: uniqueName("github-acme-platform-41"),
        project_id: systemTestConfig.issues.projectId,
        type_id: systemTestConfig.issues.typeId,
        prefix,
        title: uniqueName("Preserve the first imported issue"),
      },
      {
        source_id: uniqueName("github-acme-platform-42"),
        project_id: systemTestConfig.issues.projectId,
        type_id: systemTestConfig.issues.typeId,
        prefix,
        title: uniqueName("Preserve the second imported issue"),
      },
    ];
    const body = { source: "github", records };

    const preview = await systemClient.json(`/v1/issues/imports/${jobId}/dry-run`, {
      method: "POST",
      body,
      idempotencyKey: false,
    });
    expect(preview.body).toEqual({
      import: { job_id: jobId, source: "github", mode: "dry_run" },
      reconciliation: { received: 2, ready: 2, lossy: 0, dropped: 0 },
      losses: [],
    });

    const stillEmpty = await systemClient.json(
      `/v1/issues?state=all&key=${encodeURIComponent(`${prefix}-`)}&limit=100`,
    );
    expect(array(stillEmpty.body.items, "issues after dry-run")).toEqual([]);

    const firstRun = await systemClient.json(`/v1/issues/imports/${jobId}/run`, {
      method: "POST",
      body,
      expectedStatus: 202,
    });
    expect(firstRun.body).toMatchObject({
      import: {
        job_id: jobId,
        source: "github",
        mode: "run",
        resumable: true,
      },
      summary: { received: 2, created: 2, resumed: 0, lossy: 0, dropped: 0 },
      losses: [],
    });
    const firstIssues = array(firstRun.body.issues, "first import issues").map((value) => {
      const outcome = record(value, "first import outcome");
      expect(outcome.created).toBe(true);
      expect(outcome.authorization).toMatchObject({ status: "requested" });
      return {
        sourceId: string(outcome.source_id, "import source id"),
        issue: record(outcome.issue, "imported issue"),
        requestEventId: string(
          record(outcome.authorization, "import authorization").request_event_id,
          "import authorization request id",
        ),
      };
    });
    for (const { issue } of firstIssues) {
      expect(string(issue.ref, "imported issue ref")).toBe(
        `myelin://${systemTestConfig.tenant}/issue/issue/${string(issue.key, "imported issue key")}`,
      );
    }

    const resumed = await systemClient.json(`/v1/issues/imports/${jobId}/run`, {
      method: "POST",
      body,
      expectedStatus: 202,
    });
    expect(resumed.body).toMatchObject({
      summary: { received: 2, created: 0, resumed: 2, lossy: 0, dropped: 0 },
      losses: [],
    });
    const resumedIssues = array(resumed.body.issues, "resumed import issues").map((value) => {
      const outcome = record(value, "resumed import outcome");
      expect(outcome.created).toBe(false);
      return {
        sourceId: string(outcome.source_id, "resumed source id"),
        issue: record(outcome.issue, "resumed issue"),
      };
    });
    expect(resumedIssues).toEqual(
      firstIssues.map(({ sourceId, issue }) => ({ sourceId, issue })),
    );

    const activeIssues = await Promise.all(
      firstIssues.map(({ requestEventId }) =>
        eventually<JsonRecord>(
          async () => {
            const response = await systemClient.json(
              `/v1/issues/authorization-requests/${encodeURIComponent(requestEventId)}`,
              { expectedStatus: [200, 202] },
            );
            return response.status === 200
              ? record(response.body.issue, "active imported issue")
              : undefined;
          },
          { description: `authorization for imported issue ${requestEventId}` },
        ),
      ),
    );
    expect(activeIssues.map((issue) => issue.id)).toEqual(
      firstIssues.map(({ issue }) => issue.id),
    );
    expect(activeIssues.map((issue) => issue.ref)).toEqual(
      firstIssues.map(({ issue }) => issue.ref),
    );

    const visibleToATeammate = await reviewerClient.json(
      `/v1/issues?state=all&key=${encodeURIComponent(`${prefix}-`)}&limit=100`,
    );
    expect(array(visibleToATeammate.body.items, "imported issues visible to a teammate")).toEqual(
      expect.arrayContaining(
        records.map((source) => expect.objectContaining({ title: source.title })),
      ),
    );
    expect(array(visibleToATeammate.body.items)).toHaveLength(2);
    expect(
      array(visibleToATeammate.body.items, "re-addressable imported issues").map(
        (issue) => record(issue, "re-addressable imported issue").ref,
      ),
    ).toEqual(expect.arrayContaining(firstIssues.map(({ issue }) => issue.ref)));
  });
});
