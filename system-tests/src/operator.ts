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
