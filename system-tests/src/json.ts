export type JsonRecord = Record<string, unknown>;

export function record(value: unknown, context = "value"): JsonRecord {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`${context} must be a JSON object`);
  }
  return value as JsonRecord;
}

export function array(value: unknown, context = "value"): unknown[] {
  if (!Array.isArray(value)) throw new TypeError(`${context} must be a JSON array`);
  return value;
}

export function string(value: unknown, context = "value"): string {
  if (typeof value !== "string") throw new TypeError(`${context} must be a string`);
  return value;
}

export function boolean(value: unknown, context = "value"): boolean {
  if (typeof value !== "boolean") throw new TypeError(`${context} must be a boolean`);
  return value;
}

export function integer(value: unknown, context = "value"): number {
  if (!Number.isSafeInteger(value)) throw new TypeError(`${context} must be an integer`);
  return value as number;
}
