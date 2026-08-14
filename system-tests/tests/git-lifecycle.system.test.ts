import { randomUUID } from "node:crypto";

import { beforeAll, describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { systemTestConfig } from "../src/config.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { array, record, string, type JsonRecord } from "../src/json.js";

const oid = /^[0-9a-f]{40}$/;

async function findRepository(slug: string): Promise<JsonRecord | undefined> {
  let cursor: string | undefined;
  const visited = new Set<string>();
  do {
    const query = new URLSearchParams({ view: "summary", limit: "100" });
    if (cursor) query.set("cursor", cursor);
    const response = await systemClient.json(`/v1/git/repos?${query}`);
    const match = array(response.body.items, "repository list page")
      .map((item) => record(item, "repository list item"))
      .find((item) => string(item.slug, "repository slug").endsWith(`/${slug}`));
    if (match) return match;

    const next = record(response.body.page, "repository list cursor").next_cursor;
    cursor = next === null ? undefined : string(next, "next repository cursor");
    if (cursor && visited.has(cursor)) throw new Error("repository list repeated its cursor");
    if (cursor) visited.add(cursor);
  } while (cursor);
  return undefined;
}

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

  test("lets exactly one teammate own a repository name when they begin together", async () => {
    const sharedSlug = uniqueName("system-simultaneous-create");
    const path = `/v1/git/repos/${encodeURIComponent(sharedSlug)}`;
    const contenders = [systemClient, reviewerClient];
    const attempts = await Promise.all(
      contenders.map((contender) =>
        contender.json("/v1/git/repos", {
          method: "POST",
          body: { slug: sharedSlug },
          expectedStatus: [201, 409],
        }),
      ),
    );

    expect(attempts.map((attempt) => attempt.status).sort()).toEqual([201, 409]);
    const winnerIndex = attempts.findIndex((attempt) => attempt.status === 201);
    const loserIndex = 1 - winnerIndex;
    expect(attempts[winnerIndex]?.body).toMatchObject({
      created: true,
      durable: true,
      applied: { action: "git.repo.create", slug: sharedSlug },
    });
    expect(attempts[loserIndex]?.body).toMatchObject({ error: { code: "conflict" } });

    const winner = await contenders[winnerIndex]!.json(path);
    expect(winner.body).toMatchObject({
      slug: expect.stringContaining(sharedSlug),
      state: "empty",
    });
    const hiddenFromLoser = await contenders[loserIndex]!.json(path, { expectedStatus: 404 });
    expect(hiddenFromLoser.body).toMatchObject({ error: { code: "not_found" } });

    const loserRetry = await contenders[loserIndex]!.json("/v1/git/repos", {
      method: "POST",
      body: { slug: sharedSlug },
      expectedStatus: 409,
    });
    expect(loserRetry.body).toMatchObject({ error: { code: "conflict" } });
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

    expect(await findRepository(slug)).toMatchObject({
      slug: expect.stringContaining(slug),
      state: "empty",
    });
  });

  test("keeps every repository namespace outside another repository's storage", async () => {
    const ownerSlug = uniqueName("system-namespace-owner");
    const owner = new GitProject(ownerSlug, systemClient);
    await owner.create();

    const nestedSlug = `${ownerSlug}.git/tools`;
    const refused = await systemClient.json("/v1/git/repos", {
      method: "POST",
      body: { slug: nestedSlug },
      expectedStatus: 400,
    });
    expect(refused.body).toMatchObject({
      error: {
        code: "bad_request",
        message: expect.stringContaining("namespace segment"),
      },
    });

    const untouchedOwner = await systemClient.json(owner.path);
    expect(untouchedOwner.body).toMatchObject({
      slug: expect.stringContaining(ownerSlug),
      state: "empty",
    });
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

  test("keeps Git's administrative directory outside every repository edit", async () => {
    const guarded = new GitProject(uniqueName("system-no-dot-git-edit"), systemClient);
    await guarded.create();
    const beforeOid = (await guarded.writeFile("main", "README.md", "# Safe project\n"))
      .commitOid;

    const refused = await systemClient.json(`${guarded.path}/blob/main/.git/config`, {
      method: "POST",
      body: {
        base_oid: "",
        contents: "[core]\n\trepositoryformatversion = 0\n",
      },
      expectedStatus: 400,
    });
    expect(refused.body).toMatchObject({
      error: {
        code: "bad_request",
        message: expect.stringContaining("reserved Git administrative component"),
      },
    });

    const after = await systemClient.json(`${guarded.path}/commits/main?limit=1`);
    expect(array(after.body.items, "commit after refused edit")).toEqual([
      expect.objectContaining({ oid: beforeOid }),
    ]);
  });

  test("prevents a stale browser editor from overwriting a newer file version", async () => {
    const opened = await project.readFile("main", "README.md");
    const winningContents = `${readme}\nThe optimistic editor preserved this update.\n`;
    mainCommitOid = (await project.writeFile(
      "main",
      "README.md",
      winningContents,
      { baseOid: opened.baseOid },
    )).commitOid;

    const stale = await systemClient.json(`${project.path}/blob/main/README.md`, {
      method: "POST",
      body: {
        base_oid: opened.baseOid,
        contents: `${readme}\nA stale editor must not win.\n`,
      },
      expectedStatus: 409,
    });
    expect(stale.body).toMatchObject({ error: { code: "conflict" } });

    expect(await project.readFile("main", "README.md")).toEqual({
      contents: winningContents,
      baseOid: expect.stringMatching(oid),
    });
    const commits = await systemClient.json(`${project.path}/commits/main?limit=1`);
    expect(array(commits.body.items, "latest commit")).toEqual([
      expect.objectContaining({ oid: mainCommitOid }),
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

    const openBody = {
      title: "Ship the system-tested lifecycle",
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/feature/system-lifecycle",
      head_oid: featureCommitOid,
      reviewers: [systemTestConfig.reviewerPrincipal],
    };
    const openIdempotencyKey = `open-pr-${randomUUID()}`;
    const opened = await systemClient.json(`${project.path}/prs`, {
      method: "POST",
      body: openBody,
      expectedStatus: 201,
      idempotencyKey: openIdempotencyKey,
    });
    const applied = record(opened.body.applied, "open PR receipt.applied");
    const pullRequest = record(applied.pr, "open PR receipt.applied.pr");
    pullRequestNumber = Number(pullRequest.number);
    expect(pullRequestNumber).toBeGreaterThan(0);
    expect(opened.body).toMatchObject({ durable: true, applied: { action: "git.pr.open" } });

    const replay = await systemClient.json(`${project.path}/prs`, {
      method: "POST",
      body: openBody,
      expectedStatus: 201,
      idempotencyKey: openIdempotencyKey,
    });
    expect(replay.body).toEqual(opened.body);

    const conflictingReplay = await systemClient.json(`${project.path}/prs`, {
      method: "POST",
      body: { ...openBody, title: "A different operation under the same key" },
      expectedStatus: 409,
      idempotencyKey: openIdempotencyKey,
    });
    expect(conflictingReplay.body).toMatchObject({ error: { code: "conflict" } });

    const base = `${project.path}/prs/${pullRequestNumber}`;
    const subject = `myelin://${systemTestConfig.tenant}/git/pr/${slug}:${pullRequestNumber}`;
    const reviewRequest = await eventually(async () => {
      const inbox = await reviewerClient.json("/v1/notif/inbox?view=review-requests&limit=100");
      return array(inbox.body.items, "review request inbox items")
        .map((item) => record(item, "review request inbox item"))
        .find((item) => item.subject === subject);
    }, { description: "the opened pull request to reach its requested reviewer's inbox" });
    expect(reviewRequest).toMatchObject({
      reason: "review_requested",
      class: "direct",
      subsystem: "git",
      subject,
      coalesce_count: 1,
      state: "unread",
    });

    const reviewerOverview = await reviewerClient.json(base);
    expect(reviewerOverview.body).toMatchObject({
      number: pullRequestNumber,
      title: "Ship the system-tested lifecycle",
      head_oid: featureCommitOid,
    });
    const reviewerDiff = await reviewerClient.json(`${base}/diff?view=split&limit=100`);
    expect(array(reviewerDiff.body.files, "reviewer pull request diff files")).toEqual(
      expect.arrayContaining([expect.objectContaining({ path: "src/shipped.ts", kind: "text" })]),
    );

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

  test("limits requested-reviewer access to the assigned pull request", async () => {
    const unrelatedCommitOid = (await project.writeFile(
      "feature/unrelated-review",
      "src/unrelated.ts",
      "export const unrelated = true;\n",
      { startRef: "main" },
    )).commitOid;
    const opened = await systemClient.json(`${project.path}/prs`, {
      method: "POST",
      body: {
        title: "An unrelated pull request",
        base_ref: "refs/heads/main",
        head_ref: "refs/heads/feature/unrelated-review",
        head_oid: unrelatedCommitOid,
        reviewers: [],
      },
      expectedStatus: 201,
    });
    const unrelatedPr = record(
      record(opened.body.applied, "unrelated PR receipt.applied").pr,
      "unrelated PR receipt.applied.pr",
    );
    const unrelatedNumber = Number(unrelatedPr.number);
    expect(unrelatedNumber).toBeGreaterThan(0);

    const unrelatedBase = `${project.path}/prs/${unrelatedNumber}`;
    await reviewerClient.json(unrelatedBase, { expectedStatus: 404 });
    await reviewerClient.json(`${unrelatedBase}/diff?view=split&limit=100`, { expectedStatus: 404 });
  });

  test("keeps discussion and a batched review retry-safe for another principal", async () => {
    const base = `${project.path}/prs/${pullRequestNumber}`;
    const threadPath = `${base}/threads`;
    const threadRetryKey = `discussion-${randomUUID()}`;
    const threadReceipt = await reviewerClient.json(threadPath, {
      method: "POST",
      body: { body_md: "The externally observed contract looks coherent." },
      idempotencyKey: threadRetryKey,
      expectedStatus: 201,
    });
    const thread = record(
      record(threadReceipt.body.applied, "thread receipt.applied").thread,
      "thread receipt.applied.thread",
    );
    const threadId = string(thread.id, "thread id");
    const openingCommentId = string(
      record(array(thread.comments, "opening comments")[0], "opening comment").id,
      "opening comment id",
    );
    expect(threadReceipt.body).toMatchObject({ durable: true, applied: { action: "git.pr.thread.create" } });
    const retriedThread = await reviewerClient.json(threadPath, {
      method: "POST",
      body: { body_md: "The externally observed contract looks coherent." },
      idempotencyKey: threadRetryKey,
      expectedStatus: 201,
    });
    expect(retriedThread.body).toMatchObject({
      applied: { action: "git.pr.thread.create", thread: { id: threadId } },
    });

    const commentPath = `${base}/threads/${encodeURIComponent(threadId)}/comments`;
    const commentRetryKey = `discussion-reply-${randomUUID()}`;
    const commentReceipt = await systemClient.json(commentPath, {
      method: "POST",
      body: { body_md: "Confirmed against the durable edge." },
      idempotencyKey: commentRetryKey,
      expectedStatus: 201,
    });
    expect(commentReceipt.body).toMatchObject({
      durable: true,
      applied: { action: "git.pr.comment.create" },
    });
    const commentId = string(
      record(record(commentReceipt.body.applied, "comment receipt").comment, "comment").id,
      "comment id",
    );
    const retriedComment = await systemClient.json(commentPath, {
      method: "POST",
      body: { body_md: "Confirmed against the durable edge." },
      idempotencyKey: commentRetryKey,
      expectedStatus: 201,
    });
    expect(retriedComment.body).toMatchObject({
      applied: { action: "git.pr.comment.create", comment: { id: commentId } },
    });

    const conversation = await reviewerClient.json(`${base}/threads`);
    const discussion = array(conversation.body.discussion, "discussion threads")
      .map((value) => record(value, "discussion thread"))
      .find((value) => value.id === threadId);
    expect(discussion).toBeDefined();
    expect(array(discussion?.comments, "discussion comments").map((value) => record(value, "comment").id))
      .toEqual([openingCommentId, commentId]);

    const reviewStartRetryKey = `review-start-${randomUUID()}`;
    const reviewStart = await reviewerClient.json(`${base}/reviews/start`, {
      method: "POST",
      body: {},
      idempotencyKey: reviewStartRetryKey,
      expectedStatus: 201,
    });
    const review = record(
      record(reviewStart.body.applied, "review start.applied").review,
      "review start.applied.review",
    );
    const reviewId = string(review.id, "review id");

    const pendingPath = `${base}/reviews/${encodeURIComponent(reviewId)}/comments`;
    const pendingRetryKey = `pending-review-comment-${randomUUID()}`;
    const pending = await reviewerClient.json(pendingPath, {
      method: "POST",
      body: { body_md: "Approving after the lifecycle checks." },
      idempotencyKey: pendingRetryKey,
      expectedStatus: 201,
    });
    expect(pending.body).toMatchObject({
      durable: true,
      applied: { action: "git.pr.review.comment", comment: { pending: true } },
    });
    const pendingComment = record(
      record(pending.body.applied, "pending review comment receipt").comment,
      "pending review comment",
    );
    const pendingCommentId = string(pendingComment.id, "pending review comment id");
    const retriedPending = await reviewerClient.json(pendingPath, {
      method: "POST",
      body: { body_md: "Approving after the lifecycle checks." },
      idempotencyKey: pendingRetryKey,
      expectedStatus: 201,
    });
    expect(retriedPending.body).toMatchObject({
      durable: true,
      applied: {
        action: "git.pr.review.comment",
        comment: { id: pendingCommentId, pending: true },
      },
    });

    const submitPath = `${base}/reviews/${encodeURIComponent(reviewId)}/submit`;
    const submitRetryKey = `review-submit-${randomUUID()}`;
    const submitBody = { verdict: "approved", summary_md: "The full backend path is sound." };
    const submitted = await reviewerClient.json(submitPath, {
      method: "POST",
      body: submitBody,
      idempotencyKey: submitRetryKey,
    });
    expect(submitted.body).toMatchObject({
      durable: true,
      applied: { action: "git.pr.review.submit", result: { emitted: true } },
    });
    const submittedResult = record(
      record(submitted.body.applied, "submitted review receipt").result,
      "submitted review result",
    );
    expect(array(submittedResult.comment_ids, "submitted review comment ids"))
      .toEqual([pendingCommentId]);

    const retriedStart = await reviewerClient.json(`${base}/reviews/start`, {
      method: "POST",
      body: {},
      idempotencyKey: reviewStartRetryKey,
      expectedStatus: 201,
    });
    expect(retriedStart.body).toMatchObject({
      applied: { action: "git.pr.review.start", review: { id: reviewId, verdict: "approved" } },
    });

    const retriedSubmit = await reviewerClient.json(submitPath, {
      method: "POST",
      body: submitBody,
      idempotencyKey: submitRetryKey,
    });
    expect(retriedSubmit.body).toEqual(submitted.body);
    await reviewerClient.json(submitPath, {
      method: "POST",
      body: { ...submitBody, summary_md: "A different decision under the same retry key." },
      idempotencyKey: submitRetryKey,
      expectedStatus: 409,
    });

    const subject = `myelin://${systemTestConfig.tenant}/git/pr/${slug}:${pullRequestNumber}`;
    const completedRequest = await eventually(async () => {
      const inbox = await reviewerClient.json("/v1/notif/inbox?view=review-requests&limit=100");
      const item = array(inbox.body.items, "review request inbox items")
        .map((value) => record(value, "review request inbox item"))
        .find((value) => value.subject === subject);
      return item?.state === "done" ? item : undefined;
    }, { description: "the submitted review to complete its inbox request" });
    expect(completedRequest).toMatchObject({
      reason: "review_requested",
      class: "direct",
      state: "done",
    });

    const threads = await systemClient.json(`${base}/threads`);
    expect(array(threads.body.threads, "pull request threads")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: threadId })]),
    );
    expect(array(threads.body.reviews, "pull request reviews")).toEqual(
      [expect.objectContaining({ id: reviewId, verdict: "approved" })],
    );
    expect((await systemClient.json(`${base}/checks`)).body).toMatchObject({
      current_approvals: 1,
      gate_admitted: true,
    });
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

  test("protects the branch a new repository actually calls home", async () => {
    const trunkProject = new GitProject(uniqueName("system-trunk-default"), systemClient);
    await trunkProject.create();
    const trunkOid = (await trunkProject.writeFile(
      "trunk",
      "README.md",
      "# A repository that begins on trunk\n",
    )).commitOid;

    const refs = await systemClient.json(`${trunkProject.path}/refs?limit=100`);
    expect(array(refs.body.branches, "trunk repository branches")).toEqual([
      expect.objectContaining({ name: "trunk", is_default: true, oid: trunkOid }),
    ]);

    const proposedOid = (await trunkProject.writeFile(
      "feature/ship-from-trunk",
      "src/shipped.ts",
      "export const shippedFromTrunk = true;\n",
      { startRef: "trunk" },
    )).commitOid;
    const opened = await systemClient.json(`${trunkProject.path}/prs`, {
      method: "POST",
      body: {
        title: "Ship safely from the real default branch",
        base_ref: "refs/heads/trunk",
        head_ref: "refs/heads/feature/ship-from-trunk",
        head_oid: proposedOid,
        reviewers: [],
      },
      expectedStatus: 201,
    });
    const pullRequest = record(
      record(opened.body.applied, "trunk PR receipt.applied").pr,
      "trunk PR receipt.applied.pr",
    );
    const base = `${trunkProject.path}/prs/${Number(pullRequest.number)}`;

    const checks = await systemClient.json(`${base}/checks`);
    expect(checks.body).toMatchObject({
      required_approvals: 1,
      current_approvals: 0,
      gate_admitted: false,
    });

    const refusedMerge = await systemClient.json(`${base}/merge`, {
      method: "POST",
      body: {},
      expectedStatus: 409,
    });
    expect(refusedMerge.body).toMatchObject({
      error: { code: "merge_blocked" },
      checks: { required_approvals: 1, current_approvals: 0, gate_admitted: false },
    });
    expect((await systemClient.json(`${trunkProject.path}/refs?limit=100`)).body).toMatchObject({
      default_branch: "trunk",
    });
  });
});
