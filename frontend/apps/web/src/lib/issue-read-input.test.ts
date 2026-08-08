import { describe, expect, it } from "vitest";

import { parseIssueId, parseIssueListInput } from "./issue-read-input";

const UUID = "123e4567-e89b-12d3-a456-426614174000";

describe("Issue read RPC inputs", () => {
  it("admits exact bounded list and detail coordinates", () => {
    expect(parseIssueListInput({ state: "closed", key: "MYL-", cursor: "ic_0123abcd", limit: 50 }))
      .toEqual({ state: "closed", key: "MYL-", cursor: "ic_0123abcd", limit: 50 });
    expect(parseIssueListInput({ state: "open" })).toEqual({ state: "open" });
    expect(parseIssueId(UUID)).toBe(UUID);
  });

  it.each([
    () => parseIssueListInput({ state: "deleted" }),
    () => parseIssueListInput({ state: "open", key: "lower" }),
    () => parseIssueListInput({ state: "open", key: "A".repeat(33) }),
    () => parseIssueListInput({ state: "open", cursor: "opaque" }),
    () => parseIssueListInput({ state: "open", cursor: "ic_x\nsmuggled" }),
    () => parseIssueListInput({ state: "open", cursor: `ic_${"a".repeat(190)}` }),
    () => parseIssueListInput({ state: "open", limit: 0 }),
    () => parseIssueListInput({ state: "open", surprise: true }),
    () => parseIssueId(UUID.toUpperCase()),
    () => parseIssueId("not-an-id"),
  ])("rejects malformed, unsafe, or surplus input", (parse) => {
    expect(parse()).toBeNull();
  });
});
