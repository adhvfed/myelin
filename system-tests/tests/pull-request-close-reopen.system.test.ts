import { randomUUID } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { gitRepositoryUrl, systemTestConfig } from "../src/config.js";
import {
  browserApprovedCliCredential,
  reviewerClient,
  systemClient,
  uniqueName,
} from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { git } from "../src/git-cli.js";
import { GitProject } from "../src/git-project.js";
import { array, integer, record } from "../src/json.js";
import { runCliWith, type CliResult } from "../src/myelin-cli.js";

describe("a pull request's life: draft, ready, closed, reopened", () => {
  let scratch: string;

  beforeAll(async () => {
    scratch = await mkdtemp(join(tmpdir(), "myelin-pull-request-close-reopen-"));
  });

  afterAll(async () => {
    await rm(scratch, { recursive: true, force: true });
  });

  test("its author drafts, readies, closes and reopens it, and its reviewer is asked only while it is live", async () => {
    const session = await browserApprovedCliCredential();
    const run = (...args: string[]): Promise<CliResult> => runCliWith(
      join(scratch, "config"),
      {
        environment: {
          MYELIN_EDGE: systemTestConfig.edgeUrl,
          MYELIN_TOKEN: session.token,
          MYELIN_TOKEN_SCHEME: session.tokenScheme,
        },
      },
      args,
    );
    const myelin = async (...args: string[]): Promise<string> => {
      const result = await run(...args);
      expect(result.exitCode, result.stderr).toBe(0);
      return result.stdout;
    };
    const keyed = (...args: string[]) => myelin("--idempotency-key", randomUUID(), ...args);

    const slug = uniqueName("pull-request-close-reopen");
    const project = new GitProject(slug, systemClient);
    await project.create();
    await project.writeFile("main", "README.md", "# Greetings\n");
    const branch = "draft/greeting";
    const first = (await project.writeFile(branch, "greeting.txt", "hello\n", {
      startRef: "main",
    })).commitOid;
    const opened = await systemClient.json(`${project.path}/prs`, {
      method: "POST",
      body: {
        title: "Add a greeting",
        base_ref: "refs/heads/main",
        head_ref: `refs/heads/${branch}`,
        head_oid: first,
        draft: true,
        reviewers: [systemTestConfig.reviewerPrincipal],
      },
      expectedStatus: 201,
    });
    const number = integer(
      record(record(opened.body.applied, "opened").pr, "pull request").number,
      "pull request number",
    );
    const n = String(number);
    const view = () => myelin("repo", "pr", "view", slug, n);
    const subject = `myelin://${systemTestConfig.tenant}/git/pr/${slug}:${number}`;
    const reviewRequest = (state: string) => eventually(async () => {
      const inbox = await reviewerClient.json("/v1/notif/inbox?view=review-requests&limit=100");
      const item = array(inbox.body.items, "review request inbox items")
        .map((value) => record(value, "review request inbox item"))
        .find((value) => value.subject === subject);
      return item?.state === state ? item : undefined;
    }, { timeoutMs: 30_000, description: `the reviewer's request to be ${state}` });

    expect(await view()).toContain(`#${number} Add a greeting [draft]\n`);
    const draftMerge = await systemClient.json(`${project.path}/prs/${number}/merge`, {
      method: "POST",
      body: {},
      expectedStatus: 409,
    });
    expect(JSON.stringify(draftMerge.body)).toContain("mark it ready for review first");

    expect(await keyed("repo", "pr", "ready", slug, n)).toBe(
      `#${number} Add a greeting is ready for review\n`,
    );
    expect(await view()).toContain(`#${number} Add a greeting [open]\n`);
    await reviewRequest("unread");

    expect(await keyed("repo", "pr", "close", slug, n)).toBe(`closed #${number} Add a greeting\n`);
    expect(await view()).toContain(`#${number} Add a greeting [closed]\n`);
    await reviewRequest("done");
    const closedMerge = await systemClient.json(`${project.path}/prs/${number}/merge`, {
      method: "POST",
      body: {},
      expectedStatus: 409,
    });
    expect(JSON.stringify(closedMerge.body)).toContain("a closed pull request cannot be merged");

    const moved = (await project.writeFile(branch, "farewell.txt", "goodbye\n")).commitOid;
    expect(await keyed("repo", "pr", "reopen", slug, n)).toBe(
      `reopened #${number} Add a greeting\n`,
    );
    const reopened = await view();
    expect(reopened).toContain(`#${number} Add a greeting [open]\n`);
    expect(reopened).toContain(`head ${moved.slice(0, 12)}`);
    await reviewRequest("unread");

    await keyed("repo", "pr", "close", slug, n);
    const working = join(scratch, "work");
    await git(["clone", gitRepositoryUrl(slug), working], { token: systemTestConfig.token });
    await git(["push", "origin", "--delete", branch], {
      cwd: working,
      token: systemTestConfig.token,
    });
    const gone = await run("--idempotency-key", randomUUID(), "repo", "pr", "reopen", slug, n);
    expect(gone.exitCode).not.toBe(0);
    expect(gone.stderr).toContain(
      `its branch ${branch} no longer exists; push it again to reopen this pull request`,
    );
    expect(await view()).toContain(`#${number} Add a greeting [closed]\n`);
  }, 240_000);
});
