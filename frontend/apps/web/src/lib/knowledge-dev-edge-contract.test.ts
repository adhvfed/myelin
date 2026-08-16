import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import {
  KnowledgeFixtures,
  knowledgeTemplateBlocks,
} from "../../dev-edge/knowledge-contract.mjs";

const goldenPath = new URL(
  "../../../../../contracts/knowledge-page-templates.golden.json",
  import.meta.url,
);
const golden = JSON.parse(readFileSync(goldenPath, "utf8")) as {
  contract_id: string;
  templates: Array<{
    id: string;
    blocks: Array<{ type: string; markdown: string }>;
  }>;
};

const page = {
  title: "Retry-safe runbook",
  template: "runbook",
  visibility: "team",
};

describe("the development Knowledge mutation contract", () => {
  it("serves the same useful starting documents as the durable Edge", () => {
    expect(golden.contract_id).toBe("knowledge-page-template-parity");
    for (const template of golden.templates) {
      expect(knowledgeTemplateBlocks(template.id), template.id)
        .toEqual(template.blocks.map(({ type, markdown }) => [type, markdown]));
    }
  });

  it("replays one header-scoped operation without cloning its page", () => {
    const knowledge = new KnowledgeFixtures();
    knowledge.reset({ empty: true });

    const created = knowledge.create(page, "create-runbook-1");
    const replayed = knowledge.create(page, "create-runbook-1");
    const createdId = created.json?.page?.id;

    expect(created).toMatchObject({ status: 201, json: { created: true } });
    expect(createdId).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);
    expect(replayed).toMatchObject({
      status: 200,
      json: { created: false, page: { id: createdId } },
    });
    expect(knowledge.list({ cursor: undefined, limit: 100 }).items).toHaveLength(1);
  });

  it("rejects missing operation identity and the retired body-carried token", () => {
    const knowledge = new KnowledgeFixtures();
    knowledge.reset({ empty: true });

    expect(knowledge.create(page, undefined)).toEqual({ status: 400 });
    expect(knowledge.create({ ...page, client_nonce: "legacy" }, "create-runbook-2"))
      .toEqual({ status: 400 });
  });
});
