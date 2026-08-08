import { fireEvent, render, screen } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { describe, expect, it } from "vitest";
import { BlockEditor, balanceInlineMarks, splitBlock, toggleInlineMark, type EditorBlock } from "./BlockEditor";

describe("BlockEditor", () => {
  it("balances inline marks when splitting and turns a heading continuation into text", () => {
    const [before, after] = splitBlock({ type: "heading", markdown: "A **bold thought" }, 8);
    expect(before.markdown).toBe("A **bold**");
    expect(after).toEqual({ type: "paragraph", markdown: " thought" });
    expect(balanceInlineMarks("`code")).toBe("`code`");
  });

  it("toggles markdown marks without owning document persistence", () => {
    expect(toggleInlineMark("clear words", 6, 11, "**")).toBe("clear **words**");
    expect(toggleInlineMark("clear **words**", 6, 15, "**")).toBe("clear words");
  });

  it("is controlled, reports edits, and exposes the slash block menu", async () => {
    const Harness = () => {
      const [blocks, setBlocks] = createSignal<EditorBlock[]>([{ type: "paragraph", markdown: "Start" }]);
      return <BlockEditor value={blocks()} onChange={setBlocks} />;
    };
    render(() => <Harness />);
    const input = screen.getByRole("textbox", { name: "Text block 1" });
    input.textContent = "/";
    await fireEvent.input(input);
    expect(screen.getByRole("menu", { name: "Block type" })).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("menuitem", { name: "Heading" }));
    expect(screen.getByRole("textbox", { name: "Heading block 1" })).toHaveTextContent("");
  });

  it("renders crypto-erased content as a non-editable tombstone", () => {
    render(() => <BlockEditor value={[{ type: "paragraph", markdown: "", state: "tombstoned" }]} onChange={() => {}} />);
    expect(screen.getByText("This block was erased.")).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});
