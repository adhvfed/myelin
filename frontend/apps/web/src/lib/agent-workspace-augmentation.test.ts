import { describe, expect, it } from "vitest";

import {
  agentWorkspaceAugmentationFits,
  parseAgentWorkspaceFiles,
  validAgentWorkspaceAugmentation,
} from "./agent-workspace-augmentation";

describe("agent workspace augmentation input", () => {
  it("accepts the exact bounded create-only file contract", () => {
    const files = [{
      path: ".myelin/context.md",
      contents: "# Private context\n",
      strategy: "create" as const,
    }];
    expect(parseAgentWorkspaceFiles(files)).toEqual(files);
    expect(validAgentWorkspaceAugmentation("Explain the tradeoff.", files)).toBe(true);
  });

  it("rejects ambiguous, reserved, and duplicate paths", () => {
    for (const files of [
      [{ path: "../escape", contents: "x", strategy: "create" }],
      [{ path: ".git/config", contents: "x", strategy: "create" }],
      [{ path: "AGENTS.md", contents: "x", strategy: "create" }],
      [
        { path: "context.md", contents: "first", strategy: "create" },
        { path: "context.md", contents: "second", strategy: "create" },
      ],
    ]) {
      expect(parseAgentWorkspaceFiles(files)).toBeNull();
    }
  });

  it("accounts for the complete encoded envelope", () => {
    const files = [{ path: "context.md", contents: "x".repeat(12 * 1_024), strategy: "create" as const }];
    expect(agentWorkspaceAugmentationFits(null, files)).toBe(false);
  });
});
