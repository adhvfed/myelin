import { describe, expect, onTestFinished, test } from "vitest";

import { browserApprovedCliClient, uniqueName } from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import {
  activateExternalAgent,
  askAgent,
  beginAgentRun,
  closeAgentRun,
  findAgentPageItem,
} from "../src/journeys/agents.js";
import { passingPushPipeline, pushPipelineAndAwaitRun } from "../src/journeys/ci-runs.js";
import { awaitActiveIssue } from "../src/journeys/issues.js";
import { createProject } from "../src/journeys/projects.js";
import { record, string, type JsonRecord } from "../src/json.js";

describe("a software team's first useful hour", () => {
  test("moves from one browser approval to code, work, CI, and a governed agent", async () => {
    const founder = await browserApprovedCliClient();

    const product = await createProject(founder, uniqueName("First useful product"));
    const productRef = string(product.project.ref, "new product reference");
    const issue = await awaitActiveIssue(founder, "Ship the first green change", {
      projectId: product.id,
    });
    const issueRef = string(issue.ref, "first issue reference");

    const repository = new GitProject(uniqueName("first-useful-repository"), founder);
    await repository.create();
    const readme = `# ${repository.slug}\n\nWork starts at ${issueRef}.\n`;
    await repository.writeFile("main", "README.md", readme);

    const discoveredRun = await pushPipelineAndAwaitRun(
      founder,
      repository,
      passingPushPipeline(),
    );
    const ciRunId = string(discoveredRun.run_id, "first CI run id");
    await eventually<JsonRecord>(async () => {
      const response = await founder.json(`/v1/ci/runs/${encodeURIComponent(ciRunId)}`);
      const run = record(response.body.run, "first CI run");
      return run.state === "succeeded" ? run : undefined;
    }, {
      description: "the team's first change to pass in the real sandbox",
      timeoutMs: 60_000,
    });

    const activated = await activateExternalAgent(
      founder,
      uniqueName("First-hour collaborator"),
      ["projects.list", "issues.list", "issues.view", "git.list_repositories", "git.read_file", "ci.read_run"],
    );
    expect(activated).not.toHaveProperty("credential");
    const run = await beginAgentRun(founder, activated.agent.id);
    onTestFinished(() => closeAgentRun(run));

    await expect(findAgentPageItem(
      run,
      1,
      "projects.list",
      { limit: 100 },
      (candidate) => candidate.ref === productRef,
      "the agent-visible product",
    )).resolves.toMatchObject({ id: product.id, ref: productRef });

    const visibleIssue = await findAgentPageItem(
      run,
      101,
      "issues.list",
      { state: "open", key: issue.key, limit: 100 },
      (candidate) => candidate.ref === issueRef,
      "the agent-visible first issue",
    );
    expect(visibleIssue).toMatchObject({ ref: issueRef, project_id: product.id });
    await expect(askAgent(run, 201, "issues.view", { issue_ref: issueRef }))
      .resolves.toMatchObject({ ref: issueRef, title: "Ship the first green change" });

    const repositorySlug = `${repository.slug}`;
    await expect(findAgentPageItem(
      run,
      202,
      "git.list_repositories",
      { limit: 100 },
      (candidate) => candidate.slug === repositorySlug ||
        (typeof candidate.slug === "string" && candidate.slug.endsWith(`/${repositorySlug}`)),
      "the agent-visible repository",
    )).resolves.toMatchObject({ slug: expect.stringContaining(repositorySlug) });
    await expect(askAgent(run, 302, "git.read_file", {
      repo: repository.slug,
      ref: "main",
      path: "README.md",
    })).resolves.toMatchObject({ contents: readme });

    await expect(askAgent(run, 303, "ci.read_run", { run_id: ciRunId }))
      .resolves.toMatchObject({ run: { run_id: ciRunId, state: "succeeded" } });
  }, 120_000);
});
