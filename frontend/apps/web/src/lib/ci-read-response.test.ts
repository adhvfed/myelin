import { describe, expect, it } from "vitest";

import {
  ciJobResultLabel,
  parseCiLogRange,
  parseCiRunDetail,
  parseCiRunsPage,
} from "./ci-read-response";

const RUN = "91000000-0000-4000-8000-000000000001";
const PIPELINE = "93000000-0000-4000-8000-000000000001";
const JOB = "92000000-0000-4000-8000-000000000001";

function cursor(): string {
  const frame = Buffer.alloc(60);
  frame[0] = 1;
  frame.write("2026-07-24T12:00:00.000000Z", 1, "ascii");
  return `cr1_${frame.toString("base64url")}`;
}

function run() {
  return {
    run_id: RUN,
    ref: `myelin://acme/ci/run/${RUN}`,
    pipeline_id: PIPELINE,
    repo_ref: "myelin://acme/git/repo/alpha",
    source_ref: "refs/heads/main",
    commit_oid: "0123456789abcdef",
    trigger_kind: "push",
    trust_tier: "trusted",
    state: "failed",
    cost_settled: true,
    created_at: "2026-07-24T12:00:00.000000Z",
    finished_at: "2026-07-24T12:05:00.000000Z",
  };
}

describe("CI read responses", () => {
  it("strictly decodes list/detail facts and exact step anchors", () => {
    expect(parseCiRunsPage({
      items: [run()],
      page: { next_cursor: cursor(), limit: 1 },
    })?.items).toEqual([run()]);
    expect(parseCiRunsPage({
      items: [{ ...run(), trigger_kind: "pull_request", source_ref: null }],
      page: { next_cursor: null, limit: 1 },
    })?.items).toHaveLength(1);

    const detail = parseCiRunDetail({
      run: run(),
      jobs: [{
        job_id: JOB,
        stage: "test",
        name: "contract",
        needs: [],
        matrix_key: null,
        state: "failed",
        attempt: 1,
        result_summary: {
          passed: false,
          timed_out: false,
          disposition: "workload_failed",
          workload_started: true,
          diagnostic: "Process exited with status 1.",
        },
      }],
      steps: [{
        job_id: JOB,
        step_id: "contract",
        byte_start: 0,
        byte_end: 18,
        status: "failed",
        details_ref: "#step-contract",
      }],
    });
    expect(detail?.steps[0]?.details_ref).toBe("#step-contract");
    expect(detail?.jobs[0]?.result_summary).toEqual({
      passed: false,
      timed_out: false,
      disposition: "workload_failed",
      workload_started: true,
      diagnostic: "Process exited with status 1.",
    });
    expect(ciJobResultLabel(detail!.jobs[0]!.result_summary!)).toBe("Workload failed");
  });

  it("normalizes legacy summaries and refuses unknown or contradictory modern results", () => {
    const detailWith = (result_summary: unknown) => parseCiRunDetail({
      run: run(),
      jobs: [{
        job_id: JOB,
        stage: "test",
        name: "contract",
        needs: [],
        matrix_key: null,
        state: "failed",
        attempt: 1,
        result_summary,
      }],
      steps: [],
    });

    expect(detailWith({ passed: false, timed_out: false })?.jobs[0]?.result_summary).toEqual({
      passed: false,
      timed_out: false,
      disposition: null,
      workload_started: null,
      diagnostic: null,
    });
    expect(detailWith({ passed: true, timed_out: true })).toBeNull();
    expect(detailWith({ message: "contract failed" })).toBeNull();
    expect(detailWith({
      passed: false,
      timed_out: false,
      disposition: "workload_failed",
      workload_started: false,
    })).toBeNull();
    expect(detailWith({
      passed: false,
      timed_out: false,
      disposition: "configuration_refused",
      workload_started: false,
      diagnostic: "x".repeat(2_049),
    })).toBeNull();
    expect(detailWith({
      passed: false,
      timed_out: false,
      disposition: "configuration_refused",
      workload_started: false,
      diagnostic: "first\u{0085}second",
    })).toBeNull();
    expect(detailWith({
      passed: false,
      timed_out: false,
      disposition: "configuration_refused",
      workload_started: false,
      diagnostic: "first\u{2028}second",
    })).toBeNull();
  });

  it("preserves byte-exact ranges without pretending arbitrary boundaries are text", () => {
    const parsed = parseCiLogRange({
      run_id: RUN,
      job_id: JOB,
      byte_start: 9,
      byte_end: 16,
      total_end: 18,
      next_offset: 16,
      encoding: "base64",
      data: "qQpmYWlsZQ==",
    });
    expect(parsed).toMatchObject({
      byte_start: 9,
      byte_end: 16,
      next_offset: 16,
    });
    expect(parsed?.data).toBe("qQpmYWlsZQ==");
  });

  it("accepts the production beyond-end empty range without inventing a continuation", () => {
    expect(parseCiLogRange({
      run_id: RUN,
      job_id: JOB,
      byte_start: 100,
      byte_end: 100,
      total_end: 18,
      next_offset: null,
      encoding: "base64",
      data: "",
    })?.data).toBe("");
  });

  it.each([
    { items: [run()], page: { next_cursor: "opaque", limit: 1 } },
    { items: [run(), run()], page: { next_cursor: null, limit: 1 } },
    { items: [{ ...run(), internal: true }], page: { next_cursor: null, limit: 1 } },
    { items: [{ ...run(), state: "leased" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ ...run(), source_ref: "refs/tags/release" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ ...run(), trigger_kind: "pull_request" }], page: { next_cursor: null, limit: 1 } },
  ])("rejects malformed list payload %#", (value) => {
    expect(parseCiRunsPage(value)).toBeNull();
  });

  it("rejects orphaned or forged step anchors", () => {
    const base = {
      run: run(),
      jobs: [{
        job_id: JOB,
        stage: "test",
        name: "contract",
        needs: [],
        matrix_key: null,
        state: "failed",
        attempt: 1,
        result_summary: null,
      }],
    };
    expect(parseCiRunDetail({
      ...base,
      steps: [{
        job_id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
        step_id: "contract",
        byte_start: 0,
        byte_end: 1,
        status: "failed",
        details_ref: "#step-contract",
      }],
    })).toBeNull();
    expect(parseCiRunDetail({
      ...base,
      steps: [{
        job_id: JOB,
        step_id: "contract",
        byte_start: 0,
        byte_end: 1,
        status: "failed",
        details_ref: "#step-other",
      }],
    })).toBeNull();
  });

  it("rejects a missing opaque or nullable field instead of treating it as optional", () => {
    const missingResult = {
      run: run(),
      jobs: [{
        job_id: JOB,
        stage: "test",
        name: "contract",
        needs: [],
        matrix_key: null,
        state: "failed",
        attempt: 1,
      }],
      steps: [],
    };
    expect(parseCiRunDetail(missingResult)).toBeNull();
  });

  it("binds detail/log responses to the request and rejects a malformed job DAG", () => {
    const base = {
      run: run(),
      jobs: [{
        job_id: JOB,
        stage: "test",
        name: "contract",
        needs: ["aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"],
        matrix_key: null,
        state: "failed",
        attempt: 1,
        result_summary: null,
      }],
      steps: [],
    };
    expect(parseCiRunDetail(base)).toBeNull();
    expect(parseCiRunDetail({ ...base, jobs: [
      { ...base.jobs[0], needs: [] },
      { ...base.jobs[0], needs: [] },
    ] })).toBeNull();
    const other = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    expect(parseCiRunDetail({ ...base, jobs: [
      { ...base.jobs[0], needs: [other] },
      { ...base.jobs[0], job_id: other, needs: [JOB] },
    ] })).toBeNull();
    expect(parseCiRunDetail({ ...base, jobs: [{ ...base.jobs[0], needs: [] }] },
      "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")).toBeNull();

    expect(parseCiLogRange({
      run_id: RUN,
      job_id: JOB,
      byte_start: 0,
      byte_end: 1,
      total_end: 1,
      next_offset: null,
      encoding: "base64",
      data: "YQ==",
    }, { run: RUN, job: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa" })).toBeNull();
  });

  it.each([
    { byte_start: 0, byte_end: 2, total_end: 2, next_offset: null, data: "YQ==" },
    { byte_start: 0, byte_end: 1, total_end: 2, next_offset: null, data: "YQ==" },
    { byte_start: 0, byte_end: 2, total_end: 2, next_offset: 2, data: "YWI=" },
    { byte_start: 3, byte_end: 3, total_end: 2, next_offset: 3, data: "" },
  ])("rejects contradictory archived-range payload %#", (patch) => {
    expect(parseCiLogRange({
      run_id: RUN,
      job_id: JOB,
      encoding: "base64",
      ...patch,
    })).toBeNull();
  });
});
