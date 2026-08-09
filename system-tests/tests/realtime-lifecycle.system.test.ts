import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import { systemClient, uniqueName } from "../src/context.js";
import type { ServerSentEvent } from "../src/event-stream.js";
import { git } from "../src/git-cli.js";
import { GitProject } from "../src/git-project.js";
import { record } from "../src/json.js";

function isRepositoryEvent(event: ServerSentEvent, type: string, slug: string): boolean {
  if (event.event !== type) return false;
  try {
    return record(JSON.parse(event.data), `${type} event data`).slug === slug;
  } catch {
    return false;
  }
}

describe("real-time engineering lifecycle", () => {
  test("streams repository creation and push events to an authenticated tenant client", async () => {
    const unauthenticated = await systemClient.json(
      `/v1/t/${encodeURIComponent(systemTestConfig.tenant)}/events`,
      { authenticated: false, expectedStatus: 401 },
    );
    expect(unauthenticated.body).toHaveProperty("error.code", "unauthorized");

    const connection = await systemClient.eventStream(
      `/v1/t/${encodeURIComponent(systemTestConfig.tenant)}/events`,
    );
    expect(connection.headers.get("content-type")).toContain("text/event-stream");
    expect(connection.headers.get("cache-control")).toContain("no-cache");

    const slug = uniqueName("system-realtime");
    const project = new GitProject(slug, systemClient);
    const working = await mkdtemp(join(tmpdir(), "myelin-system-realtime-"));
    try {
      const createdEvent = connection.stream.waitFor(
        (event) => isRepositoryEvent(event, "repo.created", slug),
        { description: `repo.created for ${slug}` },
      );
      await project.create();
      expect(JSON.parse((await createdEvent).data)).toMatchObject({
        type: "repo.created",
        slug,
      });

      const pushedEvent = connection.stream.waitFor(
        (event) => isRepositoryEvent(event, "repo.pushed", slug),
        { description: `repo.pushed for ${slug}` },
      );
      const repositoryUrl = new URL(
        `/${encodeURIComponent(systemTestConfig.tenant)}/${encodeURIComponent(systemTestConfig.region)}/${encodeURIComponent(slug)}.git`,
        systemTestConfig.edgeUrl,
      ).toString();
      const pseudonym = `${systemTestConfig.principal}@${systemTestConfig.tenant}.noreply`;
      await git(["init", "--initial-branch=main", working]);
      await git(["config", "user.name", pseudonym], { cwd: working });
      await git(["config", "user.email", pseudonym], { cwd: working });
      await writeFile(join(working, "README.md"), `# ${slug}\n`, "utf8");
      await git(["add", "README.md"], { cwd: working });
      await git(["commit", "-m", "feat: initialize the realtime test repository"], {
        cwd: working,
      });
      await git(["push", repositoryUrl, "HEAD:refs/heads/main"], {
        cwd: working,
        token: systemTestConfig.token,
      });
      expect(JSON.parse((await pushedEvent).data)).toMatchObject({
        type: "repo.pushed",
        slug,
      });
    } finally {
      connection.stream.close();
      await rm(working, { recursive: true, force: true });
    }
  });
});
