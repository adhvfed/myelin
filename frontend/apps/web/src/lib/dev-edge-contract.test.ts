import { describe, expect, it } from "vitest";

import { validPrOperationId } from "../../dev-edge/dev-contract.mjs";

describe("the dev Edge production-write contract", () => {
  it.each([
    "merge-operation-1",
    "550e8400-e29b-41d4-a716-446655440000",
    ` ${"x".repeat(128)} `,
  ])("accepts a production-valid PR operation id: %s", (value) => {
    expect(validPrOperationId(value)).toBe(true);
  });

  it.each([
    undefined,
    ["duplicate", "headers"],
    "",
    "   ",
    "contains space",
    "contains\nnewline",
    "ø",
    "x".repeat(129),
  ])("rejects a missing, ambiguous, or malformed PR operation id %#", (value) => {
    expect(validPrOperationId(value)).toBe(false);
  });
});
