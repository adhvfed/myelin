import { describe, expect, it } from "vitest";

import {
  DEFAULT_HOSTED_AGENT_TOOLS,
  DEFAULT_PERSONAL_AGENT_TOOLS,
  PERSONAL_AGENT_TOOLS,
  parseAgentToolDefinitionPage,
  validAgentToolPalette,
} from "./agent-tool-palette";

describe("personal agent tool palette", () => {
  it("makes the complete default selection explicit while commands require a choice", () => {
    expect(DEFAULT_PERSONAL_AGENT_TOOLS).toEqual([
      "chat.read_messages",
      "chat.post",
      "chat.reply",
      "workspace.read_file",
      "workspace.write_file",
    ]);
    expect(PERSONAL_AGENT_TOOLS.map((tool) => tool.name)).toContain("workspace.exec");
    expect(DEFAULT_PERSONAL_AGENT_TOOLS).not.toContain("workspace.exec");
    expect(validAgentToolPalette(DEFAULT_PERSONAL_AGENT_TOOLS)).toBe(true);
    expect(DEFAULT_HOSTED_AGENT_TOOLS).toEqual(["ci.read_run", "issues.create"]);
    expect(validAgentToolPalette([...DEFAULT_HOSTED_AGENT_TOOLS])).toBe(true);
  });

  it("refuses empty, duplicate, malformed, and unbounded palettes", () => {
    expect(validAgentToolPalette([])).toBe(false);
    expect(validAgentToolPalette(["chat.post", "chat.post"])).toBe(false);
    expect(validAgentToolPalette(["Chat post"])).toBe(false);
    expect(validAgentToolPalette(Array.from({ length: 129 }, (_, index) => `tool.t${index}`)))
      .toBe(false);
  });

  it("strictly decodes the live platform catalogue used by the editor", () => {
    const tool = {
      name: "git.read_file",
      ref: "myelin://acme/agent/tool/git.read_file@2",
      subsystem: "git",
      version: 2,
      input_schema: { type: "object" },
      required_capabilities: ["repo.pull"],
      effect_kind: "read",
      side_effecting: false,
      requires_approval: false,
      exposed_over_mcp: true,
    };
    expect(parseAgentToolDefinitionPage({
      items: [tool],
      page: { next_cursor: "git.read_file.v2", limit: 100 },
    }))?.toMatchObject({ items: [{ name: "git.read_file", version: 2 }] });
    expect(parseAgentToolDefinitionPage({
      items: [{ ...tool, bearer_token: "must-not-cross" }],
      page: { next_cursor: null, limit: 100 },
    })).toBeNull();
    expect(parseAgentToolDefinitionPage({
      items: [tool, tool],
      page: { next_cursor: null, limit: 100 },
    })).toBeNull();
  });
});
