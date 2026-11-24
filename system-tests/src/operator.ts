import { execFile } from "node:child_process";
import { promisify } from "node:util";

const run = promisify(execFile);

export interface AgentThreadReconciliation {
  madeInaccessible: number;
  cleanupCandidates: number;
  deleted: number;
  cleanupFailures: number;
}

export async function reconcileAgentThreads(
  observedAt: string,
): Promise<AgentThreadReconciliation> {
  const tenant = process.env.MYELIN_SYSTEM_TEST_TENANT;
  if (!tenant) throw new Error("MYELIN_SYSTEM_TEST_TENANT is required");
  try {
    const result = await run(
      "cargo",
      [
        "run",
        "--quiet",
        "-p",
        "myelin-edge",
        "--bin",
        "edge",
        "--",
        "agent-thread-reconcile",
        "--tenant",
        tenant,
        "--now",
        observedAt,
      ],
      { timeout: 60_000, maxBuffer: 256 * 1024 },
    );
    const match = result.stderr.match(
      /made_inaccessible=(\d+) cleanup_candidates=(\d+) deleted=(\d+) cleanup_failures=(\d+)/,
    );
    if (!match) throw new Error("the reconciliation report was malformed");
    return {
      madeInaccessible: Number(match[1]),
      cleanupCandidates: Number(match[2]),
      deleted: Number(match[3]),
      cleanupFailures: Number(match[4]),
    };
  } catch {
    throw new Error("private agent thread reconciliation failed");
  }
}

/** Runs `edge invite` or `edge reset-link` as the operator and returns the printed link. */
export async function operatorLink(
  command: "invite" | "reset-link",
  email: string,
  tenant: string = requiredTenant(),
): Promise<string> {
  const result = await run(
    "cargo",
    ["run", "--quiet", "-p", "myelin-edge", "--bin", "edge", "--", command, "--tenant", tenant, "--email", email],
    { timeout: 120_000, maxBuffer: 256 * 1024 },
  );
  const link = result.stdout.trim();
  if (!/^https?:\/\/\S+\/join\/[A-Za-z0-9_-]{43}$/.test(link)) {
    throw new Error(`edge ${command} printed something other than one link`);
  }
  return link;
}

/** Runs an operator link command expected to fail, and returns its exit code and stderr. */
export async function failedOperatorLink(
  command: "invite" | "reset-link",
  email: string,
): Promise<{ code: number; stderr: string; stdout: string }> {
  try {
    await run(
      "cargo",
      ["run", "--quiet", "-p", "myelin-edge", "--bin", "edge", "--", command, "--tenant", requiredTenant(), "--email", email],
      { timeout: 120_000, maxBuffer: 256 * 1024 },
    );
  } catch (error) {
    const failure = error as { code?: number; stderr?: string; stdout?: string };
    return { code: failure.code ?? -1, stderr: failure.stderr ?? "", stdout: failure.stdout ?? "" };
  }
  throw new Error(`edge ${command} unexpectedly succeeded`);
}

function requiredTenant(): string {
  const tenant = process.env.MYELIN_SYSTEM_TEST_TENANT;
  if (!tenant) throw new Error("MYELIN_SYSTEM_TEST_TENANT is required");
  return tenant;
}
