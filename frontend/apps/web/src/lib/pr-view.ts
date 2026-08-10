// Side-effect-free view-model mapping for PR list screens.
import { fmtDate } from "./format";
import type { PrListRowVM } from "./api";

/** Use the PR number when a legacy row has no title. */
export function prTitleText(row: Pick<PrListRowVM, "title" | "number">): string {
  return row.title ?? `#${row.number}`;
}

/** Whether the row is using the PR-number fallback. */
export function isTitleFallback(row: Pick<PrListRowVM, "title">): boolean {
  return row.title == null || row.title === "";
}

/** The deep link to a PR overview. On the cross-repo page the repo comes from the row
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

/** Whether filtering, rather than an empty collection, produced no rows. */
export function isFilteredNoResults(
  itemsLen: number,
  counts: Record<string, number> | undefined,
): boolean {
  const total = counts?.all ?? 0;
  return itemsLen === 0 && total > 0;
}

/** Count and truncation state for a cross-repo bucket. Prefer the server total, but never report
 * fewer rows than are present. A next cursor also marks the result as truncated. */
export function bucketPageSummary(page: {
  items: { length: number };
  page: { total?: number; next_cursor?: string | null };
}): { count: number; shown: number; truncated: boolean } {
  const shown = page.items.length;
  const total = page.page.total ?? shown;
  const count = Math.max(total, shown);
  const truncated = shown < count || (page.page.next_cursor != null);
  return { count, shown, truncated };
}
