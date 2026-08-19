import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { awaitActiveIssue } from "../src/journeys/issues.js";
import { awaitBacklink, awaitBacklinkGone } from "../src/journeys/refs.js";
import { array, integer, record, string, type JsonRecord } from "../src/json.js";

describe("knowledge lifecycle", () => {
  test("creates, edits, and conflict-checks a durable knowledge page", async () => {
    const title = uniqueName("System-tested product spec");
    const retryKey = `knowledge-${randomUUID()}`;
    const createBody = {
      title,
      template: "product-spec",
      visibility: "team",
    };
    const created = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: createBody,
      idempotencyKey: retryKey,
      expectedStatus: 201,
    });
    const page = record(created.body.page, "created knowledge page");
    const pageId = string(page.id, "knowledge page id");
    const initialVersion = integer(page.version, "knowledge page version");
    expect(created.body).toMatchObject({ created: true, durable: true });
    expect(page).toMatchObject({ title, visibility: "team", can_edit: true });
    const startingBlocks = array(page.blocks, "product-spec starting blocks").map((value) => {
      const block = record(value, "product-spec starting block");
      return [block.type, block.markdown];
    });
    expect(startingBlocks).toEqual([
      ["heading", "Problem"],
      ["paragraph", "What user or organisational problem are we solving?"],
      ["heading", "Outcomes"],
      ["bullet_list", "Describe the measurable change this work should create."],
      ["heading", "Approach"],
      ["paragraph", "Explain the smallest coherent approach and the alternatives considered."],
      ["heading", "Risks"],
      ["bullet_list", "Name failure modes, privacy implications, and how we will observe them."],
    ]);

    const replay = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: createBody,
      idempotencyKey: retryKey,
      expectedStatus: 200,
    });
    expect(replay.body).toMatchObject({ created: false, durable: true, page: { id: pageId } });

    const reviewerView = await reviewerClient.json(
      `/v1/knowledge/pages/${encodeURIComponent(pageId)}`,
    );
    expect(reviewerView.body).toMatchObject({
      page: { id: pageId, title, visibility: "team", can_edit: false },
    });

    const reviewerSave = await reviewerClient.json(
      `/v1/knowledge/pages/${encodeURIComponent(pageId)}`,
      {
        method: "PUT",
        body: {
          expected_version: initialVersion,
          title: "A reviewer must not overwrite this page",
          visibility: "team",
          blocks: [{ type: "paragraph", markdown: "Unauthorized replacement." }],
        },
        expectedStatus: 404,
      },
    );
    expect(reviewerSave.body).toMatchObject({ error: { code: "not_found" } });

    const unchanged = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`);
    expect(unchanged.body).toMatchObject({
      page: { id: pageId, title, version: initialVersion, can_edit: true },
    });

    const editedTitle = `${title} — approved`;
    const saved = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`, {
      method: "PUT",
      body: {
        expected_version: initialVersion,
        title: editedTitle,
        visibility: "team",
        blocks: [
          { type: "heading", markdown: "Outcome" },
          { type: "paragraph", markdown: "Every engineering workflow is observable end to end." },
          { type: "task_list", markdown: "Keep the contract suite green." },
        ],
      },
    });
    const savedPage = record(saved.body.page, "saved knowledge page");
    const savedVersion = integer(saved.body.version, "saved knowledge version");
    expect(saved.body).toMatchObject({ durable: true });
    expect(savedVersion).toBeGreaterThan(initialVersion);
    expect(savedPage).toMatchObject({ id: pageId, title: editedTitle, version: savedVersion });
    expect(array(savedPage.blocks, "saved knowledge blocks")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ type: "paragraph", markdown: expect.stringContaining("observable") }),
      ]),
    );

    const stale = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`, {
      method: "PUT",
      body: {
        expected_version: initialVersion,
        title: "Stale overwrite",
        visibility: "team",
        blocks: [{ type: "paragraph", markdown: "This must not win." }],
      },
      expectedStatus: 409,
    });
    expect(stale.body).toHaveProperty("error");

    const persisted = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`);
    expect(persisted.body).toMatchObject({ page: { title: editedTitle, version: savedVersion } });
    const pages = await systemClient.json("/v1/knowledge/pages?limit=100");
    expect(array(pages.body.items, "knowledge list items")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: pageId, title: editedTitle })]),
    );
  });

  test("lets a living document point to delivery work, then forget the link cleanly", async () => {
    const issue = await awaitActiveIssue(systemClient, uniqueName("Deliver the linked runbook"));
    const issueRef = string(issue.ref, "linked delivery issue ref");
    const title = uniqueName("Linked delivery runbook");
    const created = await systemClient.json("/v1/knowledge/pages", {
      method: "POST",
      body: { title, template: "blank", visibility: "team" },
      expectedStatus: 201,
    });
    const page = record(created.body.page, "linked knowledge page");
    const pageId = string(page.id, "linked knowledge page id");
    const pageRef = string(page.ref, "linked knowledge page ref");
    const initialVersion = integer(page.version, "linked knowledge page version");

    const linked = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`, {
      method: "PUT",
      body: {
        expected_version: initialVersion,
        title,
        visibility: "team",
        blocks: [
          {
            type: "paragraph",
            markdown: "Follow the delivery issue ￼ through completion.",
            references: [issueRef],
          },
          {
            type: "paragraph",
            markdown: "Record what we learn beside ￼ while the work is fresh.",
            references: [issueRef],
          },
        ],
      },
    });
    const linkedPage = record(linked.body.page, "knowledge page with delivery link");
    const linkedVersion = integer(linked.body.version, "linked knowledge version");
    const linkedBlocks = array(linkedPage.blocks, "linked knowledge blocks")
      .map((block) => record(block, "linked knowledge block"));
    const linkedBlockIds = linkedBlocks
      .map((block) => string(block.id, "linked knowledge block id"));
    const linkedBlockRefs = linkedBlockIds.map((blockId) => `${pageRef}#b${blockId}`);
    expect(linkedBlocks).toEqual([
      expect.objectContaining({ references: [issueRef] }),
      expect.objectContaining({ references: [issueRef] }),
    ]);

    const firstPage = await eventually<JsonRecord>(async () => {
      const response = await systemClient.json(
        `/v1/refs/backlinks?ref=${encodeURIComponent(issueRef)}&limit=1`,
      );
      const body = record(response.body, "first backlink page");
      const items = array(body.items, "first backlink page items");
      const page = record(body.page, "first backlink page cursor");
      return items.length === 1 && typeof page.next_cursor === "string" ? body : undefined;
    }, {
      description: "both passages to become independently pageable issue backlinks",
    });
    const firstItems = array(firstPage.items, "first backlink page items")
      .map((item) => record(item, "first paged backlink"));
    const firstCursor = string(
      record(firstPage.page, "first backlink page cursor").next_cursor,
      "first backlink cursor",
    );
    const secondPage = await systemClient.json(
      `/v1/refs/backlinks?ref=${encodeURIComponent(issueRef)}&limit=1&cursor=${encodeURIComponent(firstCursor)}`,
    );
    const secondItems = array(secondPage.body.items, "second backlink page items")
      .map((item) => record(item, "second paged backlink"));
    const pagedRefs = [...firstItems, ...secondItems]
      .map((item) => string(item.ref, "paged backlink ref"));
    expect(pagedRefs.sort()).toEqual([...linkedBlockRefs].sort());
    expect(secondPage.body).toMatchObject({ page: { limit: 1, next_cursor: null } });
    for (const backlink of [...firstItems, ...secondItems]) {
      expect(backlink).toMatchObject({
        root_ref: pageRef,
        relation: "links",
        relation_class: "reference",
        target_ref: issueRef,
      });
    }

    const unlinked = await systemClient.json(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`, {
      method: "PUT",
      body: {
        expected_version: linkedVersion,
        title,
        visibility: "team",
        blocks: [
          {
            id: linkedBlockIds[0],
            type: "paragraph",
            markdown: "Delivery is complete; this runbook now stands on its own.",
          },
          {
            id: linkedBlockIds[1],
            type: "paragraph",
            markdown: "The lasting lesson no longer needs a live work-item link.",
          },
        ],
      },
    });
    expect(unlinked.body).toMatchObject({
      durable: true,
      page: {
        blocks: [
          expect.objectContaining({ references: [] }),
          expect.objectContaining({ references: [] }),
        ],
      },
    });
    await awaitBacklinkGone(systemClient, issueRef, pageRef, "links");
  });

});
