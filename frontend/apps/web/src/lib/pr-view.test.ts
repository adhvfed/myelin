// PR list view-model mapping (Node/Vitest): title fallback, links, tabs, review markers, and filters.
import { describe, it, expect } from "vitest";
import {
  prTitleText,
  isTitleFallback,
  prHref,
  updatedLabel,
  stateTabs,
  reviewMarker,
  isFilteredNoResults,
  bucketPageSummary,
} from "./pr-view";

describe("pr-view mapping", () => {
  it("falls back to #number when a row has no stored title (never fabricates)", () => {
    expect(prTitleText({ title: "Real title", number: 48 })).toBe("Real title");
    expect(prTitleText({ title: null, number: 48 })).toBe("#48");
    expect(isTitleFallback({ title: null })).toBe(true);
    expect(isTitleFallback({ title: "" })).toBe(true);
    expect(isTitleFallback({ title: "x" })).toBe(false);
  });

  it("resolves the PR href from the row's repo (cross-repo) or the route (per-repo)", () => {
    // Cross-repo row carries its own repo.
    expect(prHref({ number: 46, repo: "myelin/myelin" }, undefined)).toBe(
      "/git/repos/myelin%2Fmyelin/prs/46",
    );
    // Per-repo row: repo comes from the route.
    expect(prHref({ number: 7, repo: null }, "core")).toBe("/git/repos/core/prs/7");
  });

  it("formats the updated column stably (UTC) and blanks an unknown timestamp", () => {
    expect(updatedLabel({ updated_at: 1719446400 })).toBe("2024-06-27 00:00 UTC");
    expect(updatedLabel({ updated_at: null })).toBe("");
  });

  it("derives the four tabs from the leak-free counts (0 when absent)", () => {
    const tabs = stateTabs({ open: 6, merged: 128, closed: 14, all: 148 });
    expect(tabs.map((t) => [t.key, t.count])).toEqual([
      ["open", 6],
      ["merged", 128],
      ["closed", 14],
      ["all", 148],
    ]);
    // Absent counts → 0, never undefined (no NaN badge).
    expect(stateTabs(undefined).every((t) => t.count === 0)).toBe(true);
  });

  it("shows the review marker only for the viewer's own requested review", () => {
    expect(reviewMarker({ you_are_requested: true, review_state: "requested", pr_state: "open" })).toBe(
      "review requested",
    );
    expect(reviewMarker({ you_are_requested: true, review_state: "requested", pr_state: "draft" })).toBe(
      "feedback requested",
    );
    // Not requested of the viewer → no marker (the plain review count renders instead).
    expect(reviewMarker({ you_are_requested: false, review_state: "approved", pr_state: "open" })).toBeNull();
  });

  it("distinguishes filtered-no-results from a truly-empty list", () => {
    // Empty result but the repo HAS PRs (all=148) → filtered-no-results.
    expect(isFilteredNoResults(0, { all: 148 })).toBe(true);
    // Empty result and no PRs at all → the teaching empty state, not filtered.
    expect(isFilteredNoResults(0, { all: 0 })).toBe(false);
    // Non-empty → neither.
    expect(isFilteredNoResults(3, { all: 148 })).toBe(false);
  });
});

describe("bucketPageSummary (#21a — honest count + truncation disclosure)", () => {
  const mk = (shown: number, page: { total?: number; next_cursor?: string | null }) => ({
    items: { length: shown },
    page,
  });

  it("the chip shows the TRUE total, not the page size", () => {
    const s = bucketPageSummary(mk(50, { total: 148 }));
    expect(s.count).toBe(148); // NOT 50 (the page size / silent-truncation lie)
    expect(s.shown).toBe(50);
    expect(s.truncated).toBe(true);
  });

  it("is NOT truncated when the whole set is shown (total === shown)", () => {
    const s = bucketPageSummary(mk(3, { total: 3 }));
    expect(s.count).toBe(3);
    expect(s.truncated).toBe(false);
  });

  it("falls back to the shown count when the server sends no total (nothing hidden)", () => {
    const s = bucketPageSummary(mk(7, {}));
    expect(s.count).toBe(7);
    expect(s.truncated).toBe(false);
  });

  it("treats a present next_cursor as truncation even if total is absent", () => {
    const s = bucketPageSummary(mk(50, { next_cursor: "c2" }));
    expect(s.truncated).toBe(true);
    expect(s.shown).toBe(50);
  });

  it("never reports fewer than actually shown (a stale/small total cannot hide rows)", () => {
    const s = bucketPageSummary(mk(5, { total: 2 }));
    expect(s.count).toBe(5); // max(total, shown)
    expect(s.truncated).toBe(false);
  });
});
