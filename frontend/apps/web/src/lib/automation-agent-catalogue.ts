import type { AgentChoice, AgentChoicePage } from "./agent-thread-response";

export function mergeAutomationAgentPages(
  first: AgentChoice[] = [],
  pages: AgentChoicePage[] = [],
): AgentChoice[] {
  const seen = new Set<string>();
  return first.concat(...pages.map((page) => page.items)).filter((agent) => {
    if (seen.has(agent.id)) return false;
    seen.add(agent.id);
    return true;
  });
}
