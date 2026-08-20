import { describe, expect, test } from "vitest";

import { systemClient, uniqueName } from "../src/context.js";
import { GitProject } from "../src/git-project.js";
import { proposeChange, readPullRequestPage } from "../src/journeys/pull-requests.js";
import { integer, string, type JsonRecord } from "../src/json.js";
import { walkPaged } from "../src/paging.js";

function coordinate(item: JsonRecord): string {
  return `${string(item.repo, "pull request repository")}#${integer(
    item.number,
    "pull request number",
  )}`;
}

describe("pull request dashboard", () => {
  test("keeps a developer's work complete while it spans repositories and pages", async () => {
    const repositories = [
      new GitProject(uniqueName("dashboard-api"), systemClient),
      new GitProject(uniqueName("dashboard-web"), systemClient),
    ];
    const expected = new Set<string>();

    for (const repository of repositories) {
      await repository.create();
      await repository.writeFile("main", "README.md", `# ${repository.slug}\n`);
      for (const change of ["first", "second"] as const) {
        const opened = await proposeChange(systemClient, repository, {
          branch: `feature/${change}-${repository.slug}`,
          path: `${change}.txt`,
          contents: `${change} change in ${repository.slug}\n`,
          title: `${change} change in ${repository.slug}`,
        });
        expected.add(`${opened.repository}#${opened.number}`);
        expect(opened.receipt).toMatchObject({
          durable: true,
          applied: { action: "git.pr.open" },
        });
      }
    }

    const first = await readPullRequestPage(systemClient, "/v1/git/prs", {
      bucket: "yours",
      sort: "created",
      limit: 1,
    });
    expect(first.items).toHaveLength(1);
    expect(first.limit).toBe(1);
    expect(integer(first.counts.bucket, "my-work count")).toBeGreaterThanOrEqual(expected.size);
    expect(first.total).toBe(integer(first.counts.bucket, "my-work count"));
    expect(first.nextCursor).not.toBeNull();
    expect(first.previousCursor).toBeNull();

    const second = await readPullRequestPage(systemClient, "/v1/git/prs", {
      cursor: first.nextCursor!,
    });
    expect(second.items).toHaveLength(1);
    expect(coordinate(second.items[0]!)).not.toBe(coordinate(first.items[0]!));
    expect(second.previousCursor).not.toBeNull();

    const backAtFirst = await readPullRequestPage(systemClient, "/v1/git/prs", {
      cursor: second.previousCursor!,
    });
    expect(backAtFirst.items.map(coordinate)).toEqual(first.items.map(coordinate));

    const observed = new Map<string, number>();
    for await (const item of walkPaged(
      systemClient,
      "/v1/git/prs?bucket=yours&sort=created",
      { limit: 25 },
    )) {
      const key = coordinate(item);
      observed.set(key, (observed.get(key) ?? 0) + 1);
    }
    expect([...observed.values()].every((count) => count === 1)).toBe(true);
    expect([...expected].filter((key) => !observed.has(key))).toEqual([]);
    expect([...expected].filter((key) => observed.get(key) !== 1)).toEqual([]);

    const repository = repositories[0]!;
    const repositoryFirst = await readPullRequestPage(systemClient, `${repository.path}/prs`, {
      state: "all",
      sort: "created",
      limit: 1,
    });
    expect(repositoryFirst.counts).toMatchObject({
      open: 2,
      merged: 0,
      closed: 0,
      all: 2,
      yours: 2,
      needs_review: 0,
    });
    expect(repositoryFirst.total).toBe(2);

    const repositoryCoordinates: string[] = [];
    for await (const item of walkPaged(
      systemClient,
      `${repository.path}/prs?state=all&sort=created`,
      { limit: 1 },
    )) {
      repositoryCoordinates.push(`${repository.slug}#${integer(item.number, "repository PR number")}`);
    }
    expect(new Set(repositoryCoordinates)).toEqual(
      new Set([...expected].filter((key) => key.startsWith(`${repository.slug}#`))),
    );
    expect(repositoryCoordinates).toHaveLength(2);

    await systemClient.json(
      `${repository.path}/prs?cursor=${encodeURIComponent(first.nextCursor!)}`,
      { expectedStatus: 400 },
    );
    await systemClient.json(
      `/v1/git/prs?bucket=needs-review&cursor=${encodeURIComponent(first.nextCursor!)}`,
      { expectedStatus: 400 },
    );
  });
});
