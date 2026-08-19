import { array, record, string, type JsonRecord } from "./json.js";
import type { SystemTestClient } from "./client.js";

/// Walks a cursor-paged collection to exhaustion, guarding against the two
/// classic pagination lies: a repeated cursor (infinite loop) and an unbounded
/// walk. Every paged surface in the product speaks this envelope shape
/// (`items` + `page.next_cursor`), so tests should walk pages through here
/// instead of hand-rolling the loop.
export async function* walkPaged(
  client: SystemTestClient,
  path: string,
  options: { limit?: number; maxPages?: number } = {},
): AsyncGenerator<JsonRecord, void, void> {
  const limit = options.limit ?? 100;
  const maxPages = options.maxPages ?? 100;
  const visited = new Set<string>();
  let cursor: string | undefined;
  for (let page = 0; page < maxPages; page += 1) {
    const query = new URLSearchParams({ limit: String(limit) });
    if (cursor) query.set("cursor", cursor);
    const separator = path.includes("?") ? "&" : "?";
    const response = await client.json(`${path}${separator}${query}`);
    for (const item of array(response.body.items, `${path} page items`)) {
      yield record(item, `${path} page item`);
    }
    const next = record(response.body.page, `${path} page envelope`).next_cursor;
    if (next === null) return;
    cursor = string(next, `${path} next cursor`);
    if (visited.has(cursor)) throw new Error(`${path} repeated its pagination cursor`);
    visited.add(cursor);
  }
  throw new Error(`${path} exceeded ${maxPages} pages without exhausting the collection`);
}

/// First page item matching the predicate, or undefined after a full walk.
export async function findPaged(
  client: SystemTestClient,
  path: string,
  predicate: (item: JsonRecord) => boolean,
  options: { limit?: number; maxPages?: number } = {},
): Promise<JsonRecord | undefined> {
  for await (const item of walkPaged(client, path, options)) {
    if (predicate(item)) return item;
  }
  return undefined;
}
