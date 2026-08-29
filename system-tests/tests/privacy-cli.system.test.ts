import { randomUUID } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

import { describe, expect, test } from "vitest";

import { systemTestConfig } from "../src/config.js";
import { browserApprovedCliCredential, privacyClient } from "../src/context.js";
import { array, record, string } from "../src/json.js";
import { runCliWith } from "../src/myelin-cli.js";

describe("a person's privacy request from the CLI", () => {
  test("confirms one exact holder, retries safely, and reads its certificate", async () => {
    const credential = await browserApprovedCliCredential(privacyClient);
    const configDirectory = await mkdtemp(resolve(tmpdir(), "myelin-privacy-cli-"));
    const environment = {
      MYELIN_EDGE: systemTestConfig.edgeUrl,
      MYELIN_TOKEN: credential.token,
      MYELIN_TOKEN_SCHEME: credential.tokenScheme,
    };
    const runPrivacy = (...args: string[]) => runCliWith(
      configDirectory,
      { environment },
      ["privacy", ...args],
    );

    try {
      const unconfirmed = await runPrivacy("request", "erase", "git-pull-request-text");
      expect(unconfirmed.exitCode).toBe(2);
      expect(unconfirmed.stderr).toContain("privacy erasure is irreversible");

      const retryKey = `privacy-cli-${randomUUID()}`;
      const submitted = await runPrivacy(
        "--json",
        "request",
        "erase",
        "git-pull-request-text",
        "--confirm",
        "--idempotency-key",
        retryKey,
      );
      expect(submitted.exitCode, submitted.stderr).toBe(0);
      const submittedBody = record(JSON.parse(submitted.stdout), "CLI privacy submission");
      const request = record(submittedBody.request, "CLI privacy request");
      const requestId = string(request.id, "CLI privacy request id");
      expect(submittedBody.created).toBe(true);
      expect(request).toMatchObject({
        kind: "erasure",
        scope: "git_pull_request_text",
        state: "completed",
        certificate_available: true,
      });

      const replayed = await runPrivacy(
        "--json",
        "request",
        "erase",
        "git-pull-request-text",
        "--confirm",
        "--idempotency-key",
        retryKey,
      );
      expect(replayed.exitCode, replayed.stderr).toBe(0);
      expect(JSON.parse(replayed.stdout)).toMatchObject({
        created: false,
        request: { id: requestId, attempt_count: 1 },
      });

      const status = await runPrivacy("request", "status", requestId);
      expect(status.exitCode, status.stderr).toBe(0);
      expect(status.stdout).toContain(`Privacy erasure request ${requestId}: completed.`);
      expect(status.stdout).toContain(
        "Scope: pull-request titles and bodies you authored in Git.",
      );
      expect(status.stdout).toContain(`myelin privacy request certificate ${requestId}`);

      const certified = await runPrivacy(
        "--json",
        "request",
        "certificate",
        requestId,
      );
      expect(certified.exitCode, certified.stderr).toBe(0);
      const certificate = record(
        record(JSON.parse(certified.stdout), "CLI certificate envelope").certificate,
        "CLI privacy certificate",
      );
      expect(certificate).toMatchObject({
        request_id: requestId,
        kind: "erasure",
        scope: "git_pull_request_text",
      });
      expect(array(certificate.holders, "CLI certificate holders")).toEqual([
        expect.objectContaining({
          holder: "git_pull_request_text",
          operation: "erasure",
          key_unrecoverable: true,
        }),
      ]);

      const humanCertificate = await runPrivacy("request", "certificate", requestId);
      expect(humanCertificate.exitCode, humanCertificate.stderr).toBe(0);
      expect(humanCertificate.stdout).toContain(`Privacy erasure certificate ${requestId}.`);
      expect(humanCertificate.stdout).toContain("git_pull_request_text:");
      expect(humanCertificate.stdout).toContain("key unrecoverable");
    } finally {
      await rm(configDirectory, { recursive: true, force: true });
    }
  }, 60_000);
});
