import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterAll, beforeAll, describe, expect, test } from "vitest";

import { systemClient, uniqueName } from "../src/context.js";
import { git, GitCommandError } from "../src/git-cli.js";
import { GitProject } from "../src/git-project.js";
import { array } from "../src/json.js";
import { gitRepositoryUrl, systemTestConfig } from "../src/config.js";

describe.sequential("Git smart-HTTP lifecycle", () => {
  const slug = uniqueName("system-wire");
  const project = new GitProject(slug, systemClient);
  const repositoryUrl = gitRepositoryUrl(slug);
  const namespacedSlug = `team/${uniqueName("system-wire-nested")}`;
  const namespacedProject = new GitProject(namespacedSlug, systemClient);
  let root = "";
  let working = "";
  let mainOid = "";
  let featureOid = "";

  function requireWorkingCopy(): void {
    if (!mainOid) {
      throw new Error("native Git working copy was not initialized by the preceding lifecycle step");
    }
  }

  beforeAll(async () => {
    await project.create();
    root = await mkdtemp(join(tmpdir(), "myelin-system-git-"));
    working = join(root, "working");
  });

  afterAll(async () => {
    if (root) await rm(root, { recursive: true, force: true });
  });

  test("refuses an unauthenticated native Git client", async () => {
    let failure: unknown;
    try {
      await git(["ls-remote", repositoryUrl]);
    } catch (error) {
      failure = error;
    }
    expect(failure).toBeInstanceOf(GitCommandError);
    expect((failure as GitCommandError).stderr).not.toContain(systemTestConfig.token);
  });

  test("rejects personal commit identity, then accepts a pseudonymous first push", async () => {
    await git(["clone", repositoryUrl, working], { token: systemTestConfig.token });
    await git(["config", "user.name", "Myelin System Test"], { cwd: working });
    await git(["config", "user.email", "system-test@myelin.invalid"], { cwd: working });
    await writeFile(
      join(working, "README.md"),
      `# ${slug}\n\nPushed with a stock Git client over smart HTTP.\n`,
      "utf8",
    );
    await git(["add", "README.md"], { cwd: working });
    await git(["commit", "-m", "feat: initialize through smart HTTP"], { cwd: working });
    await expect(
      git(["push", "origin", "HEAD:refs/heads/main"], {
        cwd: working,
        token: systemTestConfig.token,
      }),
    ).rejects.toMatchObject({
      stderr: expect.stringContaining("NonPseudonymousCommit"),
    });
    expect(
      (await git(["ls-remote", repositoryUrl, "refs/heads/main"], {
        token: systemTestConfig.token,
      })).stdout,
    ).toBe("");

    const pseudonym = `${systemTestConfig.principal}@${systemTestConfig.tenant}.noreply`;
    await git(["config", "user.name", pseudonym], { cwd: working });
    await git(["config", "user.email", pseudonym], { cwd: working });
    await git(["commit", "--amend", "--no-edit", "--reset-author"], { cwd: working });
    await git(["push", "origin", "HEAD:refs/heads/main"], {
      cwd: working,
      token: systemTestConfig.token,
    });
    mainOid = (await git(["rev-parse", "HEAD"], { cwd: working })).stdout.trim();

    const remote = await git(["ls-remote", repositoryUrl, "refs/heads/main"], {
      token: systemTestConfig.token,
    });
    expect(remote.stdout.trim()).toBe(`${mainOid}\trefs/heads/main`);
    const home = await systemClient.json(project.path);
    expect(home.body).toMatchObject({
      state: "populated",
      snapshot_oid: mainOid,
      readme: expect.stringContaining("stock Git client"),
    });
  });

  test("pushes a feature branch and exposes both refs through Edge", async () => {
    requireWorkingCopy();
    await git(["checkout", "-b", "feature/native-wire"], { cwd: working });
    await writeFile(join(working, "delivery.txt"), "native wire delivery\n", "utf8");
    await git(["add", "delivery.txt"], { cwd: working });
    await git(["commit", "-m", "feat: exercise feature push"], { cwd: working });
    featureOid = (await git(["rev-parse", "HEAD"], { cwd: working })).stdout.trim();
    await git(["push", "origin", "HEAD:refs/heads/feature/native-wire"], {
      cwd: working,
      token: systemTestConfig.token,
    });

    const refs = await systemClient.json(`${project.path}/refs?limit=100`);
    expect(array(refs.body.branches, "smart-HTTP repository branches")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ name: "main", oid: mainOid }),
        expect.objectContaining({ name: "feature/native-wire", oid: featureOid }),
      ]),
    );
  });

  test("enforces repository read grants for an authenticated native Git client", async () => {
    requireWorkingCopy();
    let failure: unknown;
    try {
      await git(["ls-remote", repositoryUrl], { token: systemTestConfig.reviewerToken });
    } catch (error) {
      failure = error;
    }
    expect(failure).toBeInstanceOf(GitCommandError);
    expect((failure as GitCommandError).stderr).not.toContain(systemTestConfig.reviewerToken);
  });

  test("clones the published default branch into a clean worktree", async () => {
    requireWorkingCopy();
    const clean = join(root, "clean-clone");
    await git(["clone", "--branch", "main", repositoryUrl, clean], {
      token: systemTestConfig.token,
    });
    expect((await git(["rev-parse", "HEAD"], { cwd: clean })).stdout.trim()).toBe(mainOid);
    expect(await readFile(join(clean, "README.md"), "utf8")).toContain("stock Git client");
    await expect(readFile(join(clean, "delivery.txt"), "utf8")).rejects.toMatchObject({
      code: "ENOENT",
    });
  });

  test("clones a namespaced repository from its advertised URL", async () => {
    await namespacedProject.create();
    const commitOid = (await namespacedProject.writeFile(
      "main",
      "README.md",
      `# ${namespacedSlug}\n`,
    )).commitOid;
    const advertised = await systemClient.json(namespacedProject.path);
    const namespacedUrl = gitRepositoryUrl(namespacedSlug);
    expect(advertised.body).toMatchObject({
      clone_url: namespacedUrl,
      snapshot_oid: commitOid,
    });

    const clone = join(root, "namespaced-clone");
    await git(["clone", "--branch", "main", namespacedUrl, clone], {
      token: systemTestConfig.token,
    });
    expect((await git(["rev-parse", "HEAD"], { cwd: clone })).stdout.trim()).toBe(commitOid);
    expect(await readFile(join(clone, "README.md"), "utf8")).toContain(namespacedSlug);
  });
});
