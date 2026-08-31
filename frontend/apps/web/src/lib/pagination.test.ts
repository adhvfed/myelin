import { describe, expect, it } from "vitest";

import { nextCursorForPages, type CursorPage } from "./pagination";

function page(next: string | null): CursorPage {
  return { page: { next_cursor: next } };
}

describe("paged catalogue cursor", () => {
  it("uses the first page until another page has loaded", () => {
    expect(nextCursorForPages(page("first"), [])).toBe("first");
    expect(nextCursorForPages(undefined, [])).toBeNull();
    expect(nextCursorForPages(null, [])).toBeNull();
  });

  it("uses the last loaded page cursor", () => {
    expect(nextCursorForPages(page("first"), [page("second"), page("third")]))
      .toBe("third");
  });

  it("does not resurrect the first cursor after the terminal page", () => {
    expect(nextCursorForPages(page("first"), [page(null)])).toBeNull();
  });
});
