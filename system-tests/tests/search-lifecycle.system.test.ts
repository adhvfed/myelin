import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { gitRepositoryUrl, systemTestConfig } from "../src/config.js";
import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { git } from "../src/git-cli.js";
import { GitProject } from "../src/git-project.js";
import { array } from "../src/json.js";

describe.sequential("Code search lifecycle", () => {
  const slug = uniqueName("system-search");
  const project = new GitProject(slug, systemClient);
  const repositoryUrl = gitRepositoryUrl(slug);
  const originalToken = `original_${slug.replaceAll("-", "_")}`;
  const replacementToken = `replacement_${slug.replaceAll("-", "_")}`;
  const featureToken = `feature_${slug.replaceAll("-", "_")}`;
  const sourcePath = "src/search-lifecycle.ts";
  let mainOid = "";
  let root = "";

  beforeAll(async () => {
    await project.create();
    await project.writeFile("main", "README.md", `# ${slug}\n`);
    mainOid = (await project.writeFile(
      "main",
      sourcePath,
      `export const marker = "${originalToken}";\n`,
    )).commitOid;
  });

  afterAll(async () => {
    if (root) await rm(root, { recursive: true, force: true });
  });

  test("returns exact coordinates from the current default-branch snapshot", async () => {
    const search = await project.searchCode(originalToken);
    expect(search.complete).toBe(true);
    expect(search.items).toEqual([
      expect.objectContaining({
        repo: slug,
        ref: "refs/heads/main",
        snapshot_oid: mainOid,
        path: sourcePath,
        line: 1,
        excerpt: `export const marker = "${originalToken}";`,
      }),
    ]);
  });

  test("does not reveal a repository to an unrelated principal", async () => {
    const params = new URLSearchParams({ repo: slug, q: originalToken });
    const response = await reviewerClient.json(`/v1/git/search/code?${params}`);
    expect(response.body).toMatchObject({ complete: true });
    expect(array(response.body.items, "unauthorized code search items")).toEqual([]);
  });

  test("replaces stale matches when the default branch advances", async () => {
    mainOid = (await project.updateFile(
      "main",
      sourcePath,
      `export const marker = "${replacementToken}";\n`,
    )).commitOid;

    expect((await project.searchCode(originalToken)).items).toEqual([]);
    expect((await project.searchCode(replacementToken)).items).toEqual([
      expect.objectContaining({
        snapshot_oid: mainOid,
        path: sourcePath,
        excerpt: `export const marker = "${replacementToken}";`,
      }),
    ]);
  });

  test("keeps feature-branch content out of default-branch results until promotion", async () => {
    await project.updateFile(
      "feature/search-lifecycle",
      sourcePath,
      `export const marker = "${featureToken}";\n`,
      { startRef: "main" },
    );

    expect((await project.searchCode(featureToken)).items).toEqual([]);
    expect((await project.searchCode(replacementToken)).items).toHaveLength(1);

    mainOid = (await project.updateFile(
      "main",
      sourcePath,
      `export const marker = "${featureToken}";\n`,
    )).commitOid;
    expect((await project.searchCode(featureToken)).items).toEqual([
      expect.objectContaining({ snapshot_oid: mainOid, path: sourcePath }),
    ]);
    expect((await project.searchCode(replacementToken)).items).toEqual([]);
  });

  test("removes deleted files from browse and search after a stock Git push", async () => {
    root = await mkdtemp(join(tmpdir(), "myelin-system-search-"));
    const working = join(root, "working");
    await git(["clone", "--branch", "main", repositoryUrl, working], {
      token: systemTestConfig.token,
    });
    const pseudonym = `${systemTestConfig.principal}@${systemTestConfig.tenant}.noreply`;
    await git(["config", "user.name", pseudonym], { cwd: working });
    await git(["config", "user.email", pseudonym], { cwd: working });
    await git(["rm", sourcePath], { cwd: working });
    await git(["commit", "-m", "test: remove search lifecycle marker"], { cwd: working });
    await git(["push", "origin", "HEAD:refs/heads/main"], {
      cwd: working,
      token: systemTestConfig.token,
    });

    await systemClient.json(`${project.path}/blob/main/${sourcePath}`, { expectedStatus: 404 });
    expect((await project.searchCode(featureToken)).items).toEqual([]);
  });
});
