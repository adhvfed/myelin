// Tiny pure formatters shared by the Git browse screens. `fmtDate` renders a STABLE UTC string (not a
// locale string) so the SSR render and the client hydration agree (no hydration mismatch).

/** Unix seconds → a stable `YYYY-MM-DD HH:MM UTC` string (empty for a falsy/0 timestamp). */
export function fmtDate(unixSeconds: number): string {
  if (!unixSeconds) return "";
  return new Date(unixSeconds * 1000).toISOString().replace("T", " ").slice(0, 16) + " UTC";
}

/** The bare repo name (drop the leading `tenant/`) — the slug the edge routes key on. */
export function bareRepo(slug: string | undefined): string {
  if (!slug) return "";
  const parts = slug.split("/");
  return parts.length > 1 ? parts.slice(1).join("/") : slug;
}
