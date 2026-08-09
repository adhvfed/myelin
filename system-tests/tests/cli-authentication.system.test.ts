import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { gitRepositoryUrl, systemTestConfig } from "../src/config.js";
import { git } from "../src/git-cli.js";
import { GitProject } from "../src/git-project.js";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function cliEnvironment(
  configDirectory: string,
  additions: NodeJS.ProcessEnv = {},
): NodeJS.ProcessEnv {
  const environment: NodeJS.ProcessEnv = {
    ...process.env,
    MYELIN_CONFIG_DIR: configDirectory,
    MYELIN_TEST_CREDENTIAL_STORE: "file",
  };
  delete environment.MYELIN_TOKEN;
  delete environment.MYELIN_TOKEN_SCHEME;
  delete environment.MYELIN_EDGE;
  delete environment.MYELIN_PROFILE;
  return { ...environment, ...additions };
}

function startCli(configDirectory: string, ...args: string[]): ChildProcessWithoutNullStreams {
  return spawn(
    "cargo",
    ["run", "--quiet", "-p", "myelin-cli", "--", ...args],
    {
      cwd: repository,
      env: cliEnvironment(configDirectory),
      stdio: "pipe",
    },
  );
}

async function waitForCode(
  child: ChildProcessWithoutNullStreams,
): Promise<{ code: string; stdout: () => string; stderr: () => string }> {
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => { stdout += chunk; });
  child.stderr.on("data", (chunk: string) => { stderr += chunk; });

  const code = await new Promise<string>((resolveCode, reject) => {
    const timeout = setTimeout(() => {
      reject(new Error(`CLI did not print an approval code; stdout=${stdout} stderr=${stderr}`));
    }, 30_000);
    const inspect = () => {
      const match = stdout.match(/Confirm this code in your browser: ([A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4})/);
      if (!match) return;
      clearTimeout(timeout);
      resolveCode(match[1]!);
    };
    child.stdout.on("data", inspect);
    child.once("exit", (exitCode) => {
      clearTimeout(timeout);
      reject(new Error(`CLI exited ${exitCode} before approval; stdout=${stdout} stderr=${stderr}`));
    });
    inspect();
  });
  return { code, stdout: () => stdout, stderr: () => stderr };
}

async function finish(
  child: ChildProcessWithoutNullStreams,
  output: { stdout: () => string; stderr: () => string },
): Promise<string> {
  const exitCode = await new Promise<number | null>((resolveExit, reject) => {
    const timeout = setTimeout(() => reject(new Error("CLI did not finish after approval")), 15_000);
    child.once("error", reject);
    child.once("exit", (code) => {
      clearTimeout(timeout);
      resolveExit(code);
    });
  });
  expect(exitCode, `stderr=${output.stderr()}`).toBe(0);
  return output.stdout();
}

async function runCli(configDirectory: string, ...args: string[]) {
  return runCliWith(configDirectory, {}, args);
}

async function runCliWith(
  configDirectory: string,
  options: { environment?: NodeJS.ProcessEnv; input?: string },
  args: string[],
) {
  const child = spawn("cargo", ["run", "--quiet", "-p", "myelin-cli", "--", ...args], {
    cwd: repository,
    env: cliEnvironment(configDirectory, options.environment),
    stdio: "pipe",
  });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => { stdout += chunk; });
  child.stderr.on("data", (chunk: string) => { stderr += chunk; });
  child.stdin.end(options.input);
  const exitCode = await new Promise<number | null>((resolveExit, reject) => {
    child.once("error", reject);
    child.once("exit", resolveExit);
  });
  return { exitCode, stdout, stderr };
}

async function askGitForCredential(
  configDirectory: string,
  gitConfig: string,
  request: string,
) {
  const child = spawn("git", ["credential", "fill"], {
    cwd: repository,
    env: cliEnvironment(configDirectory, {
      GIT_CONFIG_GLOBAL: gitConfig,
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_TERMINAL_PROMPT: "0",
    }),
    stdio: "pipe",
  });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => { stdout += chunk; });
  child.stderr.on("data", (chunk: string) => { stderr += chunk; });
  child.stdin.end(request);
  const exitCode = await new Promise<number | null>((resolveExit, reject) => {
    child.once("error", reject);
    child.once("exit", resolveExit);
  });
  return { exitCode, stdout, stderr };
}

function profileSection(config: string, profile: string): string {
  const marker = `[profiles.${profile}]`;
  const start = config.indexOf(marker);
  if (start === -1) throw new Error(`config has no ${marker} section`);
  const next = config.indexOf("\n[profiles.", start + marker.length);
  return config.slice(start, next === -1 ? config.length : next);
}

function expireProfile(config: string, profile: string): string {
  const section = profileSection(config, profile);
  const expired = section.replace(/^expires_at_unix = \d+$/m, "expires_at_unix = 1");
  if (expired === section) throw new Error(`profile ${profile} has no numeric expiry`);
  return config.replace(section, expired);
}

describe("the CLI authentication journey", () => {
  test("a developer approves in the browser once, then works without copying an API key", async () => {
    const configDirectory = await mkdtemp(resolve(tmpdir(), "myelin-cli-system-"));
    let login: ChildProcessWithoutNullStreams | undefined;
    try {
      login = startCli(
        configDirectory,
        "--edge",
        systemTestConfig.edgeUrl,
        "auth",
        "login",
        "--no-browser",
      );
      const output = await waitForCode(login);

      const approved = await systemClient.json("/v1/auth/device/approval", {
        method: "POST",
        body: { user_code: output.code },
      });
      expect(approved.body).toEqual({ approved: true });

      const loginStory = await finish(login, output);
      expect(loginStory).toContain("Approved. Your CLI session is ready");
      expect(loginStory).not.toContain(systemTestConfig.token);

      const configPath = resolve(configDirectory, "config.toml");
      const firstConfig = await readFile(configPath, "utf8");
      const defaultProfile = profileSection(firstConfig, "default");
      expect(firstConfig).toContain('active_profile = "default"');
      expect(defaultProfile).toContain('scheme = "session"');
      expect(defaultProfile).toContain(`edge_url = "${systemTestConfig.edgeUrl}"`);
      expect(defaultProfile).toContain(`tenant = "${systemTestConfig.tenant}"`);
      expect(defaultProfile).toContain(`region = "${systemTestConfig.region}"`);
      expect(defaultProfile).toMatch(/^credential_ref = "[A-Za-z0-9_-]{22}"$/m);
      expect(defaultProfile).toMatch(/^expires_at_unix = \d+$/m);
      expect(firstConfig).not.toContain(systemTestConfig.token);
      if (process.platform !== "win32") {
        expect((await stat(configPath)).mode & 0o777).toBe(0o600);
      }

      // A second browser-approved identity becomes another named context—not another copied key.
      login = startCli(
        configDirectory,
        "--profile",
        "reviewer",
        "--edge",
        systemTestConfig.edgeUrl,
        "auth",
        "login",
        "--no-browser",
      );
      const reviewerOutput = await waitForCode(login);
      await reviewerClient.json("/v1/auth/device/approval", {
        method: "POST",
        body: { user_code: reviewerOutput.code },
      });
      const reviewerLoginStory = await finish(login, reviewerOutput);
      expect(reviewerLoginStory).toContain("Approved. Your CLI session is ready");
      expect(reviewerLoginStory).not.toContain(systemTestConfig.reviewerToken);

      const contexts = await runCli(configDirectory, "--json", "context", "list");
      expect(contexts.exitCode, contexts.stderr).toBe(0);
      expect(JSON.parse(contexts.stdout)).toMatchObject({
        profiles: [
          {
            name: "default",
            active: false,
            tenant: systemTestConfig.tenant,
            region: systemTestConfig.region,
          },
          {
            name: "reviewer",
            active: true,
            tenant: systemTestConfig.tenant,
            region: systemTestConfig.region,
          },
        ],
      });
      const twoProfileConfig = await readFile(configPath, "utf8");
      expect(twoProfileConfig.match(/^credential_ref = /gm)).toHaveLength(2);
      expect(twoProfileConfig).not.toContain(systemTestConfig.token);
      expect(twoProfileConfig).not.toContain(systemTestConfig.reviewerToken);

      const reviewerContext = await runCli(configDirectory, "--json", "context", "current");
      expect(reviewerContext.exitCode, reviewerContext.stderr).toBe(0);
      expect(JSON.parse(reviewerContext.stdout)).toMatchObject({
        profile: "reviewer",
        edge_url: systemTestConfig.edgeUrl,
        identity: {
          principal_id: systemTestConfig.reviewerPrincipal,
          tenant: systemTestConfig.tenant,
          region: systemTestConfig.region,
        },
      });

      const gitConfig = resolve(configDirectory, "gitconfig");
      const gitEnvironment = {
        GIT_CONFIG_GLOBAL: gitConfig,
        GIT_CONFIG_NOSYSTEM: "1",
      };
      const configureReviewerGit = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["auth", "configure-git"],
      );
      expect(configureReviewerGit.exitCode, configureReviewerGit.stderr).toBe(0);

      const chooseDefault = await runCli(configDirectory, "context", "use", "default");
      expect(chooseDefault.exitCode, chooseDefault.stderr).toBe(0);
      expect(chooseDefault.stdout).toContain("Using CLI context `default`");

      const status = await runCli(configDirectory, "auth", "status");
      expect(status.exitCode, status.stderr).toBe(0);
      expect(status.stdout).toContain(systemTestConfig.principal);
      expect(status.stdout).toContain(`tenant=${systemTestConfig.tenant}`);

      // The project belongs to the context, so ordinary work does not repeat an opaque UUID.
      const chooseProject = await runCli(
        configDirectory,
        "context",
        "use",
        "--project",
        systemTestConfig.issues.projectId,
      );
      expect(chooseProject.exitCode, chooseProject.stderr).toBe(0);
      expect(chooseProject.stdout).toContain(
        `Default project: ${systemTestConfig.issues.projectId}`,
      );

      const contextualIssueTitle = uniqueName("Created from the active project");
      const contextualIssue = await runCli(
        configDirectory,
        "--json",
        "--idempotency-key",
        uniqueName("cli-context-issue"),
        "issue",
        "create",
        "--type",
        systemTestConfig.issues.typeId,
        "--prefix",
        systemTestConfig.issues.prefix,
        "--title",
        contextualIssueTitle,
      );
      expect(contextualIssue.exitCode, contextualIssue.stderr).toBe(0);
      expect(JSON.parse(contextualIssue.stdout)).toMatchObject({
        issue: { project_id: systemTestConfig.issues.projectId },
        authorization: { status: "pending" },
      });

      const contextAfterProject = await runCli(configDirectory, "--json", "context", "current");
      expect(contextAfterProject.exitCode, contextAfterProject.stderr).toBe(0);
      expect(JSON.parse(contextAfterProject.stdout)).toMatchObject({
        profile: "default",
        project: systemTestConfig.issues.projectId,
      });

      const repositories = await runCli(configDirectory, "repo", "list");
      expect(repositories.exitCode, repositories.stderr).toBe(0);

      const configureGit = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["auth", "configure-git"],
      );
      expect(configureGit.exitCode, configureGit.stderr).toBe(0);
      expect(configureGit.stdout).toContain("Git is ready");
      expect(configureGit.stdout).not.toContain(systemTestConfig.token);

      // One Edge gets one Myelin helper. Reconfiguring replaces the old profile binding instead
      // of letting Git accept whichever helper happens to answer first.
      const edgeOrigin = new URL(systemTestConfig.edgeUrl).origin;
      const helpers = await git(
        ["config", "--global", "--get-all", `credential.${edgeOrigin}.helper`],
        { environment: gitEnvironment },
      );
      expect(helpers.stdout.trim().split("\n")).toHaveLength(1);
      expect(helpers.stdout).toContain("--profile 'default' auth git-credential");

      const edge = new URL(systemTestConfig.edgeUrl);
      const credential = await askGitForCredential(
        configDirectory,
        gitConfig,
        `protocol=${edge.protocol.slice(0, -1)}\nhost=${edge.host}\npath=${systemTestConfig.tenant}/${systemTestConfig.region}/repo.git\n\n`,
      );
      expect(credential.exitCode, credential.stderr).toBe(0);
      expect(credential.stdout).toContain("username=myelin-session");
      const gitPassword = credential.stdout.match(/^password=(.+)$/m)?.[1];
      expect(gitPassword).toBeTruthy();
      expect(gitPassword).not.toBe(systemTestConfig.token);

      const project = new GitProject(uniqueName("cli-session-wire"), systemClient);
      await project.create();
      const remote = await git(["ls-remote", gitRepositoryUrl(project.slug)], {
        environment: cliEnvironment(configDirectory, {
          ...gitEnvironment,
          GIT_TERMINAL_PROMPT: "0",
        }),
      });
      expect(remote.stdout).toBe("");
      expect(remote.stderr).not.toContain(String(gitPassword));

      const stranger = await runCliWith(
        configDirectory,
        {
          input: "protocol=https\nhost=not-myelin.example\npath=stolen.git\n\n",
        },
        ["auth", "git-credential", "get"],
      );
      expect(stranger.exitCode, stranger.stderr).toBe(0);
      expect(stranger.stdout).toBe("");

      // Once browser approval reaches its deadline, neither the CLI nor its Git helper pretends
      // that the old session is still useful—and neither needs to send the secret to discover it.
      await writeFile(configPath, expireProfile(await readFile(configPath, "utf8"), "default"));
      const afterExpiry = await runCli(configDirectory, "auth", "status");
      expect(afterExpiry.exitCode).toBe(3);
      expect(afterExpiry.stderr).toContain("saved CLI session has expired");
      expect(afterExpiry.stderr).not.toContain(String(gitPassword));

      const gitAfterExpiry = await runCliWith(
        configDirectory,
        {
          input: `protocol=${edge.protocol.slice(0, -1)}\nhost=${edge.host}\npath=acme/eu/repo.git\n\n`,
        },
        ["auth", "git-credential", "get"],
      );
      expect(gitAfterExpiry.exitCode).toBe(3);
      expect(gitAfterExpiry.stdout).toBe("");
      expect(gitAfterExpiry.stderr).toContain("saved CLI session has expired");
      expect(gitAfterExpiry.stderr).not.toContain(String(gitPassword));

      const unconfigureGit = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["auth", "unconfigure-git"],
      );
      expect(unconfigureGit.exitCode, unconfigureGit.stderr).toBe(0);
      expect(unconfigureGit.stdout).toContain("Removed the Myelin credential helper");

      const logout = await runCli(configDirectory, "auth", "logout");
      expect(logout.exitCode, logout.stderr).toBe(0);
      expect(logout.stdout).toContain("Removed the selected profile's stored credential");

      // Removing the active context falls through to the other saved identity without losing it.
      const reviewerAfterLogout = await runCli(configDirectory, "auth", "status");
      expect(reviewerAfterLogout.exitCode, reviewerAfterLogout.stderr).toBe(0);
      expect(reviewerAfterLogout.stdout).toContain(systemTestConfig.reviewerPrincipal);

      const finalLogout = await runCli(configDirectory, "auth", "logout");
      expect(finalLogout.exitCode, finalLogout.stderr).toBe(0);
      expect(await readdir(resolve(configDirectory, ".test-credentials"))).toEqual([]);
      await expect(stat(configPath)).rejects.toMatchObject({ code: "ENOENT" });

      const afterLogout = await runCli(configDirectory, "auth", "status");
      expect(afterLogout.exitCode).toBe(3);
      expect(afterLogout.stderr).toContain("run `myelin auth login`");
    } finally {
      if (login && login.exitCode === null) login.kill("SIGTERM");
      await rm(configDirectory, { recursive: true, force: true });
    }
  }, 90_000);
});
