/** Browser-owned identity for retrying one unchanged durable mutation. */
export function isClientNonce(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(value);
}
