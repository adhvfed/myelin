import { describe, expect, it } from "vitest";

import {
  ciRunMutationErrorKind,
  ciRunMutationMessage,
  parseCiCancelReceipt,
  parseCiRetryReceipt,
  parseCiRunMutation,
  retainsCiRunKey,
} from "./ci-run-control";

const RUN = "91000000-0000-4000-8000-000000000001";
const NEW_RUN = "91000000-0000-4000-8000-000000000009";
const WORKFLOW = "94000000-0000-4000-8000-000000000001";

const retryReceipt = () => ({
  run_id: NEW_RUN,
  ref: `myelin://acme/ci/run/${NEW_RUN}`,
  source_run_id: RUN,
  workflow_run_id: WORKFLOW,
  event_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
  replayed: false,
});

const cancelReceipt = () => ({
  run_id: RUN,
  ref: `myelin://acme/ci/run/${RUN}`,
  workflow_run_id: WORKFLOW,
  state: "cancelled",
  event_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
  replayed: true,
});

describe("parseCiRunMutation", () => {
  it.each(["retry", "cancel"] as const)("admits an exact %s intent", (op) => {
    const input = { op, run: RUN, clientNonce: "intent-1_a" };
    expect(parseCiRunMutation(input)).toEqual(input);
  });

  it.each([
    null,
    [],
    "retry",
    { op: "retry", run: RUN },
    { op: "retry", clientNonce: "n1" },
    { run: RUN, clientNonce: "n1" },
    { op: "rerun", run: RUN, clientNonce: "n1" },
    { op: "delete", run: RUN, clientNonce: "n1" },
    { op: "retry", run: "not-a-uuid", clientNonce: "n1" },
    { op: "retry", run: "ABCDEF00-0000-4000-8000-000000000001", clientNonce: "n1" },
    { op: "retry", run: `${RUN}/x`, clientNonce: "n1" },
    { op: "cancel", run: RUN, clientNonce: "" },
    { op: "cancel", run: RUN, clientNonce: "has space" },
    { op: "cancel", run: RUN, clientNonce: "x".repeat(129) },
    { op: "cancel", run: RUN, clientNonce: "n1", extra: true },
  ])("rejects malformed input %#", (value) => {
    expect(parseCiRunMutation(value)).toBeNull();
  });
});

describe("parseCiRetryReceipt", () => {
  it("decodes the new run and the replay flag", () => {
    expect(parseCiRetryReceipt(retryReceipt(), RUN)).toEqual({
      run_id: NEW_RUN,
      source_run_id: RUN,
      workflow_run_id: WORKFLOW,
      replayed: false,
    });
    expect(parseCiRetryReceipt({ ...retryReceipt(), replayed: true }, RUN)?.replayed).toBe(true);
  });

  it.each([
    ["non-UUID new run", { run_id: "nope" }],
    ["non-UUID source", { source_run_id: "nope" }],
    ["non-UUID workflow run", { workflow_run_id: "nope" }],
    ["mismatched source run", { source_run_id: NEW_RUN }],
    ["the same run echoed back", { run_id: RUN, ref: `myelin://acme/ci/run/${RUN}` }],
    ["ref for another run", { ref: `myelin://acme/ci/run/${RUN}` }],
    ["ref in the wrong namespace", { ref: `myelin://acme/git/repo/${NEW_RUN}` }],
    ["empty event id", { event_id: "" }],
    ["spaced event id", { event_id: "a b" }],
    ["non-boolean replayed", { replayed: "false" }],
    ["unknown field", { state: "queued" }],
  ])("rejects %s", (_name, patch) => {
    expect(parseCiRetryReceipt({ ...retryReceipt(), ...patch }, RUN)).toBeNull();
  });

  it("rejects a missing field and non-objects", () => {
    const missing: Partial<ReturnType<typeof retryReceipt>> = retryReceipt();
    delete missing.event_id;
    expect(parseCiRetryReceipt(missing, RUN)).toBeNull();
    expect(parseCiRetryReceipt(null, RUN)).toBeNull();
    expect(parseCiRetryReceipt([], RUN)).toBeNull();
  });
});

describe("parseCiCancelReceipt", () => {
  it("decodes a cancelled run", () => {
    expect(parseCiCancelReceipt(cancelReceipt(), RUN)).toEqual({
      run_id: RUN,
      workflow_run_id: WORKFLOW,
      replayed: true,
    });
  });

  it.each([
    ["a state other than cancelled", { state: "running" }],
    ["a succeeded state", { state: "succeeded" }],
    ["non-UUID run", { run_id: "nope" }],
    ["mismatched run", { run_id: NEW_RUN, ref: `myelin://acme/ci/run/${NEW_RUN}` }],
    ["non-UUID workflow run", { workflow_run_id: "nope" }],
    ["ref for another run", { ref: `myelin://acme/ci/run/${NEW_RUN}` }],
    ["empty event id", { event_id: "" }],
    ["non-boolean replayed", { replayed: 1 }],
    ["unknown field", { source_run_id: RUN }],
  ])("rejects %s", (_name, patch) => {
    expect(parseCiCancelReceipt({ ...cancelReceipt(), ...patch }, RUN)).toBeNull();
  });

  it("rejects a missing state", () => {
    const missing: Partial<ReturnType<typeof cancelReceipt>> = cancelReceipt();
    delete missing.state;
    expect(parseCiCancelReceipt(missing, RUN)).toBeNull();
  });
});

describe("error mapping", () => {
  it.each([
    [400, "bad-input"],
    [403, "forbidden"],
    [404, "not-found"],
    [409, "conflict"],
    [500, "error"],
    [503, "error"],
    [502, "error"],
  ] as const)("maps HTTP %i to %s", (status, kind) => {
    expect(ciRunMutationErrorKind(status)).toBe(kind);
  });

  it("uses fixed copy per outcome", () => {
    expect(ciRunMutationMessage("cancel", "conflict")).toBe("This run already finished.");
    expect(ciRunMutationMessage("retry", "conflict")).toBe("This run can no longer be re-run.");
    expect(ciRunMutationMessage("retry", "forbidden")).toBe("You need push access to this repository.");
    expect(ciRunMutationMessage("cancel", "not-found")).toBe("This run is no longer available.");
    expect(ciRunMutationMessage("cancel", "error")).toMatch(/Try again/);
    expect(ciRunMutationMessage("retry", "bad-input")).toMatch(/Try again/);
  });

  it("keeps the idempotency key only when the Edge never decided", () => {
    expect(retainsCiRunKey("error")).toBe(true);
    for (const kind of ["bad-input", "forbidden", "not-found", "conflict"] as const) {
      expect(retainsCiRunKey(kind)).toBe(false);
    }
  });
});
