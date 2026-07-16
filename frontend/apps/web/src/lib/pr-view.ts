// Pure view-model mapping for the PR list screens (R3.1). Kept side-effect-free + DOM-free so it runs
// under the node vitest harness (the a11y/render layer is the Playwright + design-system jsdom gates).
// The rules here are the honest ones: a missing title becomes `#number` (never fabricated), and the
// row's href resolves the repo from the row (cross-repo) or the route (per-repo).
import { fmtDate } from "./format";
import type { PrListRowVM } from "./api";

/** The row's display title. A legacy record has `title === null` → render `#number` (honest, never a
 *  fabricated title). When a title exists the `#number` is shown separately by the row, so this is the
 *  TITLE text only. */
export function prTitleText(row: Pick<PrListRowVM, "title" | "number">): string {
  return row.title ?? `#${row.number}`;
}

/** Whether the row is showing the honest `#number` fallback (no stored title). */
export function isTitleFallback(row: Pick<PrListRowVM, "title">): boolean {
  return row.title == null || row.title === "";
}

/** The deep link to a PR overview. On the cross-repo front door the repo comes from the ROW
 *  (`row.repo`); on a per-repo list it comes from the route (`routeRepo`). */
export function prHref(
  row: Pick<PrListRowVM, "number" | "repo">,
  routeRepo?: string,
): string {
  const repo = row.repo ?? routeRepo ?? "";
  return `/git/repos/${encodeURIComponent(repo)}/prs/${row.number}`;
}

/** The "updated" column — unix seconds → a stable UTC string (hydration-safe; a relative "3h ago"
 *  needs a fixed clock to avoid an SSR/client mismatch — a named follow-on). Empty when unknown. */
export function updatedLabel(row: Pick<PrListRowVM, "updated_at">): string {
  return row.updated_at ? fmtDate(row.updated_at) : "";
}

/** One state-tab descriptor for the filter tablist (Open/Merged/Closed/All). The count is read from
 *  the edge `counts` (computed over the leak-free set — a forbidden PR never inflates a badge). */
export interface StateTab {
  key: "open" | "merged" | "closed" | "all";
  label: string;
  count: number;
}

/** The four filter tabs, counts sourced from the edge envelope (0 when absent). */
export function stateTabs(counts: Record<string, number> | undefined): StateTab[] {
  const c = counts ?? {};
  return [
    { key: "open", label: "Open", count: c.open ?? 0 },
    { key: "merged", label: "Merged", count: c.merged ?? 0 },
    { key: "closed", label: "Closed", count: c.closed ?? 0 },
    { key: "all", label: "All", count: c.all ?? 0 },
  ];
}

/** The quiet review marker text for a row (the row's right cluster). `null` = show the plain review
 *  count instead. "review requested" only when the VIEWER is a requested reviewer (never leaks another
 *  reviewer's request — that predicate is resolved server-side per viewer). */
export function reviewMarker(
  row: Pick<PrListRowVM, "you_are_requested" | "review_state" | "pr_state">,
): string | null {
  if (row.you_are_requested) {
    return row.pr_state === "draft" ? "feedback requested" : "review requested";
  }
  return null;
}

/** Whether a bucket/tab yielded no rows because of a FILTER (distinct from a truly-empty list) — the
 *  screen shows "no results" (offer Clear filters), never the teaching empty state. */
export function isFilteredNoResults(
  itemsLen: number,
  counts: Record<string, number> | undefined,
): boolean {
  const total = counts?.all ?? 0;
  return itemsLen === 0 && total > 0;
}
