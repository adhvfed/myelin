import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { systemClient, uniqueName } from "../src/context.js";
import { GitProject } from "../src/git-project.js";
import { awaitActiveIssue } from "../src/journeys/issues.js";
import { awaitBacklink, awaitLink } from "../src/journeys/refs.js";
import { integer, record, string } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

describe("pull request lifecycle", () => {
  test("gathers a pull request's delivery promise and living context without integration keys", async () => {
    const issue = await awaitActiveIssue(systemClient, uniqueName("Deliver the promised change"));
    const issueKey = string(issue.key, "promised delivery issue key");
    const issueRef = string(issue.ref, "promised delivery issue ref");
    const slug = uniqueName("promised-delivery");
    const project = new GitProject(slug, systemClient);
    await project.create();
    await project.writeFile("main", "README.md", `# ${slug}\n`);
    const headOid = (await project.writeFile(
      "feature/promised-delivery",
      "delivery.txt",
      "This change belongs to its delivery issue.\n",
      { startRef: "main" },
    )).commitOid;

    const opened = await systemClient.json(`${project.path}/prs`, {
      method: "POST",
      body: {
        title: "Carry the promised delivery",
        body_md: `The implementation and its work item stay navigable.\n\nCloses ${issueKey}\n`,
        base_ref: "refs/heads/main",
        head_ref: "refs/heads/feature/promised-delivery",
        head_oid: headOid,
        reviewers: [],
      },
      expectedStatus: 201,
    });
    const pullRequest = record(
      record(opened.body.applied, "promised delivery PR receipt").pr,
      "promised delivery pull request",
    );
    const pullRequestNumber = integer(pullRequest.number, "promised delivery PR number");
    const pullRequestRef = string(pullRequest.ref, "promised delivery pull request ref");
    expect(pullRequestRef).toBe(
      `myelin://${systemTestConfig.tenant}/git/pr/${slug}:${pullRequestNumber}`,
    );

    expect(await awaitBacklink(systemClient, issueRef, pullRequestRef, "closes")).toMatchObject({
      ref: pullRequestRef,
      root_ref: pullRequestRef,
      target_ref: issueRef,
      relation: "closes",
      relation_class: "lifecycle",
    });
    expect(await awaitLink(systemClient, pullRequestRef, issueRef, "closes")).toMatchObject({
      ref: issueRef,
      root_ref: issueRef,
      source_ref: pullRequestRef,
      target_ref: issueRef,
      relation: "closes",
      relation_class: "lifecycle",
    });

    const contextTitle = uniqueName("Promised delivery notes");
    const createdContext = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: { title: contextTitle, template: "blank", visibility: "team" },
      expectedStatus: 201,
    });
    const contextPage = record(createdContext.body.page, "pull request context page");
    const contextPageId = string(contextPage.id, "pull request context page id");
    const contextPageRef = string(contextPage.ref, "pull request context page ref");
    const contextVersion = integer(contextPage.version, "pull request context page version");
    await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(contextPageId)}`, {
      method: "PUT",
      body: {
        expected_version: contextVersion,
        title: contextTitle,
        visibility: "team",
        blocks: [{
          type: "paragraph",
          markdown: "Keep the reasoning beside the pull request \uFFFC.",
          references: [pullRequestRef],
        }],
      },
    });

    const contextLink = await awaitBacklink(systemClient, pullRequestRef, contextPageRef, "links");
    expect(contextLink).toMatchObject({
      root_ref: contextPageRef,
      target_ref: pullRequestRef,
      relation: "links",
      relation_class: "reference",
    });
    const contextPassageRef = string(contextLink.ref, "linked context passage ref");
    expect(contextPassageRef.startsWith(`${contextPageRef}#b`)).toBe(true);
    expect(contextPassageRef.slice(`${contextPageRef}#b`.length)).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);
    expect(contextLink.source_ref).toBe(contextPassageRef);
  });

  test("lets one retry key open pull requests in two repositories", async () => {
    const firstProject = new GitProject(uniqueName("retry-scope-first"), systemClient);
    const secondProject = new GitProject(uniqueName("retry-scope-second"), systemClient);
    const retryKey = `open-pr-${randomUUID()}`;

    for (const project of [firstProject, secondProject]) {
      await project.create();
      await project.writeFile("main", "README.md", `# ${project.slug}\n`);
      const headOid = (await project.writeFile(
        "feature/retry-scope",
        "change.txt",
        `A change for ${project.slug}.\n`,
        { startRef: "main" },
      )).commitOid;

      const opened = await systemClient.json(`${project.path}/prs`, {
        method: "POST",
        body: {
          title: `Change ${project.slug}`,
          base_ref: "refs/heads/main",
          head_ref: "refs/heads/feature/retry-scope",
          head_oid: headOid,
          reviewers: [],
        },
        idempotencyKey: retryKey,
        expectedStatus: 201,
      });
      expect(opened.body).toMatchObject({
        durable: true,
        applied: { action: "git.pr.open", pr: { number: 1 } },
      });
    }
  });

});
