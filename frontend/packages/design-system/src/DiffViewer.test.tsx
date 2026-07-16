// DiffViewer gate (R3.2 · G-7 · R-17 §5.1, WCAG 1.4.1): change kind is announced as TEXT with the
// line number in the SR prefix (never colour alone); binary files never dump text; split + unified
// both render; the line grid is one tab stop (roving tabindex).
import { render, screen } from "@solidjs/testing-library";
import { describe, it, expect } from "vitest";
import { DiffViewer, type DiffViewerFile } from "./DiffViewer";

const modified: DiffViewerFile = {
  path: "src/list_filter.rs",
  old_path: null,
  status: "M",
  kind: "text",
  additions: 2,
  deletions: 1,
  hunks: [
    {
      header: "@@ -1,3 +1,4 @@",
      old_start: 1,
      old_lines: 3,
      new_start: 1,
      new_lines: 4,
      lines: [
        { origin: " ", content: "a", old_no: 1, new_no: 1 },
        { origin: "-", content: "b", old_no: 2, new_no: null },
        { origin: "+", content: "B", old_no: null, new_no: 2 },
        { origin: " ", content: "c", old_no: 3, new_no: 3 },
        { origin: "+", content: "d", old_no: null, new_no: 4 },
      ],
    },
  ],
};

describe("DiffViewer — SR contract", () => {
  it("announces change kind + line number as TEXT (never colour alone)", () => {
    render(() => <DiffViewer files={[modified]} view="unified" />);
    // The added line carries "added, new line 4:" as visually-hidden TEXT.
    expect(screen.getByText(/added, new line 4:/)).toBeTruthy();
    expect(screen.getByText(/removed, old line 2:/)).toBeTruthy();
    expect(screen.getByText(/unchanged, line 1:/)).toBeTruthy();
  });

  it("renders both split and unified without crashing and shows the file header", () => {
    const { unmount } = render(() => <DiffViewer files={[modified]} view="split" />);
    expect(screen.getByText("src/list_filter.rs")).toBeTruthy();
    unmount();
    render(() => <DiffViewer files={[modified]} view="unified" />);
    expect(screen.getByText("src/list_filter.rs")).toBeTruthy();
  });

  it("marks each code cell as one tab stop (roving tabindex, none is a positive tabindex)", () => {
    const { container } = render(() => <DiffViewer files={[modified]} view="unified" />);
    const cells = container.querySelectorAll("[data-rowkey]");
    expect(cells.length).toBeGreaterThan(0);
    for (const c of cells) {
      const ti = c.getAttribute("tabindex");
      expect(ti === "0" || ti === "-1").toBe(true);
    }
  });
});

describe("DiffViewer — R-21 rows", () => {
  it("renders a binary file as a no-text-diff row (never a garbled dump)", () => {
    const bin: DiffViewerFile = { path: "logo.png", status: "A", kind: "binary", hunks: [], size_bytes: 2048 };
    render(() => <DiffViewer files={[bin]} view="unified" />);
    expect(screen.getByTestId("binary-row").textContent).toMatch(/Binary file/);
  });

  it("tombstones a deleted file behind 'Show deleted contents' (never a red wall)", () => {
    const del: DiffViewerFile = {
      path: "old.rs",
      status: "D",
      kind: "text",
      deletions: 200,
      deleted_body_available: true,
      hunks: [{ header: "@@ -1,1 +0,0 @@", old_start: 1, old_lines: 1, new_start: 0, new_lines: 0, lines: [{ origin: "-", content: "x", old_no: 1, new_no: null }] }],
    };
    render(() => <DiffViewer files={[del]} view="unified" />);
    expect(screen.getByTestId("deleted-tombstone")).toBeTruthy();
    expect(screen.getByTestId("show-deleted")).toBeTruthy();
  });
});
