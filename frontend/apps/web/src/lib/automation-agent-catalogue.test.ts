import { describe, expect, it } from "vitest";

import type { AgentChoice, AgentChoicePage } from "./agent-thread-response";
import { mergeAutomationAgentPages } from "./automation-agent-catalogue";

function agent(id: string): AgentChoice {
  return {
    id,
    name: `agent-${id}`,
    owner_principal_id: "owner",
    runtime_ref: "hosted:luna",
    status: "active",
    selected_tools: ["ci.read_run"],
    effective_tools: ["ci.read_run"],
    workspace_profile: { revision: 0, personalization: null, updated_at: null },
  };
}

function page(items: AgentChoice[], next: string | null): AgentChoicePage {
  return { items, page: { next_cursor: next, limit: 100 } };
}

describe("automation agent catalogue", () => {
  it("merges overlapping pages once", () => {
    const first = page([agent("a"), agent("b")], "b");
    const earlier = page([agent("b"), agent("c")], "c");

    expect(mergeAutomationAgentPages(first.items, [earlier]).map((row) => row.id))
      .toEqual(["a", "b", "c"]);
  });
});
