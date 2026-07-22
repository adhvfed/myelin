import { describe, expect, it } from "vitest";

import {
  MAX_FILE_LINES_BLOB_BYTES,
  MAX_FILE_LINES_RANGE,
  parseFileLinesInput,
  parseFileLinesResponse,
} from "./file-lines";

const OID = "0123456789abcdef0123456789abcdef01234567";

describe("file-lines boundary codec", () => {
  it("admits an exact canonical request and projects context lines", () => {
    expect(parseFileLinesInput({
      repo: "team/core",
      oid: OID,
      path: "src/main file.rs",
      start: 2,
      end: 4,
    })).toEqual({
      repo: "team/core",
      oid: OID,
      path: "src/main file.rs",
      start: 2,
      end: 4,
    });
    expect(parseFileLinesResponse({
      lines: [{ origin: " ", content: "hello", old_no: null, new_no: 2, secret: "drop" }],
      internal: "drop",
    })).toEqual({
      lines: [{ origin: " ", content: "hello", old_no: null, new_no: 2 }],
    });
  });

  it.each([
    null,
    { repo: "core", oid: OID, path: "x", start: 1, end: 1, extra: true },
    { repo: "../core", oid: OID, path: "x", start: 1, end: 1 },
    { repo: "core", oid: OID.toUpperCase(), path: "x", start: 1, end: 1 },
    { repo: "core", oid: OID, path: "../secret", start: 1, end: 1 },
    { repo: "core", oid: OID, path: "x", start: 0, end: 1 },
    { repo: "core", oid: OID, path: "x", start: 2, end: 1 },
    { repo: "core", oid: OID, path: "x", start: 1, end: MAX_FILE_LINES_RANGE + 1 },
  ])("rejects malformed or unbounded request %#", (value) => {
    expect(parseFileLinesInput(value)).toBeNull();
  });

  it.each([
    null,
    { lines: "not-an-array" },
    { lines: Array.from({ length: MAX_FILE_LINES_RANGE + 1 }, () => ({})) },
    { lines: [{ origin: "+", content: "x", old_no: null, new_no: 1 }] },
    { lines: [{ origin: " ", content: "x", old_no: 1, new_no: 1 }] },
    { lines: [{ origin: " ", content: "x", old_no: null, new_no: 0 }] },
    { lines: [{ origin: " ", content: "x".repeat(MAX_FILE_LINES_BLOB_BYTES + 1), old_no: null, new_no: 1 }] },
  ])("rejects malformed or oversized response %#", (value) => {
    expect(parseFileLinesResponse(value)).toBeNull();
  });
});
