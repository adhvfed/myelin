// Browser-facing contract for re-running and cancelling a CI run: bounded server-action input,
// strict receipt decoders, and the fixed error copy. No Edge-provided text ever reaches the page.

import { isCiUuid } from "./ci-read-input";
import { isClientNonce } from "./client-nonce";

export type CiRunOp = "retry" | "cancel";
export interface CiRunMutation {
  op: CiRunOp;
  run: string;
  clientNonce: string;
}

export interface CiRetryReceipt {
  run_id: string;
  source_run_id: string;
  workflow_run_id: string;
  replayed: boolean;
}

export interface CiCancelReceipt {
  run_id: string;
  workflow_run_id: string;
  replayed: boolean;
}

export type CiRunMutationError = "bad-input" | "forbidden" | "not-found" | "conflict" | "error";
export type CiRunMutationResult =
  | { ok: true; op: "retry"; receipt: CiRetryReceipt }
  | { ok: true; op: "cancel"; receipt: CiCancelReceipt }
  | { ok: false; error: CiRunMutationError };

type WireRecord = Record<string, unknown>;

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const actual = Object.keys(value);
  return actual.length === keys.length && keys.every((key) => Object.hasOwn(value, key));
}

export function parseCiRunMutation(value: unknown): CiRunMutation | null {
  const input = record(value);
  if (!input || !exact(input, ["op", "run", "clientNonce"]) ||
      (input.op !== "retry" && input.op !== "cancel") ||
      !isCiUuid(input.run) || !isClientNonce(input.clientNonce)) return null;
  return { op: input.op, run: input.run, clientNonce: input.clientNonce };
}

function validRef(value: unknown, runId: unknown): boolean {
  return typeof value === "string" && typeof runId === "string" &&
    /^myelin:\/\/[^/\s]+\/ci\/run\/[0-9a-f-]{36}$/.test(value) && value.endsWith(`/ci/run/${runId}`);
}

function validEventId(value: unknown): boolean {
  return typeof value === "string" && value.length > 0 && value.length <= 128 &&
    /^[\x21-\x7e]+$/.test(value);
}

/** `run_id` is the NEW run; it must differ from the run the person asked to re-run. */
export function parseCiRetryReceipt(value: unknown, sourceRun: string): CiRetryReceipt | null {
  const receipt = record(value);
  if (!receipt || !exact(receipt, [
    "run_id", "ref", "source_run_id", "workflow_run_id", "event_id", "replayed",
  ]) || !isCiUuid(receipt.run_id) || !validRef(receipt.ref, receipt.run_id) ||
      !isCiUuid(receipt.source_run_id) || receipt.source_run_id !== sourceRun ||
      receipt.run_id === sourceRun || !isCiUuid(receipt.workflow_run_id) ||
      !validEventId(receipt.event_id) || typeof receipt.replayed !== "boolean") return null;
  return {
    run_id: receipt.run_id,
    source_run_id: receipt.source_run_id,
    workflow_run_id: receipt.workflow_run_id,
    replayed: receipt.replayed,
  };
}

export function parseCiCancelReceipt(value: unknown, run: string): CiCancelReceipt | null {
  const receipt = record(value);
  if (!receipt || !exact(receipt, [
    "run_id", "ref", "workflow_run_id", "state", "event_id", "replayed",
  ]) || !isCiUuid(receipt.run_id) || receipt.run_id !== run ||
      !validRef(receipt.ref, receipt.run_id) || !isCiUuid(receipt.workflow_run_id) ||
      receipt.state !== "cancelled" || !validEventId(receipt.event_id) ||
      typeof receipt.replayed !== "boolean") return null;
  return {
    run_id: receipt.run_id,
    workflow_run_id: receipt.workflow_run_id,
    replayed: receipt.replayed,
  };
}

export function ciRunMutationErrorKind(status: number): CiRunMutationError {
  if (status === 400) return "bad-input";
  if (status === 403) return "forbidden";
  if (status === 404) return "not-found";
  if (status === 409) return "conflict";
  return "error";
}

export function ciRunMutationMessage(op: CiRunOp, error: CiRunMutationError): string {
  if (error === "conflict") {
    return op === "cancel" ? "This run already finished." : "This run can no longer be re-run.";
  }
  if (error === "forbidden") return "You need push access to this repository.";
  if (error === "not-found") return "This run is no longer available.";
  return op === "cancel"
    ? "Couldn't cancel this run. Try again."
    : "Couldn't re-run this run. Try again.";
}

/** Only an outcome the Edge never decided keeps the same idempotency key for the next attempt. */
export function retainsCiRunKey(error: CiRunMutationError): boolean {
  return error === "error";
}
