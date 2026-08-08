import { describe, expect, it } from "vitest";

import { parseCommitDiff, parseCommitsPage, parsePrCommitsPage } from "./commit-read-response";

const OID = "0123456789abcdef0123456789abcdef01234567";
const OTHER_OID = "1123456789abcdef0123456789abcdef01234567";

function prCommitCursor(position = 1): string {
  const frame = new Uint8Array(78);
  frame[0] = 1;
  frame[33] = 0;
  frame.set(Uint8Array.from(OID.match(/../g)!, (byte) => Number.parseInt(byte, 16)), 54);
  new DataView(frame.buffer).setUint32(74, position, false);
  return `pc1_${Buffer.from(frame).toString("base64url")}`;
}

function row(oid = OID) {
  return {
    oid,
    short_oid: oid.slice(0, 12),
    summary: "ship",
    author: "u",
    committed_at: 1,
    parents: [],
  };
}

describe("commit read response projection", () => {
  it("projects commit pages and diffs recursively", () => {
    expect(parseCommitsPage({
      items: [{ oid: OID, short_oid: OID.slice(0, 12), summary: "ship", author: "u", committed_at: 1, parents: [], secret: "drop" }],
      page: { next_cursor: null, prev_cursor: "0", limit: 50, offset: 1, range: { from: 2, to: 2 }, total: 9 },
    })?.items[0]).toEqual({
      oid: OID, short_oid: OID.slice(0, 12), summary: "ship", author: "u", committed_at: 1, parents: [],
    });
    expect(parseCommitDiff({
      oid: OID, short_oid: OID.slice(0, 12), summary: "ship", message: "ship", author: "u",
      committed_at: 1, parents: [],
      files: [{ path: "x", old_path: null, status: "A", lines: [{ origin: "+", content: "x", secret: "drop" }], secret: "drop" }],
      secret: "drop",
    })?.files[0]).toEqual({
      path: "x", old_path: null, status: "A", lines: [{ origin: "+", content: "x" }],
    });
  });

  it("accepts only the exact bounded PR commit envelope and canonical continuation", () => {
    const cursor = prCommitCursor(20);
    expect(parsePrCommitsPage({
      items: [row(), row(OTHER_OID)],
      page: { next_cursor: cursor, limit: 2 },
    })).toEqual({
      items: [row(), row(OTHER_OID)],
      page: { next_cursor: cursor, limit: 2 },
    });

    const wrongVersionFrame = Buffer.from(cursor.slice(4), "base64url");
    wrongVersionFrame[0] = 2;
    const zeroPositionFrame = Buffer.from(cursor.slice(4), "base64url");
    zeroPositionFrame.fill(0, 74, 78);
    const invalidBaseSentinelFrame = Buffer.from(cursor.slice(4), "base64url");
    invalidBaseSentinelFrame[34] = 1;
    for (const value of [
      { items: [row(), row(OTHER_OID)], page: { next_cursor: null, limit: 1 } },
      { items: [row(), row()], page: { next_cursor: null, limit: 2 } },
      { items: [row()], page: { next_cursor: null, limit: 1 }, secret: true },
      { items: [{ ...row(), secret: true }], page: { next_cursor: null, limit: 1 } },
      { items: [{ ...row(), short_oid: OID.slice(0, 7) }], page: { next_cursor: null, limit: 1 } },
      { items: [{ ...row(), summary: "x".repeat(8 * 1024 + 1) }], page: { next_cursor: null, limit: 1 } },
      { items: [{ ...row(), author: "x".repeat(1_025) }], page: { next_cursor: null, limit: 1 } },
      { items: [{ ...row(), committed_at: -1 }], page: { next_cursor: null, limit: 1 } },
      { items: [{ ...row(), parents: Array(65).fill(OID) }], page: { next_cursor: null, limit: 1 } },
      { items: [row()], page: { next_cursor: null, limit: 1, offset: 0 } },
      { items: [row()], page: { next_cursor: `${cursor}=`, limit: 1 } },
      {
        items: [row()],
        page: { next_cursor: `pc1_${wrongVersionFrame.toString("base64url")}`, limit: 1 },
      },
      {
        items: [row()],
        page: { next_cursor: `pc1_${zeroPositionFrame.toString("base64url")}`, limit: 1 },
      },
      {
        items: [row()],
        page: {
          next_cursor: `pc1_${invalidBaseSentinelFrame.toString("base64url")}`,
          limit: 1,
        },
      },
      { items: [row()], page: { next_cursor: `pc1_${"a".repeat(253)}`, limit: 1 } },
    ]) expect(parsePrCommitsPage(value), JSON.stringify(value)).toBeNull();
  });

  it.each([
    () => parseCommitsPage({ items: [], page: { next_cursor: null, limit: 0 } }),
    () => parseCommitsPage({ items: Array(101).fill({}), page: { next_cursor: null, limit: 50 } }),
    () => parseCommitDiff({ oid: "short" }),
    () => parseCommitDiff({
      oid: OID, short_oid: OID.slice(0, 12), summary: "x", message: "x", author: "u",
      committed_at: 1, parents: [], files: [{ path: "../x", old_path: null, status: "A", lines: [] }],
    }),
    () => parseCommitDiff({
      oid: OID, short_oid: OID.slice(0, 12), summary: "x", message: "x", author: "u",
      committed_at: 1, parents: [], files: [{ path: "x", old_path: null, status: "A", lines: [{ origin: "!", content: "x" }] }],
    }),
  ])("rejects malformed or unbounded commit payload", (parse) => {
    expect(parse()).toBeNull();
  });
});
