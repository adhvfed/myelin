import { describe, expect, it } from "vitest";

import {
  ciLogSearchParams,
  ciRunsSearchParams,
  parseCiLogInput,
  parseCiRunId,
  parseCiRunsInput,
} from "./ci-read-input";

const RUN = "91000000-0000-4000-8000-000000000001";
const JOB = "92000000-0000-4000-8000-000000000001";

function cursor(): string {
  const frame = new Uint8Array(60);
  frame[0] = 1;
  frame.set(new TextEncoder().encode("2026-07-24T12:00:00.000000Z"), 1);
  frame.set(Uint8Array.from({ length: 16 }, (_, index) => index), 28);
  frame.set(Uint8Array.from({ length: 16 }, (_, index) => 255 - index), 44);
  return `cr1_${Buffer.from(frame).toString("base64url")}`;
}

describe("CI read RPC inputs", () => {
  it("admits and encodes only canonical bounded coordinates", () => {
    const opaque = cursor();
    expect(parseCiRunsInput({ state: "failed", limit: 25, cursor: opaque }))
      .toEqual({ state: "failed", limit: 25, cursor: opaque });
    expect(ciRunsSearchParams({ state: "failed", limit: 25, cursor: opaque }).toString())
      .toBe(`state=failed&limit=25&cursor=${opaque}`);
    expect(parseCiRunId(RUN)).toBe(RUN);
    expect(parseCiLogInput({ run: RUN, job: JOB, start: 64, limit: 65_536 }))
      .toEqual({ run: RUN, job: JOB, start: 64, limit: 65_536 });
    expect(ciLogSearchParams({ run: RUN, job: JOB, start: 64, limit: 65_536 }).toString())
      .toBe("start=64&limit=65536");
  });

  it.each([
    { state: "passed" },
    { limit: 0 },
    { limit: 101 },
    { limit: 1.5 },
    { cursor: "cr1_1" },
    { cursor: `cr1_${Buffer.concat([Buffer.from([2]), Buffer.alloc(59, 1)]).toString("base64url")}` },
    { surprise: true },
  ])("rejects malformed run-list input %#", (value) => {
    expect(parseCiRunsInput(value)).toBeNull();
  });

  it.each([
    { run: `g${RUN.slice(1)}`, job: JOB },
    { run: RUN, job: "not-a-job" },
    { run: RUN, job: JOB, start: -1 },
    { run: RUN, job: JOB, start: 1.5 },
    { run: RUN, job: JOB, limit: 0 },
    { run: RUN, job: JOB, limit: 262_145 },
    { run: RUN, job: JOB, extra: true },
  ])("rejects malformed log coordinates %#", (value) => {
    expect(parseCiLogInput(value)).toBeNull();
  });
});
