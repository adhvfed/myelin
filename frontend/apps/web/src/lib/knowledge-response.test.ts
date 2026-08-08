import { describe, expect, it } from "vitest";
import {
  parseKnowledgeCreateDraft, parseKnowledgeCreateReceipt, parseKnowledgePage,
  parseKnowledgePages, parseKnowledgeSaveDraft, parseKnowledgeSaveReceipt,
} from "./knowledge-response";

const ID = "01J00000000000000000000000";
const BLOCK = "01J00000000000000000000001";
const summary = {
  id: ID, space: "engineering", parent_page_id: null, title: "Incident response",
  title_state: "active", visibility: "private", version: 1, can_edit: true,
  created_at: 1_700_000_000, updated_at: 1_700_000_000,
};
const page = { ...summary, blocks: [{ id: BLOCK, type: "heading", markdown: "Response", state: "active", is_you: true }] };

describe("Knowledge wire projection", () => {
  it("strictly decodes list, document, and durable receipts", () => {
    expect(parseKnowledgePages({ items: [summary], page: { next_cursor: null, limit: 50 } })?.items[0]).toEqual(summary);
    expect(parseKnowledgePage({ page })).toEqual(page);
    expect(parseKnowledgeCreateReceipt({ page, created: true, durable: true })?.page.id).toBe(ID);
    expect(parseKnowledgeSaveReceipt({ page, version: 1, durable: true })?.version).toBe(1);
  });

  it("rejects surplus fields, invalid block types, and malformed tombstones", () => {
    expect(parseKnowledgePages({ items: [{ ...summary, owner: "leak" }], page: { next_cursor: null, limit: 50 } })).toBeNull();
    expect(parseKnowledgePage({ page: { ...page, blocks: [{ ...page.blocks[0], type: "html" }] } })).toBeNull();
    expect(parseKnowledgePage({ page: { ...page, blocks: [{ ...page.blocks[0], state: "tombstoned", markdown: "secret" }] } })).toBeNull();
  });
});

describe("Knowledge mutation input", () => {
  it("accepts bounded creation and controlled editor state", () => {
    expect(parseKnowledgeCreateDraft({ title: "Runbook", template: "runbook", visibility: "team", clientNonce: "browser_1" })).not.toBeNull();
    expect(parseKnowledgeSaveDraft({ pageId: ID, expectedVersion: 1, title: "Runbook", visibility: "private", blocks: [{ id: BLOCK, type: "paragraph", markdown: "Hello", state: "active" }] })).not.toBeNull();
  });

  it("rejects hidden scope, blank titles, and invented erased content", () => {
    expect(parseKnowledgeCreateDraft({ title: "", template: "blank", visibility: "private", clientNonce: "x" })).toBeNull();
    expect(parseKnowledgeCreateDraft({ title: "Doc", template: "blank", visibility: "private", clientNonce: "x", tenant: "other" })).toBeNull();
    expect(parseKnowledgeSaveDraft({ pageId: ID, expectedVersion: 1, title: "Doc", visibility: "private", blocks: [{ id: BLOCK, type: "paragraph", markdown: "restored?", state: "tombstoned" }] })).toBeNull();
  });
});
