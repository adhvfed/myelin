import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { browserApprovedCliClient, uniqueName } from "../src/context.js";
import {
  activateExternalAgent,
  askAgent,
  askAgentToAct,
  askAgentToBeDenied,
  beginAgentThreadRun,
  closeAgentRun,
} from "../src/journeys/agents.js";
import { startPrivateAgentThread } from "../src/journeys/agent-threads.js";
import { record } from "../src/json.js";

describe("commands in a private agent workspace", () => {
  test("retires runaway work and leaves the durable workspace useful", async () => {
    const founder = await browserApprovedCliClient();
    const agent = await activateExternalAgent(
      founder,
      uniqueName("Bounded workspace agent"),
      ["workspace.exec", "workspace.read_file"],
    );
    const { thread } = await startPrivateAgentThread(founder, {
      name: uniqueName("Keep experimental commands contained"),
      agentId: agent.agent.id,
      retentionDays: 1,
      idempotencyKey: `bounded-command-thread-${randomUUID()}`,
    });
    const run = await beginAgentThreadRun(founder, thread.id);

    try {
      const timedOut = record(
        (
          await askAgentToAct(
            run,
            1,
            "workspace.exec",
            {
              command: "printf 'started\\n'; sleep 10; printf 'too late\\n'",
              timeout_seconds: 1,
            },
            `timed-workspace-command-${randomUUID()}`,
          )
        ).data,
        "timed-out command result",
      );
      expect(timedOut).toMatchObject({
        stdout: "started\n",
        stderr: "",
        timed_out: true,
        cancelled: false,
        output_limit_exceeded: false,
        workspace_generation: thread.workspace.generation,
      });
      expect(timedOut.exit_code).not.toBe(0);
      expect(Number(timedOut.elapsed_ms)).toBeLessThan(10_000);

      const tooLoud = record(
        (
          await askAgentToAct(
            run,
            2,
            "workspace.exec",
            {
              command: "chunk=0123456789abcdef; while :; do printf '%s' \"$chunk\"; done",
              timeout_seconds: 10,
            },
            `loud-workspace-command-${randomUUID()}`,
          )
        ).data,
        "output-bounded command result",
      );
      expect(tooLoud).toMatchObject({
        stderr: "",
        timed_out: false,
        cancelled: false,
        output_limit_exceeded: true,
        workspace_generation: thread.workspace.generation,
      });
      expect(tooLoud.exit_code).not.toBe(0);
      expect(Buffer.byteLength(String(tooLoud.stdout))).toBe(32 * 1024);
      expect(Number(tooLoud.output_bytes)).toBe(32 * 1024);

      const recoveryPath = "notes/after-runaway.txt";
      const recoveryKey = `recovered-workspace-command-${randomUUID()}`;
      const recovered = record(
        (
          await askAgentToAct(
            run,
            3,
            "workspace.exec",
            {
              command: `mkdir -p notes && printf 'workspace ready\\n' > ${recoveryPath} && printf 'ready\\n'`,
              timeout_seconds: 10,
            },
            recoveryKey,
          )
        ).data,
        "recovered command result",
      );
      expect(recovered).toMatchObject({
        exit_code: 0,
        stdout: "ready\n",
        stderr: "",
        timed_out: false,
        cancelled: false,
        output_limit_exceeded: false,
      });

      const conflictingRetry = await askAgentToBeDenied(
        run,
        4,
        "workspace.exec",
        { command: `printf 'wrong retry\\n' > ${recoveryPath}`, timeout_seconds: 10 },
        recoveryKey,
      );
      expect(conflictingRetry).toContain("idempotency key was already used");

      expect(await askAgent(run, 5, "workspace.read_file", { path: recoveryPath })).toMatchObject({
        content: "workspace ready\n",
        workspace_generation: thread.workspace.generation,
      });
    } finally {
      await closeAgentRun(run);
    }
  }, 120_000);
});
