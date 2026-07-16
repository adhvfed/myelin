// PR list view-model mapping (R3.1) — pure, DOM-free (node vitest). Proves the honest mappings: the
// `#number` title fallback, the leak-free tab counts, the per-viewer review marker, and the
// filtered-no-results vs truly-empty distinction.
import { describe, it, expect } from "vitest";
import {
  prTitleText,
  isTitleFallback,
  prHref,
  updatedLabel,
  stateTabs,
  reviewMarker,
  isFilteredNoResults,
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
