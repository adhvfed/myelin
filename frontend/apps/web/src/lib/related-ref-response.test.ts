import { describe, expect, it } from "vitest";

import { parseRelatedRefsPage } from "./related-ref-response";

const PR = "myelin://acme/git/pr/core:42";
const ISSUE = "myelin://acme/issue/issue/ENG-7";
const edge = {
  ref: ISSUE,
  root_ref: ISSUE,
  source_ref: PR,
  source_root_ref: PR,
  target_ref: ISSUE,
  target_root_ref: ISSUE,
  relation: "closes",
  relation_class: "lifecycle",
  origin_actor: "psn:author",
};

describe("related reference projection", () => {
  it("accepts a bounded visible edge page", () => {
    expect(parseRelatedRefsPage({
      ref: PR,
      root_ref: PR,
      items: [edge],
      page: { next_cursor: null, limit: 100 },
    })).toEqual({
      ref: PR,
      root_ref: PR,
      items: [edge],
      page: { next_cursor: null, limit: 100 },
    });
  });

  it.each([
    { ...edge, secret: "leak" },
    { ...edge, ref: "https://example.com" },
    { ...edge, root_ref: `${ISSUE}#comment` },
    { ...edge, relation: "invented" },
  ])("rejects malformed edge fields without projecting partial data", (item) => {
    expect(parseRelatedRefsPage({
      ref: PR,
      root_ref: PR,
      items: [item],
      page: { next_cursor: null, limit: 100 },
    })).toBeNull();
  });

  it("rejects a malformed or surprising envelope", () => {
    expect(parseRelatedRefsPage({
      ref: PR,
      root_ref: PR,
      items: [edge],
      page: { next_cursor: null, limit: 50 },
    })).toBeNull();
  });
});
