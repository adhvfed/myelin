import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, test } from "vitest";

import { systemClient, uniqueName } from "../src/context.js";
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
  };
  delete environment.MYELIN_TOKEN;
  delete environment.MYELIN_TOKEN_SCHEME;
  delete environment.MYELIN_EDGE;
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

      const credentialPath = resolve(configDirectory, "credentials.json");
      const stored = JSON.parse(await readFile(credentialPath, "utf8")) as Record<string, unknown>;
      expect(stored).toMatchObject({
        version: 1,
        scheme: "session",
        edge_url: systemTestConfig.edgeUrl,
      });
      expect(stored.token).not.toBe(systemTestConfig.token);
      if (process.platform !== "win32") {
        expect((await stat(credentialPath)).mode & 0o777).toBe(0o600);
      }

      const status = await runCli(configDirectory, "auth", "status");
      expect(status.exitCode, status.stderr).toBe(0);
      expect(status.stdout).toContain(systemTestConfig.principal);
      expect(status.stdout).toContain(`tenant=${systemTestConfig.tenant}`);

      const gitConfig = resolve(configDirectory, "gitconfig");
      const gitEnvironment = {
        GIT_CONFIG_GLOBAL: gitConfig,
        GIT_CONFIG_NOSYSTEM: "1",
      };
      const configureGit = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["auth", "configure-git"],
      );
      expect(configureGit.exitCode, configureGit.stderr).toBe(0);
      expect(configureGit.stdout).toContain("Git is ready");
      expect(configureGit.stdout).not.toContain(stored.token);

      const edge = new URL(systemTestConfig.edgeUrl);
      const credential = await askGitForCredential(
        configDirectory,
        gitConfig,
        `protocol=${edge.protocol.slice(0, -1)}\nhost=${edge.host}\npath=${systemTestConfig.tenant}/${systemTestConfig.region}/repo.git\n\n`,
      );
      expect(credential.exitCode, credential.stderr).toBe(0);
      expect(credential.stdout).toContain("username=myelin-session");
      expect(credential.stdout).toContain(`password=${String(stored.token)}`);

      const project = new GitProject(uniqueName("cli-session-wire"), systemClient);
      await project.create();
      const remote = await git(["ls-remote", gitRepositoryUrl(project.slug)], {
        environment: cliEnvironment(configDirectory, {
          ...gitEnvironment,
          GIT_TERMINAL_PROMPT: "0",
        }),
      });
      expect(remote.stdout).toBe("");
      expect(remote.stderr).not.toContain(stored.token);

      const stranger = await runCliWith(
        configDirectory,
        {
          input: "protocol=https\nhost=not-myelin.example\npath=stolen.git\n\n",
        },
        ["auth", "git-credential", "get"],
      );
      expect(stranger.exitCode, stranger.stderr).toBe(0);
      expect(stranger.stdout).toBe("");

      const unconfigureGit = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["auth", "unconfigure-git"],
      );
      expect(unconfigureGit.exitCode, unconfigureGit.stderr).toBe(0);
      expect(unconfigureGit.stdout).toContain("Removed the Myelin credential helper");

      const logout = await runCli(configDirectory, "auth", "logout");
      expect(logout.exitCode, logout.stderr).toBe(0);
      expect(logout.stdout).toContain("Removed stored credentials");

      const afterLogout = await runCli(configDirectory, "auth", "status");
      expect(afterLogout.exitCode).toBe(3);
      expect(afterLogout.stderr).toContain("run `myelin auth login`");
    } finally {
      if (login && login.exitCode === null) login.kill("SIGTERM");
      await rm(configDirectory, { recursive: true, force: true });
    }
  }, 60_000);
});
