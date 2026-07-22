import { describe, expect, it } from "vitest";

import {
  MAX_EXPANDED_CONTEXT_LINES,
  mapPrDiffContextLines,
  prDiffContextRange,
} from "./pr-diff-context";

const file = {
  hunks: [
    { old_start: 10, old_lines: 3, new_start: 10, new_lines: 4 },
    { old_start: 30, old_lines: 2, new_start: 31, new_lines: 2 },
  ],
};

describe("PR diff context gap mapping", () => {
  it("excludes both hunks and reconstructs the unchanged old-side offset", () => {
    expect(prDiffContextRange(file, "1")).toEqual({
      start: 14,
      end: 30,
      oldLineOffset: -1,
    });
    expect(mapPrDiffContextLines([
      { origin: " ", content: "context", old_no: null, new_no: 14 },
    ], { start: 14, end: 14, oldLineOffset: -1 })).toEqual([
      { origin: " ", content: "context", old_no: 13, new_no: 14 },
    ]);
  });

  it("maps the gap before the first hunk and rejects absent or unbounded gaps", () => {
    expect(prDiffContextRange(file, "0")).toEqual({ start: 1, end: 9, oldLineOffset: 0 });
    expect(prDiffContextRange(file, "01")).toBeNull();
    expect(prDiffContextRange(file, "2")).toBeNull();
    expect(prDiffContextRange({
      hunks: [{ old_start: 1, old_lines: 1, new_start: MAX_EXPANDED_CONTEXT_LINES + 2, new_lines: 1 }],
    }, "0")).toBeNull();
  });

  it("uses the following hunk's old/new offset and rejects adjacent or unsafe mappings", () => {
    expect(prDiffContextRange({
      hunks: [
        { old_start: 10, new_start: 10, new_lines: 2 },
        { old_start: 40, new_start: 42, new_lines: 1 },
      ],
    }, "1")).toEqual({ start: 12, end: 41, oldLineOffset: -2 });
    expect(prDiffContextRange({
      hunks: [
        { old_start: 1, new_start: 1, new_lines: 4 },
        { old_start: 5, new_start: 5, new_lines: 1 },
      ],
    }, "1")).toBeNull();
    expect(mapPrDiffContextLines([
      { origin: " ", content: "overflow", old_no: null, new_no: Number.MAX_SAFE_INTEGER },
    ], {
      start: Number.MAX_SAFE_INTEGER,
      end: Number.MAX_SAFE_INTEGER,
      oldLineOffset: 1,
    })).toBeNull();
    expect(mapPrDiffContextLines([
      { origin: " ", content: "wrong line", old_no: null, new_no: 8 },
    ], { start: 7, end: 7, oldLineOffset: 0 })).toBeNull();
    expect(mapPrDiffContextLines([], { start: 7, end: 7, oldLineOffset: 0 })).toBeNull();
  });
});
