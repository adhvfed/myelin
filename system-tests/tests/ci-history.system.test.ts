import { describe, expect, test } from "vitest";

import { systemClient, reviewerClient, uniqueName } from "../src/context.js";
import {
  passingPushPipeline,
  pushPipelineAndAwaitRun,
  readCiRunPage,
} from "../src/journeys/ci-runs.js";
import { GitProject } from "../src/git-project.js";
import { string } from "../src/json.js";
import { walkPaged } from "../src/paging.js";

describe("CI history", () => {
  test("keeps every build in order while delivery spans repositories and pages", async () => {
    const projects = [
      new GitProject(uniqueName("history-api"), systemClient),
      new GitProject(uniqueName("history-web"), systemClient),
    ];
    const expectedRunIds: string[] = [];

    for (const project of projects) {
      await project.create();
      await project.writeFile("main", "README.md", `# ${project.slug}\n`);
      const run = await pushPipelineAndAwaitRun(
        systemClient,
        project,
        passingPushPipeline(),
      );
      expectedRunIds.push(string(run.run_id, `${project.slug} CI run id`));
      expect(run).toMatchObject({
        repo_ref: expect.stringContaining(`/git/repo/${project.slug}`),
        trigger_kind: "push",
        source_ref: "refs/heads/main",
      });
    }

    const first = await readCiRunPage(systemClient, { state: "all", limit: 1 });
    expect(first.items).toHaveLength(1);
    expect(first.limit).toBe(1);
    expect(string(first.items[0]!.run_id, "newest CI run id")).toBe(expectedRunIds[1]);
    expect(first.nextCursor).not.toBeNull();

    const second = await readCiRunPage(systemClient, {
      cursor: first.nextCursor!,
      limit: 1,
    });
    expect(second.items).toHaveLength(1);
    expect(string(second.items[0]!.run_id, "next CI run id")).toBe(expectedRunIds[0]);

    const observed = new Map<string, number>();
    for await (const run of walkPaged(systemClient, "/v1/ci/runs?state=all")) {
      const runId = string(run.run_id, "CI history run id");
      observed.set(runId, (observed.get(runId) ?? 0) + 1);
    }
    expect([...observed.values()].every((count) => count === 1)).toBe(true);
    expect(expectedRunIds.filter((runId) => observed.get(runId) !== 1)).toEqual([]);

    const reviewerPage = await readCiRunPage(reviewerClient, { state: "all", limit: 100 });
    expect(
      reviewerPage.items.some((run) => expectedRunIds.includes(
        string(run.run_id, "reviewer CI run id"),
      )),
    ).toBe(false);

    await systemClient.json(
      `/v1/ci/runs?state=failed&cursor=${encodeURIComponent(first.nextCursor!)}`,
      { expectedStatus: 409 },
    );
    await reviewerClient.json(
      `/v1/ci/runs?cursor=${encodeURIComponent(first.nextCursor!)}`,
      { expectedStatus: 409 },
    );
  });
});
