import { fireEvent, render, screen } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { describe, expect, it } from "vitest";
import { BlockEditor, balanceInlineMarks, mergeBlocks, splitBlock, toggleInlineMark, type EditorBlock } from "./BlockEditor";

describe("BlockEditor", () => {
  it("balances inline marks when splitting and turns a heading continuation into text", () => {
    const [before, after] = splitBlock({ type: "heading", markdown: "A **bold thought" }, 8);
    expect(before.markdown).toBe("A **bold**");
    expect(after).toEqual({ type: "paragraph", markdown: " thought" });
    expect(balanceInlineMarks("`code")).toBe("`code`");
  });

  it("keeps structured references attached to their text when blocks split and merge", () => {
    const block: EditorBlock = {
      type: "paragraph",
      markdown: "Issue \uFFFC meets pull request \uFFFC",
      references: ["myelin://acme/issue/issue/MYL-7", "myelin://acme/git/pr/core:3"],
    };
    const [before, after] = splitBlock(block, "Issue \uFFFC".length);
    expect(before).toMatchObject({ markdown: "Issue \uFFFC", references: ["myelin://acme/issue/issue/MYL-7"] });
    expect(after).toMatchObject({ markdown: " meets pull request \uFFFC", references: ["myelin://acme/git/pr/core:3"] });
    expect(mergeBlocks(before, after)).toMatchObject(block);
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

  it("keeps the active editable node stable while controlled value updates arrive", async () => {
    const Harness = () => {
      const [blocks, setBlocks] = createSignal<EditorBlock[]>([{ type: "paragraph", markdown: "" }]);
      return <BlockEditor value={blocks()} onChange={setBlocks} inputLabel="Comment" />;
    };
    render(() => <Harness />);
    const input = screen.getByRole("textbox", { name: "Comment" });
    input.focus();
    input.textContent = "K";
    await fireEvent.input(input);
    expect(screen.getByRole("textbox", { name: "Comment" })).toBe(input);
    input.textContent = "Ki";
    await fireEvent.input(input);
    expect(screen.getByRole("textbox", { name: "Comment" })).toBe(input);
    expect(input).toHaveTextContent("Ki");
  });

  it("renders references as named chips and removes their metadata with the chip", async () => {
    const reference = "myelin://acme/issue/issue/MYL-7";
    const Harness = () => {
      const [blocks, setBlocks] = createSignal<EditorBlock[]>([{
        type: "paragraph",
        markdown: "Follow \uFFFC",
        references: [reference],
      }]);
      return <>
        <BlockEditor value={blocks()} onChange={setBlocks} referenceLabel={() => "MYL-7"} />
        <output>{JSON.stringify(blocks())}</output>
      </>;
    };
    render(() => <Harness />);

    const input = screen.getByRole("textbox", { name: "Text block 1" });
    expect(screen.getByLabelText("Reference: MYL-7")).toHaveAttribute("title", reference);
    input.querySelector("[data-block-reference]")?.remove();
    await fireEvent.input(input);
    expect(screen.getByRole("status")).toHaveTextContent('"markdown":"Follow ","references":[]');
  });

  it("makes references navigable in read-only documents", () => {
    const reference = "myelin://acme/knowledge/page/01J00000000000000000000000";
    render(() => <BlockEditor
      value={[{ type: "paragraph", markdown: "Read \uFFFC", references: [reference] }]}
      onChange={() => {}}
      readOnly
      referenceLabel={() => "Deployment guide"}
      referenceHref={() => "/knowledge?page=01J00000000000000000000000"}
    />);
    expect(screen.getByRole("link", { name: "Reference: Deployment guide" }))
      .toHaveAttribute("href", "/knowledge?page=01J00000000000000000000000");
  });
});
