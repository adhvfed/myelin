import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

export type CliResult = {
  exitCode: number | null;
  stdout: string;
  stderr: string;
};

export type PendingApproval = {
  code: string;
  stdout: () => string;
  stderr: () => string;
};

export function cliEnvironment(
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
  delete environment.MYELIN_PROJECT;
  return { ...environment, ...additions };
}

export function startCli(
  configDirectory: string,
  ...args: string[]
): ChildProcessWithoutNullStreams {
  return spawn("cargo", ["run", "--quiet", "-p", "myelin-cli", "--", ...args], {
    cwd: repository,
    env: cliEnvironment(configDirectory),
    stdio: "pipe",
  });
}

export async function waitForCode(
  child: ChildProcessWithoutNullStreams,
): Promise<PendingApproval> {
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
      const match = stdout.match(
        /Confirm this code in your browser: ([A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4})/,
      );
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

export async function finish(
  child: ChildProcessWithoutNullStreams,
  output: Pick<PendingApproval, "stdout" | "stderr">,
): Promise<string> {
  const exitCode = await new Promise<number | null>((resolveExit, reject) => {
    const timeout = setTimeout(() => reject(new Error("CLI did not finish after approval")), 15_000);
    child.once("error", reject);
    child.once("exit", (code) => {
      clearTimeout(timeout);
      resolveExit(code);
    });
  });
  if (exitCode !== 0) {
    throw new Error(`CLI exited ${exitCode} after approval; stderr=${output.stderr()}`);
  }
  return output.stdout();
}

export async function runCli(configDirectory: string, ...args: string[]): Promise<CliResult> {
  return runCliWith(configDirectory, {}, args);
}

export async function runCliWith(
  configDirectory: string,
  options: { environment?: NodeJS.ProcessEnv; input?: string },
  args: string[],
): Promise<CliResult> {
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
