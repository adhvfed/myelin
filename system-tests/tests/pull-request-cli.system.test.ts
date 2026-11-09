import { randomUUID } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import {
  browserApprovedCliCredential,
  reviewerClient,
  systemClient,
  uniqueName,
} from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import { runCliWith } from "../src/myelin-cli.js";

describe("pull requests from the terminal", () => {
  let configDirectory: string;

  beforeAll(async () => {
    configDirectory = await mkdtemp(resolve(tmpdir(), "myelin-pull-request-cli-"));
  });

  afterAll(async () => {
    await rm(configDirectory, { recursive: true, force: true });
  });

  test("a developer opens, updates, and queues a pull request by following what the CLI says", async () => {
    const session = await browserApprovedCliCredential();
    const myelin = async (...args: string[]): Promise<string> => {
      const result = await runCliWith(
        configDirectory,
        {
          environment: {
            MYELIN_EDGE: systemTestConfig.edgeUrl,
            MYELIN_TOKEN: session.token,
            MYELIN_TOKEN_SCHEME: session.tokenScheme,
          },
        },
        args,
      );
      expect(result.exitCode, result.stderr).toBe(0);
      return result.stdout;
    };

    const project = new GitProject(uniqueName("pull-request-cli"), systemClient);
    await project.create();
    await systemClient.json(`${project.path}/branch-protection`, {
      method: "POST",
      body: {
        rulesets: [{
          ref_pattern: "refs/heads/main",
          required_contexts: [],
          required_approvals: 1,
          require_codeowner_review: false,
          require_conversation_resolution: false,
          allow_force_push: false,
        }],
      },
    });
    await project.writeFile("main", "README.md", "# Greetings\n");
    const head = (await project.writeFile("greet", "greeting.txt", "hello\n", {
      startRef: "main",
    })).commitOid;

    const opened = await myelin(
      "--idempotency-key", `open-${randomUUID()}`,
      "repo", "pr", "open", project.slug,
      "--title", "Add a greeting",
      "--base", "refs/heads/main",
      "--head", "refs/heads/greet",
      "--head-oid", head,
      "--reviewer", systemTestConfig.reviewerPrincipal,
    );
    const number = Number(/^opened #(\d+) /.exec(opened)?.[1]);
    const unknownReviewer = await runCliWith(
      configDirectory,
      {
        environment: {
          MYELIN_EDGE: systemTestConfig.edgeUrl,
          MYELIN_TOKEN: session.token,
          MYELIN_TOKEN_SCHEME: session.tokenScheme,
        },
      },
      [
        "--idempotency-key", `open-${randomUUID()}`,
        "repo", "pr", "open", project.slug,
        "--title", "Ask nobody",
        "--base", "refs/heads/main",
        "--head", "refs/heads/greet",
        "--head-oid", head,
        "--reviewer", "nobody-by-this-name",
      ],
    );
    expect(unknownReviewer.exitCode).not.toBe(0);
    expect(unknownReviewer.stderr).toContain(
      "`nobody-by-this-name` is not an active person or agent here, so they cannot review",
    );
    expect(opened).toBe(`opened #${number} Add a greeting [open]  greet → main · review requested\n`);

    await project.writeFile("main", "NEWS.md", "Greetings arrive.\n");
    const view = () => myelin("repo", "pr", "view", project.slug, String(number));
    const behind = await view();
    expect(behind).toContain(
      `#${number} Add a greeting [open]\n  greet → main · head ${head.slice(0, 12)} · 1 commit · review requested\n`,
    );
    const hint = `myelin --idempotency-key <key> repo pr update ${project.slug} ${number}`;
    expect(behind).toContain(`  main has moved on; update the branch to merge: ${hint}\n`);

    const [, ...update] = hint.replace("<key>", `update-${randomUUID()}`).split(" ");
    expect(await myelin(...update)).toMatch(
      /^merged the base into the branch; head is now [0-9a-f]{12}\n$/,
    );

    expect(await myelin(
      "--idempotency-key", `queue-${randomUUID()}`,
      "repo", "pr", "merge", project.slug, String(number), "--auto",
    )).toBe("will merge automatically once approved, up to date with its base, and green\n");
    const queued = await view();
    expect(queued).not.toContain("has moved on");
    expect(queued).toMatch(/\n {2}merging when ready · next in line for main · asked by \S+\n/);
    expect(await myelin("repo", "pr", "list", project.slug)).toContain(
      `#${number} Add a greeting [open]  greet → main · review requested\n`,
    );

    await reviewerClient.json(`${project.path}/prs/${number}/reviews`, {
      method: "POST",
      body: { verdict: "approve" },
    });
    const merged = await eventually(async () => {
      const current = await view();
      return current.includes("[merged]") ? current : undefined;
    }, { timeoutMs: 60_000, intervalMs: 1_000, description: "the queue to merge the approved pull request" });
    expect(merged).toMatch(/\n {2}merged as [0-9a-f]{12}\n/);
    expect(merged).not.toContain("merging when ready");

    expect(await myelin(
      "--idempotency-key", `edit-${randomUUID()}`,
      "repo", "pr", "edit", project.slug, String(number),
      "--title", "Add a friendly greeting",
    )).toBe(`edited #${number} Add a friendly greeting [merged]  greet → main · approved\n`);
    expect(await view()).toContain(`#${number} Add a friendly greeting [merged]\n`);
  }, 240_000);
});
