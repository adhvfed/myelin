import { describe, expect, it } from "vitest";

import { canonicalCliUserCode, cliApprovalPath } from "./cli-auth-core";

describe("CLI authorization codes", () => {
  it.each([
    ["ABCD-EFGH", "ABCD-EFGH"],
    ["abcd-efgh", "ABCD-EFGH"],
    [" ABCDEFGH ", "ABCD-EFGH"],
  ])("canonicalizes %j for a human to compare", (input, expected) => {
    expect(canonicalCliUserCode(input)).toBe(expected);
  });

  it.each(["ABCI-EFGH", "ABCO-EFGH", "ABC0-EFGH", "ABC1-EFGH", "ABC-DEFGH", "ABCD--EFGH"])(
    "rejects ambiguous or malformed code %j",
    (input) => expect(canonicalCliUserCode(input)).toBeNull(),
  );

  it("builds one bounded local consent URL", () => {
    expect(cliApprovalPath("ABCD-EFGH", "approved")).toBe(
      "/cli/auth?code=ABCD-EFGH&result=approved",
    );
  });
});
