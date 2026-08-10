type WireRecord = Record<string, unknown>;

export type IssueListState = "open" | "closed" | "all";

export interface IssueListInput {
  state: IssueListState;
  key?: string;
  cursor?: string;
  limit?: number;
}

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(value).every((key) => allowed.has(key));
}

export function parseIssueListInput(value: unknown): IssueListInput | null {
  const input = record(value);
  if (!input || !exact(input, ["state", "key", "cursor", "limit"]) ||
      !["open", "closed", "all"].includes(input.state as string) ||
      (input.key !== undefined &&
        (typeof input.key !== "string" || !/^[A-Z0-9-]{1,32}$/.test(input.key))) ||
      (input.cursor !== undefined &&
        (typeof input.cursor !== "string" || !/^ic_[A-Za-z0-9_-]+$/.test(input.cursor) ||
          input.cursor.length > 192)) ||
      (input.limit !== undefined &&
        (!Number.isSafeInteger(input.limit) || (input.limit as number) < 1 ||
          (input.limit as number) > 100))) return null;
  return {
    state: input.state as IssueListState,
    ...(input.key === undefined ? {} : { key: input.key }),
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
    ...(input.limit === undefined ? {} : { limit: input.limit as number }),
  };
}

export function parseIssueId(value: unknown): string | null {
  return typeof value === "string" &&
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value)
    ? value
    : null;
}
