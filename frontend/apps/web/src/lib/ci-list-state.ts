import {
  parseCiRunsInput,
  type CiRunStateFilter,
  type CiRunsInput,
} from "./ci-read-input";

export const CI_WEB_PAGE_LIMIT = 25;

/** Convert router search values without repairing malformed or duplicate CI coordinates. */
export function ciRunsInputFromSearch(
  state: unknown,
  limit: unknown,
  cursor: unknown,
): CiRunsInput | null {
  const raw: Record<string, unknown> = {};
  if (state !== undefined) raw.state = state;
  raw.limit = limit === undefined
    ? CI_WEB_PAGE_LIMIT
    : typeof limit === "string" && /^(?:[1-9]|[1-9][0-9]|100)$/.test(limit)
      ? Number(limit)
      : limit;
  if (cursor !== undefined) raw.cursor = cursor;
  return parseCiRunsInput(raw);
}

export function ciRunsHref(input: {
  state?: CiRunStateFilter;
  limit?: number;
  cursor?: string;
}): string {
  const query = new URLSearchParams();
  if (input.state !== undefined && input.state !== "all") query.set("state", input.state);
  if (input.limit !== undefined && input.limit !== CI_WEB_PAGE_LIMIT) {
    query.set("limit", String(input.limit));
  }
  if (input.cursor !== undefined) query.set("cursor", input.cursor);
  const encoded = query.toString();
  return `/ci${encoded ? `?${encoded}` : ""}`;
}
