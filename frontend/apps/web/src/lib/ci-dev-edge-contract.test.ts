import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import {
  canonicalVisibleRepoRefs,
  ciLiveOpen,
  ciLogJson,
  ciRunJson,
  ciRunsEnvelope,
  parseCiLogQuery,
  parseCiRunsQuery,
} from "../../dev-edge/ci-contract.mjs";

// FRONTEND-CONTRACT: ci-read-dev-edge-parity
// The production Rust handler and this dev-edge consumer execute this exact committed artifact.
const CI_READ_GOLDEN_PATH = "contracts/ci-read-dev-edge.golden.json";
const golden = JSON.parse(readFileSync(
  new URL(`../../../../../${CI_READ_GOLDEN_PATH}`, import.meta.url),
  "utf8",
)) as {
  contract_id: string;
  vectors: Array<{
    id: string;
    endpoint: "visibility" | "runs" | "run" | "log" | "live";
    after?: string;
    mutation?: "add-visible-repo" | "prune-live-log";
    request: {
      state?: string;
      limit?: number;
      run_id?: string;
      job_id?: string;
      start?: number;
      last_event_id?: string;
      visible_repo_refs?: string[];
    };
    expected: Record<string, unknown>;
  }>;
};

describe("the shared CI read golden contract", () => {
  it("matches the production Edge request/response vectors", () => {
    expect(golden.contract_id).toBe("ci-read-dev-edge-parity");
    const cursors = new Map<string, string>();

    for (const vector of golden.vectors) {
      const cursor = vector.after ? cursors.get(vector.after) : undefined;
      if (vector.after && !cursor) throw new Error(`missing cursor from ${vector.after}`);
      const visibleRepoRefs = vector.request.visible_repo_refs;
      let normalized: Record<string, unknown>;

      if (vector.endpoint === "visibility") {
        normalized = {
          status: 200,
          visible_repo_refs: canonicalVisibleRepoRefs(visibleRepoRefs),
        };
      } else if (vector.endpoint === "runs") {
        const query = new URLSearchParams();
        if (vector.request.state !== undefined) query.set("state", vector.request.state);
        if (vector.request.limit !== undefined) query.set("limit", String(vector.request.limit));
        if (cursor !== undefined) query.set("cursor", cursor);
        const input = parseCiRunsQuery(query.toString());
        if (!input) throw new Error(`invalid golden list request ${vector.id}`);
        const response = ciRunsEnvelope(input, { visibleRepoRefs });
        if ("stale" in response) {
          normalized = { status: 409 };
        } else if ("bad" in response) {
          normalized = { status: 400 };
        } else {
          if (!("items" in response)) throw new Error(`invalid list response ${vector.id}`);
          if (!response.page) throw new Error(`missing list page ${vector.id}`);
          const page = response.page;
          const next = page.next_cursor;
          if (next) {
            expect(next).toMatch(/^cr1_[A-Za-z0-9_-]+$/);
            cursors.set(vector.id, next);
          }
          normalized = {
            status: 200,
            ...response,
            page: {
              ...page,
              next_cursor: next ? "cr1_<opaque>" : null,
            },
          };
        }
      } else if (vector.endpoint === "run") {
        const response = ciRunJson(vector.request.run_id);
        normalized = response ? { status: 200, ...response } : { status: 404 };
      } else if (vector.endpoint === "log") {
        const query = new URLSearchParams();
        if (vector.request.start !== undefined) query.set("start", String(vector.request.start));
        if (vector.request.limit !== undefined) query.set("limit", String(vector.request.limit));
        const input = parseCiLogQuery(query.toString());
        if (!input) throw new Error(`invalid golden log request ${vector.id}`);
        const response = ciLogJson(
          vector.request.run_id,
          vector.request.job_id,
          input,
        );
        normalized = response ? { status: 200, ...response } : { status: 404 };
      } else {
        const open = ciLiveOpen(
          vector.request.run_id,
          vector.request.job_id,
          vector.request.last_event_id,
          { ...(vector.mutation === "prune-live-log" ? { segments: [] } : {}) },
        );
        normalized = {
          status: open.status,
          ...("events" in open ? { events: open.events } : {}),
        };
      }

      expect(normalized, vector.id).toEqual(vector.expected);
    }
  });

  it("rejects duplicate, unknown, noncanonical, and out-of-bounds query coordinates", () => {
    for (const query of [
      "state=passed",
      "state=all&state=failed",
      "limit=01",
      "limit=0",
      "limit=101",
      "cursor=",
      "unknown=x",
      "state",
    ]) expect(parseCiRunsQuery(query), query).toBeNull();

    for (const query of [
      "start=-1",
      "start=00",
      "start=1&start=2",
      "limit=0",
      "limit=262145",
      "limit=01",
      "unknown=1",
      "start",
    ]) expect(parseCiLogQuery(query), query).toBeNull();
  });
});
