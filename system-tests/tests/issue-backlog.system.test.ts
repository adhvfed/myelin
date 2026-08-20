import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import {
  awaitActiveIssue,
  issuesMatching,
  readIssuePage,
} from "../src/journeys/issues.js";
import { createProject } from "../src/journeys/projects.js";
import { string } from "../src/json.js";

describe("project backlog", () => {
  test("keeps every issue in order as work crosses pages and queues", async () => {
    const { id: projectId, project } = await createProject(
      systemClient,
      uniqueName("Backlog without blind spots"),
    );
    const prefix = string(project.issue_prefix, "project issue prefix");
    const typeId = string(project.default_issue_type_id, "project default issue type");
    const issues = [];
    for (const title of ["Frame the work", "Build the seam", "Ship with confidence"]) {
      issues.push(await awaitActiveIssue(systemClient, uniqueName(title), {
        projectId,
        typeId,
        prefix,
      }));
    }
    const issueIds = issues.map((issue) => string(issue.id, "created backlog issue id"));
    const keyPrefix = `${prefix}-`;

    const first = await readIssuePage(systemClient, {
      state: "all",
      key: keyPrefix,
      limit: 1,
    });
    expect(first.items.map((issue) => issue.id)).toEqual([issueIds[2]]);
    expect(first.limit).toBe(1);
    expect(first.nextCursor).not.toBeNull();

    const second = await readIssuePage(systemClient, {
      state: "all",
      key: keyPrefix,
      limit: 1,
      cursor: first.nextCursor!,
    });
    expect(second.items.map((issue) => issue.id)).toEqual([issueIds[1]]);
    expect(second.nextCursor).not.toBeNull();

    const third = await readIssuePage(systemClient, {
      state: "all",
      key: keyPrefix,
      limit: 1,
      cursor: second.nextCursor!,
    });
    expect(third.items.map((issue) => issue.id)).toEqual([issueIds[0]]);
    expect(third.nextCursor).toBeNull();

    const completeBacklog = await issuesMatching(systemClient, () => true, {
      state: "all",
      key: keyPrefix,
    });
    expect(completeBacklog.map((issue) => issue.id)).toEqual(issueIds.toReversed());
    expect(new Set(completeBacklog.map((issue) => issue.id)).size).toBe(issueIds.length);

    await systemClient.json(
      `/v1/issues?state=closed&key=${encodeURIComponent(keyPrefix)}&limit=1&cursor=${encodeURIComponent(first.nextCursor!)}`,
      { expectedStatus: 400 },
    );
    await systemClient.json(
      `/v1/issues?state=all&key=OTHER-&limit=1&cursor=${encodeURIComponent(first.nextCursor!)}`,
      { expectedStatus: 400 },
    );

    const middleKey = string(issues[1]!.key, "middle backlog issue key");
    await systemClient.json(`/v1/issues/${encodeURIComponent(middleKey)}/close`, {
      method: "POST",
      body: {},
    });
    expect((await issuesMatching(systemClient, () => true, {
      state: "open",
      key: keyPrefix,
    })).map((issue) => issue.id)).toEqual([issueIds[2], issueIds[0]]);
    expect((await issuesMatching(systemClient, () => true, {
      state: "closed",
      key: keyPrefix,
    })).map((issue) => issue.id)).toEqual([issueIds[1]]);
    expect(await issuesMatching(reviewerClient, () => true, {
      state: "all",
      key: keyPrefix,
    })).toEqual([]);
  });
});
