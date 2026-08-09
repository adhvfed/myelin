import { randomUUID } from "node:crypto";

import { beforeAll, describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { systemTestConfig } from "../src/config.js";
import { GitProject } from "../src/git-project.js";
import { array, record, string } from "../src/json.js";

const oid = /^[0-9a-f]{40}$/;

describe.sequential("Git engineering lifecycle", () => {
  const slug = uniqueName("system-git");
  const project = new GitProject(slug, systemClient);
  const readme = `# ${slug}\n\nA repository exercised from outside the running backend.\n`;
  let mainCommitOid = "";
  let featureCommitOid = "";
  let pullRequestNumber = 0;

  beforeAll(async () => {
    const created = await project.create();
    expect(created).toMatchObject({
      durable: true,
      created: true,
      applied: { action: "git.repo.create", slug },
    });
  });

  test("creates a durable repository with retry-safe duplicate handling", async () => {
    const replayKey = `system-create-${randomUUID()}`;
    const firstSlug = uniqueName("system-replay");
    const replayProject = new GitProject(firstSlug, systemClient);
    const first = await replayProject.create(replayKey);
    const replay = await replayProject.create(replayKey);
    expect(first).toMatchObject({ created: true, durable: true });
    expect(replay).toMatchObject({
      created: false,
      durable: true,
      applied: { action: "git.repo.create", slug: firstSlug },
    });

    const duplicate = await replayProject.create();
    expect(duplicate).toMatchObject({
      durable: true,
      created: false,
      applied: { action: "git.repo.create", slug: firstSlug },
    });

    const home = await systemClient.json(project.path);
    expect(home.body).toMatchObject({ state: "empty", slug: expect.stringContaining(slug) });

    const repositories = await systemClient.json("/v1/git/repos?limit=100");
    expect(array(repositories.body.items, "repository list items")).toEqual(
      expect.arrayContaining([expect.objectContaining({ slug: expect.stringContaining(slug) })]),
    );
  });

  test("commits and reads a snapshot through every primary browse projection", async () => {
    mainCommitOid = (await project.writeFile("main", "README.md", readme)).commitOid;
    expect(mainCommitOid).toMatch(oid);
    await project.writeFile("main", "src/index.ts", "export const lifecycle = 'ready';\n");

    const home = await systemClient.json(project.path);
    expect(home.body).toMatchObject({ state: "populated", readme });
    expect(home.body.snapshot_oid).toMatch(oid);

    const refs = await systemClient.json(`${project.path}/refs?limit=100`);
    expect(array(refs.body.branches, "repository branches")).toEqual(
      expect.arrayContaining([expect.objectContaining({ name: "main", is_default: true })]),
    );

    const tree = await systemClient.json(`${project.path}/tree/main/`);
    expect(tree.body).toMatchObject({ path: "", ref: "main" });
    expect(array(tree.body.entries, "root tree entries")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ name: "README.md", is_dir: false }),
        expect.objectContaining({ name: "src", is_dir: true }),
      ]),
    );

    const nestedTree = await systemClient.json(`${project.path}/tree/main/src`);
    expect(nestedTree.body).toMatchObject({ path: "src", ref: "main" });
    expect(array(nestedTree.body.entries, "nested tree entries")).toEqual(
      expect.arrayContaining([expect.objectContaining({ name: "index.ts", is_dir: false })]),
    );

    const blob = await systemClient.json(`${project.path}/blob/main/README.md`);
    expect(blob.body).toMatchObject({
      path: "README.md",
      contents: readme,
      viewer_may_edit: true,
    });
    expect(blob.body.base_oid).toMatch(oid);

    const blame = await systemClient.json(`${project.path}/blame/main/README.md`);
    expect(blame.body).toMatchObject({ path: "README.md", ref: "main", contents: readme });
    expect(blame.body.snapshot_oid).toMatch(oid);
    expect(array(blame.body.hunks, "blame hunks").length).toBeGreaterThan(0);

    const commits = await systemClient.json(`${project.path}/commits/main?limit=20`);
    expect(array(commits.body.items, "commit log items").length).toBeGreaterThanOrEqual(2);

    const search = await systemClient.json(
      `/v1/git/search/code?repo=${encodeURIComponent(slug)}&q=${encodeURIComponent("lifecycle")}`,
    );
    expect(search.body).toMatchObject({ complete: true });
    expect(array(search.body.items, "code search items")).toEqual([
      expect.objectContaining({ repo: slug, path: "src/index.ts", line: 1 }),
    ]);
  });

  test("opens and inspects a pull request against immutable snapshots", async () => {
    featureCommitOid = (await project.writeFile(
      "feature/system-lifecycle",
      "src/shipped.ts",
      "export const shipped = true;\n",
      { startRef: "main" },
    )).commitOid;
    expect(featureCommitOid).toMatch(oid);

    const opened = await systemClient.json(`${project.path}/prs`, {
      method: "POST",
      body: {
        title: "Ship the system-tested lifecycle",
        base_ref: "refs/heads/main",
        head_ref: "refs/heads/feature/system-lifecycle",
        head_oid: featureCommitOid,
        reviewers: [systemTestConfig.reviewerPrincipal],
      },
      expectedStatus: 201,
    });
    const applied = record(opened.body.applied, "open PR receipt.applied");
    const pullRequest = record(applied.pr, "open PR receipt.applied.pr");
    pullRequestNumber = Number(pullRequest.number);
    expect(pullRequestNumber).toBeGreaterThan(0);
    expect(opened.body).toMatchObject({ durable: true, applied: { action: "git.pr.open" } });

    const base = `${project.path}/prs/${pullRequestNumber}`;
    const overview = await systemClient.json(base);
    expect(overview.body).toMatchObject({
      number: pullRequestNumber,
      title: "Ship the system-tested lifecycle",
      pr_state: "open",
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/feature/system-lifecycle",
      head_oid: featureCommitOid,
      durable: true,
    });

    const list = await systemClient.json(`${project.path}/prs?state=open&sort=updated`);
    expect(array(list.body.items, "pull request list items")).toEqual(
      expect.arrayContaining([expect.objectContaining({ number: pullRequestNumber })]),
    );

    const commits = await systemClient.json(`${base}/commits?limit=20`);
    expect(array(commits.body.items, "pull request commits").length).toBeGreaterThan(0);

    const diff = await systemClient.json(`${base}/diff?view=split&limit=100`);
    expect(diff.body).toMatchObject({
      number: pullRequestNumber,
      head_oid: featureCommitOid,
      three_dot: true,
    });
    expect(array(diff.body.files, "pull request diff files")).toEqual(
      expect.arrayContaining([expect.objectContaining({ path: "src/shipped.ts", kind: "text" })]),
    );

    const checks = await systemClient.json(`${base}/checks`);
    expect(checks.body).toHaveProperty("gate_admitted");
  });

  test("persists discussion threads and a batched review from another principal", async () => {
    const base = `${project.path}/prs/${pullRequestNumber}`;
    const threadReceipt = await reviewerClient.json(`${base}/threads`, {
      method: "POST",
      body: { body_md: "The externally observed contract looks coherent." },
      expectedStatus: 201,
    });
    const thread = record(
      record(threadReceipt.body.applied, "thread receipt.applied").thread,
      "thread receipt.applied.thread",
    );
    const threadId = string(thread.id, "thread id");
    expect(threadReceipt.body).toMatchObject({ durable: true, applied: { action: "git.pr.thread.create" } });

    const commentReceipt = await systemClient.json(`${base}/threads/${encodeURIComponent(threadId)}/comments`, {
      method: "POST",
      body: { body_md: "Confirmed against the durable edge." },
      expectedStatus: 201,
    });
    expect(commentReceipt.body).toMatchObject({
      durable: true,
      applied: { action: "git.pr.comment.create" },
    });

    const reviewStart = await reviewerClient.json(`${base}/reviews/start`, {
      method: "POST",
      body: {},
      expectedStatus: 201,
    });
    const review = record(
      record(reviewStart.body.applied, "review start.applied").review,
      "review start.applied.review",
    );
    const reviewId = string(review.id, "review id");

    const pending = await reviewerClient.json(`${base}/reviews/${encodeURIComponent(reviewId)}/comments`, {
      method: "POST",
      body: { body_md: "Approving after the lifecycle checks." },
      expectedStatus: 201,
    });
    expect(pending.body).toMatchObject({
      durable: true,
      applied: { action: "git.pr.review.comment", comment: { pending: true } },
    });

    const submitted = await reviewerClient.json(`${base}/reviews/${encodeURIComponent(reviewId)}/submit`, {
      method: "POST",
      body: { verdict: "approved", summary_md: "The full backend path is sound." },
    });
    expect(submitted.body).toMatchObject({
      durable: true,
      applied: { action: "git.pr.review.submit", result: { emitted: true } },
    });

    const threads = await systemClient.json(`${base}/threads`);
    expect(array(threads.body.threads, "pull request threads")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: threadId })]),
    );
    expect(array(threads.body.reviews, "pull request reviews")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: reviewId, verdict: "approved" }),
      ]),
    );
  });

  test("merges the approved pull request and exposes the result on the base ref", async () => {
    const base = `${project.path}/prs/${pullRequestNumber}`;
    const merged = await systemClient.json(`${base}/merge`, {
      method: "POST",
      body: {},
      expectedStatus: 200,
    });
    expect(merged.body).toMatchObject({
      durable: true,
      applied: {
        action: "git.pr.merge",
        merged: true,
        base_ref: "refs/heads/main",
        new_oid: featureCommitOid,
      },
    });

    const overview = await systemClient.json(base);
    expect(overview.body).toMatchObject({ pr_state: "merged", head_oid: featureCommitOid });

    const mergedBlob = await systemClient.json(`${project.path}/blob/main/src/shipped.ts`);
    expect(mergedBlob.body).toMatchObject({ contents: "export const shipped = true;\n" });
  });
});
