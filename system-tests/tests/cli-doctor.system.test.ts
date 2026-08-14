import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

import { describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import { systemClient, uniqueName } from "../src/context.js";
import { array, record, string } from "../src/json.js";
import { finish, runCli, runCliWith, startCli, waitForCode } from "../src/myelin-cli.js";

function check(report: Record<string, unknown>, name: string): Record<string, unknown> {
  const found = array(report.checks, "doctor checks")
    .map((value, index) => record(value, `doctor check ${index}`))
    .find((value) => value.name === name);
  if (!found) throw new Error(`doctor omitted ${name}`);
  return found;
}

describe("the CLI development-context diagnosis", () => {
  test("turns one browser-approved session into an actionable, verified development context", async () => {
    const configDirectory = await mkdtemp(resolve(tmpdir(), "myelin-doctor-system-"));
    const gitConfig = resolve(configDirectory, "gitconfig");
    const gitEnvironment = {
      GIT_CONFIG_GLOBAL: gitConfig,
      GIT_CONFIG_NOSYSTEM: "1",
    };
    const login = startCli(
      configDirectory,
      "--edge",
      systemTestConfig.edgeUrl,
      "auth",
      "login",
      "--no-browser",
    );

    try {
      const approval = await waitForCode(login);
      await systemClient.json("/v1/auth/device/approval", {
        method: "POST",
        body: { user_code: approval.code },
      });
      const loginStory = await finish(login, approval);
      expect(loginStory).toContain("Your CLI session is ready");
      expect(loginStory).not.toContain(systemTestConfig.token);

      const firstDiagnosis = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["--json", "doctor"],
      );
      expect(firstDiagnosis.exitCode, firstDiagnosis.stderr).toBe(0);
      const before = record(JSON.parse(firstDiagnosis.stdout), "first doctor report");
      expect(before.ready).toBe(false);
      expect(check(before, "identity")).toMatchObject({ status: "ready" });
      expect(check(before, "project")).toMatchObject({
        status: "attention",
        next_command: "myelin context use --project <project-id>",
      });
      expect(check(before, "git authentication")).toMatchObject({
        status: "attention",
        next_command: "myelin auth configure-git",
      });

      const selectProject = await runCli(
        configDirectory,
        "context",
        "use",
        "--project",
        systemTestConfig.issues.projectId,
      );
      expect(selectProject.exitCode, selectProject.stderr).toBe(0);

      const repository = `doctor-${uniqueName("workspace")}`;
      const createRepository = await runCli(
        configDirectory,
        "repo",
        "create",
        repository,
        "--idempotency-key",
        `create-${repository}`,
      );
      expect(createRepository.exitCode, createRepository.stderr).toBe(0);

      const configureGit = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["auth", "configure-git"],
      );
      expect(configureGit.exitCode, configureGit.stderr).toBe(0);

      const diagnosed = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["--json", "doctor"],
      );
      expect(diagnosed.exitCode, diagnosed.stderr).toBe(0);
      expect(diagnosed.stdout).not.toContain(systemTestConfig.token);
      expect(diagnosed.stderr).not.toContain(systemTestConfig.token);

      const after = record(JSON.parse(diagnosed.stdout), "ready doctor report");
      expect(after, JSON.stringify(after, null, 2)).toMatchObject({
        ready: true,
        profile: "default",
        edge_url: systemTestConfig.edgeUrl,
      });
      for (const value of array(after.checks, "ready doctor checks")) {
        expect(record(value, "ready doctor check").status).toBe("ready");
      }
      expect(string(check(after, "project").summary, "project diagnosis")).toContain(
        systemTestConfig.issues.projectId,
      );

      const humanDiagnosis = await runCliWith(
        configDirectory,
        { environment: gitEnvironment },
        ["doctor"],
      );
      expect(humanDiagnosis.exitCode, humanDiagnosis.stderr).toBe(0);
      expect(humanDiagnosis.stdout).toContain("Myelin doctor: ready");
      expect(humanDiagnosis.stdout).toContain("[ok] git authentication");
    } finally {
      if (login.exitCode === null) login.kill();
      await rm(configDirectory, { recursive: true, force: true });
    }
  });
});
