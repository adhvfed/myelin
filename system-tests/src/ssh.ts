import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { promisify } from "node:util";

const run = promisify(execFile);

export interface EphemeralSshKey {
  privateKeyPath: string;
  publicKey: string;
  remove(): Promise<void>;
}

interface WorkspaceSshAccess {
  host: string;
  port: number;
  username: string;
  host_public_key: string;
}

export interface WorkspaceOverSsh {
  hasInteractiveTerminal(): Promise<boolean>;
  readText(path: string): Promise<string>;
  writeText(path: string, content: string): Promise<void>;
  waitForText(path: string, content: string): Promise<void>;
}

export async function generateEphemeralSshKey(): Promise<EphemeralSshKey> {
  const directory = await mkdtemp(join(tmpdir(), "myelin-workspace-ssh-"));
  const privateKeyPath = join(directory, "id_ed25519");
  try {
    await run("ssh-keygen", [
      "-q",
      "-t",
      "ed25519",
      "-N",
      "",
      "-C",
      "myelin-one-shot",
      "-f",
      privateKeyPath,
    ]);
    const publicKey = (await readFile(`${privateKeyPath}.pub`, "utf8")).trim();
    return {
      privateKeyPath,
      publicKey,
      remove: () => rm(directory, { recursive: true, force: true }),
    };
  } catch (error) {
    await rm(directory, { recursive: true, force: true });
    throw error;
  }
}

export async function connectToWorkspace(
  key: EphemeralSshKey,
  access: WorkspaceSshAccess,
): Promise<WorkspaceOverSsh> {
  const knownHostsPath = join(dirname(key.privateKeyPath), "known_hosts");
  const host = knownHostName(access.host, access.port);
  await writeFile(knownHostsPath, `${host} ${access.host_public_key}\n`, { mode: 0o600 });

  return {
    async hasInteractiveTerminal(): Promise<boolean> {
      const answer = await runWorkspaceCommand(
        key,
        access,
        knownHostsPath,
        "if test -t 0 && test -t 1; then printf yes; else printf no; fi",
        true,
      );
      return answer.trim() === "yes";
    },
    async readText(path: string): Promise<string> {
      const relative = workspacePath(path);
      return runWorkspaceCommand(key, access, knownHostsPath, `cat ${shellWord(relative)}`);
    },
    async writeText(path: string, content: string): Promise<void> {
      const relative = workspacePath(path);
      const parent = dirname(relative);
      const prepare = parent === "." ? "" : `mkdir -p ${shellWord(parent)} && `;
      await runWorkspaceCommand(
        key,
        access,
        knownHostsPath,
        `${prepare}printf %s ${shellWord(content)} > ${shellWord(relative)}`,
      );
    },
    async waitForText(path: string, content: string): Promise<void> {
      const relative = workspacePath(path);
      const expected = shellWord(content);
      await runWorkspaceCommand(
        key,
        access,
        knownHostsPath,
        `for attempt in $(seq 1 40); do if test -f ${shellWord(relative)} && test "$(cat ${shellWord(relative)})" = ${expected}; then exit 0; fi; sleep 0.25; done; exit 1`,
      );
    },
  };
}

async function runWorkspaceCommand(
  key: EphemeralSshKey,
  access: WorkspaceSshAccess,
  knownHostsPath: string,
  command: string,
  terminal = false,
): Promise<string> {
  try {
    const result = await run(
      "ssh",
      [
        "-F",
        "/dev/null",
        terminal ? "-tt" : "-T",
        "-i",
        key.privateKeyPath,
        "-l",
        access.username,
        "-p",
        String(access.port),
        "-o",
        "BatchMode=yes",
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "ControlMaster=no",
        "-o",
        "PasswordAuthentication=no",
        "-o",
        "KbdInteractiveAuthentication=no",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        `UserKnownHostsFile=${knownHostsPath}`,
        "-o",
        "GlobalKnownHostsFile=/dev/null",
        "-o",
        "ConnectTimeout=10",
        access.host,
        command,
      ],
      { timeout: 30_000, maxBuffer: 512 * 1024 },
    );
    return result.stdout;
  } catch {
    throw new Error("the host-key-pinned workspace SSH command failed");
  }
}

function knownHostName(host: string, port: number): string {
  return port === 22 ? host : `[${host}]:${port}`;
}

function workspacePath(path: string): string {
  const parts = path.split("/");
  if (
    path.length === 0
    || path.length > 1_024
    || path.startsWith("/")
    || parts.some((part) => part.length === 0 || part === "." || part === ".." || part.includes("\0"))
  ) {
    throw new TypeError("workspace SSH paths must be relative canonical paths");
  }
  return path;
}

function shellWord(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}
