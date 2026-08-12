// Diff rendering, accessible line labels, binary handling, layouts, and keyboard navigation.
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { describe, it, expect, vi } from "vitest";
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
    // The added line has a visually hidden change and line-number prefix.
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

  it("exposes exactly one initial code-cell tab stop", () => {
    const { container } = render(() => <DiffViewer files={[modified]} view="unified" />);
    const cells = container.querySelectorAll("[data-rowkey]");
    expect(cells.length).toBeGreaterThan(0);
    expect(Array.from(cells).filter((cell) => cell.getAttribute("tabindex") === "0")).toHaveLength(1);
    expect(Array.from(cells).filter((cell) => cell.getAttribute("tabindex") === "-1")).toHaveLength(cells.length - 1);
  });

  it("handles line navigation on the focused cell", () => {
    const scrollIntoView = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = vi.fn();
    try {
      const { container } = render(() => <DiffViewer files={[modified]} view="unified" />);
      const cells = container.querySelectorAll<HTMLElement>("[data-rowkey]");
      cells[0]!.focus();

      fireEvent.keyDown(cells[0]!, { key: "j" });

      expect(document.activeElement).toBe(cells[1]);
    } finally {
      HTMLElement.prototype.scrollIntoView = scrollIntoView;
    }
  });

  it("keeps file navigation working after focus moves onto file headers", () => {
    const scrollIntoView = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = vi.fn();
    try {
      const files = [
        modified,
        { ...modified, path: "src/second.rs" },
        { ...modified, path: "src/third.rs" },
      ];
      const { container } = render(() => <DiffViewer files={files} view="unified" />);
      const cells = container.querySelectorAll<HTMLElement>("[data-rowkey]");
      const headers = container.querySelectorAll<HTMLElement>("[data-fileheader]");
      cells[0]!.focus();

      fireEvent.keyDown(cells[0]!, { key: "n" });
      expect(document.activeElement).toBe(headers[1]);
      fireEvent.keyDown(headers[1]!, { key: "n" });
      expect(document.activeElement).toBe(headers[2]);
      fireEvent.keyDown(headers[2]!, { key: "p" });
      expect(document.activeElement).toBe(headers[1]);
    } finally {
      HTMLElement.prototype.scrollIntoView = scrollIntoView;
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

describe("DiffViewer — bounded context expansion", () => {
  const withGap: DiffViewerFile = {
    ...modified,
    new_blob_oid: "c3d4e5f60718293a4b5c6d7e8f90011223344556",
    hunks: [
      modified.hunks[0]!,
      {
        header: "@@ -20,1 +20,1 @@",
        old_start: 20,
        old_lines: 1,
        new_start: 20,
        new_lines: 1,
        lines: [{ origin: " ", content: "later", old_no: 20, new_no: 20 }],
      },
    ],
  };

  it("only exposes a live control backed by a new-side blob", () => {
    const onExpand = vi.fn();
    const { unmount } = render(() => (
      <DiffViewer files={[withGap]} view="unified" onExpandContext={onExpand} />
    ));
    fireEvent.click(screen.getByTestId("expand-all"));
    expect(onExpand).toHaveBeenCalledWith(0, "1", "all");
    unmount();

    render(() => (
      <DiffViewer files={[{ ...withGap, new_blob_oid: null }]} view="unified" onExpandContext={onExpand} />
    ));
    expect(screen.queryByTestId("expand-all")).toBeNull();
  });

  it("injects expanded lines through the unified and split flat-row paths", () => {
    const expanded = {
      "0:1": [{ origin: " ", content: "expanded context", old_no: 5, new_no: 5 }],
    };
    const { unmount } = render(() => (
      <DiffViewer files={[withGap]} view="unified" onExpandContext={() => undefined} expandedContext={expanded} />
    ));
    expect(screen.getByText("expanded context")).toBeTruthy();
    expect(screen.queryByTestId("expand-all")).toBeNull();
    unmount();

    render(() => (
      <DiffViewer files={[withGap]} view="split" onExpandContext={() => undefined} expandedContext={expanded} />
    ));
    expect(screen.getAllByText("expanded context")).toHaveLength(2);
  });
});
