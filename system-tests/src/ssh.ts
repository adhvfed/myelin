import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

const run = promisify(execFile);

export interface EphemeralSshKey {
  privateKeyPath: string;
  publicKey: string;
  remove(): Promise<void>;
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
